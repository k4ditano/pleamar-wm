//! Native program identities and persistent pins for the scene's dock.
use super::*;
use pleamar::scene::NestEvent;
use std::{collections::BTreeMap, io::{Read, Write}, os::windows::fs::OpenOptionsExt, path::PathBuf};
use windows::Win32::{Storage::{FileSystem::*, Packaging::Appx::GetApplicationUserModelId},
    System::Com::{*, StructuredStorage::{PropVariantClear, PropVariantToStringAlloc}},
    UI::Shell::{*, PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow}}};
use windows::core::Interface;

#[path = "windows_dock_activation.rs"]
mod activation;
pub(super) use activation::Activations;

const APP_ID: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3), pid: 5 };
const MAX_FILE: u64 = 128 * 1024;

type LookupResult = (String, std::result::Result<Program,String>);
pub(super) struct Lookup {
    requests:Option<std::sync::mpsc::SyncSender<Window>>,
    replies:std::sync::mpsc::Receiver<LookupResult>,
    active:std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Lookup {
    pub fn new(wake:std::sync::Arc<wait::Wake>) -> Result<Self> {
        use std::sync::{mpsc,atomic::{AtomicBool,Ordering},Arc};
        let (requests,receive)=mpsc::sync_channel::<Window>(64);
        let (send,replies)=mpsc::sync_channel(64);
        let active=Arc::new(AtomicBool::new(true));let alive=active.clone();
        // Shell metadata can load an extension. Its latency must not stall
        // frame delivery, window actions, or the native event pump.
        std::thread::Builder::new().name("native-dock-metadata".into()).spawn(move || {
            while let Ok(window)=receive.recv() {
                if !alive.load(Ordering::Acquire) { break; }
                let result=program(&window).map_err(|e|e.to_string());
                if !alive.load(Ordering::Acquire) { break; }
                if send.send((window.id,result)).is_err() { break; }
                wake.signal();
            }
        })?;
        Ok(Self {requests:Some(requests),replies,active})
    }
    pub fn request(&self,window:&Window) -> bool { self.requests.as_ref().is_some_and(|tx|tx.try_send(window.clone()).is_ok()) }
    pub fn poll(&self) -> Vec<LookupResult> { self.replies.try_iter().collect() }
}
impl Drop for Lookup {
    fn drop(&mut self) {
        self.active.store(false,std::sync::atomic::Ordering::Release);
        self.requests.take();
    }
}

struct Apartment;
impl Apartment {
    fn new() -> Result<Self> { unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()?; } Ok(Self) }
}
impl Drop for Apartment { fn drop(&mut self) { unsafe { CoUninitialize(); } } }

fn wide(value:&str) -> Vec<u16> { value.encode_utf16().chain([0]).collect() }
fn take_string(raw:PWSTR) -> windows::core::Result<String> {
    let value=unsafe { raw.to_string() };
    unsafe { CoTaskMemFree(Some(raw.0 as _)); }
    Ok(value?)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
enum Target { Executable(String), Application(String) }
impl Target {
    fn key(&self) -> String {
        match self { Self::Executable(path)=>format!("exe:{}",path.to_lowercase()), Self::Application(id)=>format!("app:{}",id.to_lowercase()) }
    }
    fn valid(&self) -> bool {
        match self {
            Self::Executable(path)=>path.len()<=32768 && !path.contains('\0') && Path::new(path).is_absolute()
                && Path::new(path).extension().and_then(|e|e.to_str()).is_some_and(|e|e.eq_ignore_ascii_case("exe")),
            Self::Application(id)=>!id.is_empty() && id.len()<=1024 && !id.chars().any(char::is_control),
        }
    }
    fn icon(&self) -> String {
        match self { Self::Executable(path)=>format!("windows-file:{path}"),Self::Application(id)=>format!("windows-app:{id}") }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Program { target:Target, name:String }
impl Program {
    pub fn key(&self) -> String { self.target.key() }
    pub fn event(&self,slot:usize) -> NestEvent {
        NestEvent::Program {slot,icon:self.target.icon(),name:self.name.clone(),exec:self.key()}
    }
    fn pin(&self) -> pleamar::scene::DockPin {
        pleamar::scene::DockPin {keys:vec![self.key()],icon:self.target.icon(),name:self.name.clone(),exec:self.key()}
    }
    fn valid(&self) -> bool { self.target.valid() && !self.name.is_empty() && self.name.len()<=4096 && !self.name.contains('\0') }
}

pub(super) fn program(window:&Window) -> Result<Program> {
    let _apartment=Apartment::new()?;
    let (hwnd,current)=target(&window.id)?;
    let process=unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,current.process) }?;
    let mut path=vec![0u16;32768];let mut length=path.len() as u32;
    let path_result=unsafe { QueryFullProcessImageNameW(process,PROCESS_NAME_WIN32,PWSTR(path.as_mut_ptr()),&mut length) };
    let mut aumid=vec![0u16;1024];let mut id_length=aumid.len() as u32;
    let id_result=unsafe { GetApplicationUserModelId(process,&mut id_length,Some(PWSTR(aumid.as_mut_ptr()))) };
    let _=unsafe { CloseHandle(process) };
    let window_id=(|| -> windows::core::Result<String> { unsafe {
        let properties:IPropertyStore=SHGetPropertyStoreForWindow(hwnd)?;
        let mut value=properties.GetValue(&APP_ID)?;
        let raw=PropVariantToStringAlloc(&value);let _=PropVariantClear(&mut value);
        take_string(raw?)
    } })().ok().filter(|v|!v.is_empty());
    let process_id=(id_result.is_ok() && (1..=1024).contains(&id_length)).then(||wide_text(&aumid));
    let id=window_id.or(process_id);
    // A classic app can declare an AUMID too. Packaged activation needs its
    // package-family!application identity and a matching AppsFolder entry.
    let shell=id.as_ref().filter(|id|id.contains('!')).and_then(|id| {
        let parsing=wide(&format!("shell:AppsFolder\\{id}"));
        unsafe { SHCreateItemFromParsingName::<_,_,IShellItem>(PCWSTR(parsing.as_ptr()),None).ok() }
    });
    let (target,item)=if let (Some(id),Some(item))=(id,shell) { (Target::Application(id),Some(item)) }
    else {
        path_result?;
        let path=String::from_utf16_lossy(&path[..length as usize]);
        let parsing=wide(&path);
        let item=unsafe { SHCreateItemFromParsingName::<_,_,IShellItem>(PCWSTR(parsing.as_ptr()),None).ok() };
        (Target::Executable(path),item)
    };
    let name=item.and_then(|item|unsafe { item.GetDisplayName(SIGDN_NORMALDISPLAY).and_then(take_string).ok() })
        .filter(|v|!v.is_empty()).unwrap_or_else(||window.app.clone());
    let result=Program {target,name};
    if !result.valid() || Identity::read(hwnd).map(Identity::token).as_deref()!=Some(&window.id) {
        return Err("window program identity changed during dock lookup".into());
    }
    Ok(result)
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved { version:u32, pins:Vec<Program> }

pub(super) struct Dock { path:PathBuf, pins:Vec<Program>, programs:BTreeMap<String,Program> }
impl Dock {
    pub fn new(path:PathBuf) -> Result<Self> { Ok(Self {pins:read(&path)?,path,programs:BTreeMap::new()}) }
    pub fn events(&self) -> NestEvent { NestEvent::Dock(self.pins.iter().map(Program::pin).collect()) }
    pub fn remember(&mut self,program:Program) { self.programs.insert(program.key(),program); }
    pub fn retain(&mut self,keys:&HashSet<String>) { self.programs.retain(|key,_|keys.contains(key)); }
    pub fn refresh(&mut self) -> Result<bool> {
        let current=read(&self.path)?;
        if current==self.pins { return Ok(false); }
        self.pins=current;Ok(true)
    }
    pub fn pin(&mut self,key:&str,yes:bool) -> Result<()> {
        let parent=self.path.parent().ok_or("dock configuration has no parent")?;
        std::fs::create_dir_all(parent)?;
        // Serialize read/modify/replace across scene processes without holding
        // a lock for their lifetime, or discarding another scene's pins.
        let _lock=std::fs::OpenOptions::new().create(true).write(true).share_mode(0).open(self.path.with_extension("lock"))?;
        let mut pins=read(&self.path)?;
        if yes && !pins.iter().any(|p|p.key()==key) {
            if pins.len()>=pleamar::scene::DOCK_ITEMS { return Err("the dock has no free pin slots".into()); }
            pins.push(self.programs.get(key).ok_or("the program is no longer in this scene's catalog")?.clone());
        } else if !yes { pins.retain(|p|p.key()!=key); }
        write(&self.path,&pins)?;self.pins=pins;Ok(())
    }
    pub fn open(&self,key:&str,files:&[String],activations:&Activations) -> Result<()> {
        let program=self.programs.get(key).or_else(||self.pins.iter().find(|p|p.key()==key)).ok_or("unknown dock program")?;
        if files.len()>256 || files.iter().any(|p|p.contains('\0') || p.len()>32768 || !Path::new(p).is_absolute()) {
            return Err("invalid dropped file paths".into());
        }
        match &program.target {
            Target::Executable(path)=>{ launch::application(path,files)?; },
            Target::Application(id)=>activations.request(id,files)?,
        }
        Ok(())
    }
}

fn activate_package(app_id:&str,files:&[String]) -> Result<()> {
    // All shell objects stay on the activation worker's STA; capture uses MTA.
    unsafe {CoInitializeEx(None,COINIT_APARTMENTTHREADED|COINIT_DISABLE_OLE1DDE).ok()?;}
    let _apartment=Apartment;
    let id=wide(app_id);
    unsafe {
        let activation:IApplicationActivationManager=CoCreateInstance(&ApplicationActivationManager,None,CLSCTX_LOCAL_SERVER)?;
        if files.is_empty() {activation.ActivateApplication(PCWSTR(id.as_ptr()),w!(""),AO_NONE)?;}
        else {
            let shell_files=files.iter().map(|file|file_item(file)).collect::<Result<Vec<_>>>()?;
            let items=file_array(&shell_files)?;
            // One drop is one Windows.File activation. Retrying that contract
            // between classic-handler launches can disrupt the preceding app.
            match activation.ActivateForFile(PCWSTR(id.as_ptr()),&items,w!("open")) {
                Ok(_)=>{},
                // Packaged desktop apps can register associations without
                // implementing UWP's Windows.File contract.
                Err(error) if error.code().0 as u32==0x80270254=>{
                    let handlers=files.iter().map(|file|file_handler(app_id,file)).collect::<Result<Vec<_>>>()?;
                    for (item,handler) in shell_files.iter().zip(handlers) {
                        let data:IDataObject=item.BindToHandler(None,&BHID_DataObject)?;
                        handler.Invoke(&data).map_err(|e|format!("Windows registered file handler: {e}"))?;
                    }
                },
                Err(error)=>return Err(format!("Windows packaged file activation: {error}").into()),
            }
        }
    }
    Ok(())
}

fn file_handler(app_id:&str,path:&str) -> Result<IAssocHandler> {
    let extension=Path::new(path).extension().and_then(|s|s.to_str()).filter(|s|!s.is_empty())
        .ok_or("the selected application has no registered handler for this file")?;
    let extension=wide(&format!(".{extension}"));
    let handlers=unsafe {SHAssocEnumHandlers(PCWSTR(extension.as_ptr()),ASSOC_FILTER_NONE)}?;
    // Open With's handler preserves the selected app and its package identity.
    // Never use the default association as a substitute for a dock target.
    for _ in 0..256 {
        let mut next=[None];let mut fetched=0;
        unsafe {handlers.Next(&mut next,Some(&mut fetched))}?;
        if fetched==0 {break;}
        let Some(handler)=next[0].take() else {break;};
        let id=handler.cast::<IObjectWithAppUserModelID>().ok()
            .and_then(|object|unsafe {object.GetAppID().and_then(take_string).ok()});
        let name=unsafe {handler.GetName().and_then(take_string)}.ok();
        if id.as_deref().is_some_and(|id|id.eq_ignore_ascii_case(app_id))
            || name.as_deref().is_some_and(|name|name.eq_ignore_ascii_case(app_id)) {return Ok(handler);}
    }
    Err("the selected packaged application has no registered handler for this file type".into())
}

fn shell_path(path:&str) -> Result<std::borrow::Cow<'_,str>> {
    let Some(tail)=path.strip_prefix(r"\\?\") else {return Ok(path.into());};
    // std::fs::canonicalize returns verbatim paths. Shell items reject that
    // namespace even when normal Win32 file APIs accept the same existing file.
    let normalized=if let Some(unc)=tail.strip_prefix(r"UNC\") {format!(r"\\{unc}")}
        else if tail.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) && tail.as_bytes().get(1..3)==Some(b":\\") {tail.to_owned()}
        else {return Err("the Windows shell cannot represent this device namespace".into());};
    if normalized.split('\\').any(|part|part.ends_with(['.',' '])) {
        return Err("the Windows shell cannot preserve a verbatim path with dot or space suffixes".into());
    }
    Ok(normalized.into())
}
fn file_item(path:&str) -> Result<IShellItem> {
    let path=wide(&shell_path(path)?);
    unsafe {SHCreateItemFromParsingName(PCWSTR(path.as_ptr()),None)}
        .map_err(|e|format!("Windows shell file item: {e}").into())
}

fn file_array(items:&[IShellItem]) -> Result<IShellItemArray> {
    struct IdList(*mut Common::ITEMIDLIST);
    impl Drop for IdList {
        fn drop(&mut self) { unsafe {CoTaskMemFree(Some(self.0.cast()));} }
    }
    let ids=items.iter().map(|item|unsafe {SHGetIDListFromObject(item).map(IdList)})
        .collect::<windows::core::Result<Vec<_>>>()?;
    let pointers=ids.iter().map(|id|id.0 as *const _).collect::<Vec<_>>();
    unsafe {SHCreateShellItemArrayFromIDLists(&pointers)}
        .map_err(|e|format!("Windows shell file array: {e}").into())
}

fn read(path:&Path) -> Result<Vec<Program>> {
    let mut bytes=Vec::new();
    match std::fs::File::open(path) {
        Ok(file)=>{ file.take(MAX_FILE+1).read_to_end(&mut bytes)?; },
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(Vec::new()),
        Err(e)=>return Err(e.into()),
    }
    if bytes.len() as u64>MAX_FILE { return Err("dock configuration is too large".into()); }
    let saved:Saved=serde_json::from_slice(&bytes)?;
    let mut keys=HashSet::new();
    if saved.version!=1 || saved.pins.len()>pleamar::scene::DOCK_ITEMS
        || saved.pins.iter().any(|p|!p.valid() || !keys.insert(p.key())) { return Err("invalid dock configuration".into()); }
    Ok(saved.pins)
}
fn write(path:&Path,pins:&[Program]) -> Result<()> {
    let bytes=serde_json::to_vec_pretty(&Saved {version:1,pins:pins.to_vec()})?;
    if bytes.len() as u64>MAX_FILE { return Err("dock configuration is too large".into()); }
    let temp=path.with_extension(format!("{}.new",std::process::id()));
    let mut created=false;
    let result=(|| -> Result<()> {
        let mut file=std::fs::OpenOptions::new().create_new(true).write(true).open(&temp)?;
        created=true;
        file.write_all(&bytes)?;file.sync_all()?;drop(file);
        let from=wide(&temp.to_string_lossy());let to=wide(&path.to_string_lossy());
        unsafe { MoveFileExW(PCWSTR(from.as_ptr()),PCWSTR(to.as_ptr()),MOVEFILE_REPLACE_EXISTING|MOVEFILE_WRITE_THROUGH) }?;
        Ok(())
    })();
    if created && result.is_err() { let _=std::fs::remove_file(&temp); }
    result
}

#[cfg(test)]
#[path = "windows_dock_package_tests.rs"]
mod package_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_file_array_keeps_every_file_identity_and_order() -> Result<()> {
        let _apartment=Apartment::new()?;
        let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let directory=std::env::temp_dir().join(format!("pleamar file selection {}-{nonce}",std::process::id()));
        std::fs::create_dir(&directory)?;
        let files=[directory.join("owned ñ 海 ' $HOME.txt"),directory.join("second $(exit 9).txt")];
        let result=(|| -> Result<()> {
            for file in &files {std::fs::write(file,"owned shell selection")?;}
            let items=files.iter().map(|file|file_item(&file.to_string_lossy())).collect::<Result<Vec<_>>>()?;
            let selection=file_array(&items)?;
            drop(items);
            assert_eq!(unsafe {selection.GetCount()}?,2);
            for (index,file) in files.iter().enumerate() {
                let displayed=unsafe {selection.GetItemAt(index as u32)?.GetDisplayName(SIGDN_FILESYSPATH).and_then(take_string)}?;
                assert_eq!(std::fs::canonicalize(displayed)?,std::fs::canonicalize(file)?);
            }
            Ok(())
        })();
        for file in files {if file.exists() {std::fs::remove_file(file)?;}}
        std::fs::remove_dir(directory)?;
        result
    }
    #[test]
    fn canonical_file_paths_resolve_to_real_shell_items_without_aliasing() -> Result<()> {
        assert_eq!(shell_path(r"\\?\C:\folder\a.txt")?,r"C:\folder\a.txt");
        assert_eq!(shell_path(r"\\?\UNC\server\share\a.txt")?,r"\\server\share\a.txt");
        for bad in [r"\\?\GLOBALROOT\Device\test",r"\\?\C:\folder.\a.txt",r"\\?\C:\folder\..\a.txt",r"\\?\C:\name "] {
            assert!(shell_path(bad).is_err(),"{bad}");
        }
        let _apartment=Apartment::new()?;
        let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let file=std::env::temp_dir().join(format!("pleamar shell ñ 海 ' {}-{nonce}.txt",std::process::id()));
        std::fs::write(&file,"owned shell item test")?;
        let canonical=std::fs::canonicalize(&file)?;
        let result=file_item(&canonical.to_string_lossy()).and_then(|item|unsafe {
            item.GetDisplayName(SIGDN_FILESYSPATH).and_then(take_string).map_err(Into::into)
        }).and_then(|displayed|std::fs::canonicalize(displayed).map_err(Into::into));
        std::fs::remove_file(&file)?;
        assert_eq!(result?,canonical);
        Ok(())
    }
    #[test]
    fn file_handler_never_substitutes_the_default_application() -> Result<()> {
        let _apartment=Apartment::new()?;
        assert!(file_handler("Pleamar.DoesNotExist!Missing",r"C:\owned.txt").is_err());
        assert!(file_handler("Pleamar.DoesNotExist!Missing",r"C:\without-extension").is_err());
        Ok(())
    }
    unsafe extern "system" fn fixture(hwnd:HWND,message:u32,w:WPARAM,l:LPARAM) -> LRESULT {
        unsafe { DefWindowProcW(hwnd,message,w,l) }
    }
    #[test]
    #[ignore = "resolves metadata from one owned non-activating window on an explicit non-primary monitor"]
    fn native_dock_metadata_on_secondary_monitor() -> Result<()> {
        use super::super::tests::{OwnWindows,ThreadDpi};
        let requested=std::env::var("PLEAMAR_WM_TEST_MONITOR")?;
        let previous=unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        assert!(!previous.0.is_null());let _dpi=ThreadDpi(previous);
        let screen=select_monitor(&requested)?;assert!(!screen.primary,"the dock fixture needs a non-primary monitor");
        let foreground=unsafe { GetForegroundWindow() };
        let module=unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }?;
        let class=WNDCLASSW {lpfnWndProc:Some(fixture),hInstance:module.into(),lpszClassName:w!("pleamar-owned-dock-metadata"),..Default::default()};
        assert_ne!(unsafe { RegisterClassW(&class) },0);
        let hwnd=unsafe { CreateWindowExW(WS_EX_APPWINDOW,class.lpszClassName,w!("Owned dock metadata ñ"),WS_OVERLAPPEDWINDOW,
            screen.work.x+60,screen.work.y+60,440,240,None,None,Some(module.into()),None) }?;
        let _owned=OwnWindows(vec![hwnd]);unsafe { let _=ShowWindow(hwnd,SW_SHOWNOACTIVATE); }pump();
        let window=inspect(hwnd).ok_or("owned dock fixture is not visible")?;
        assert!(screen.work.contains(&window.bounds));
        let resolved=program(&window)?;
        assert_eq!(resolved.key(),Target::Executable(std::env::current_exe()?.to_string_lossy().into()).key());
        assert!(!resolved.name.is_empty());assert!(resolved.target.icon().starts_with("windows-file:"));
        assert_eq!(unsafe { GetForegroundWindow() },foreground,"metadata changed foreground ownership");
        println!("{}",json!({"passed":true,"monitor":screen.name,"key":resolved.key(),"name":resolved.name,"foreground_changed":false,"input_injected":false,"dock_rendering_tested":false}));
        Ok(())
    }
    #[test]
    fn pinned_programs_survive_restart_and_concurrent_edit_without_clobbering() -> Result<()> {
        let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let directory=std::env::temp_dir().join(format!("pleamar-dock-{}-{nonce}",std::process::id()));
        std::fs::create_dir(&directory)?;let path=directory.join("pins.json");
        let mut first=Dock::new(path.clone())?;let mut second=Dock::new(path.clone())?;
        let a=Program {target:Target::Executable("C:\\ñ name\\one.exe".into()),name:"One ñ".into()};
        let b=Program {target:Target::Application("App_family!Main".into()),name:"Second".into()};
        first.remember(a.clone());second.remember(b.clone());
        first.pin(&a.key(),true)?;second.pin(&b.key(),true)?;
        assert_eq!(read(&path)?,vec![a.clone(),b.clone()]);
        first.pin(&a.key(),false)?;assert_eq!(read(&path)?,vec![b.clone()]);
        assert!(second.refresh()?);assert_eq!(second.pins,vec![b]);
        let original=std::fs::read(&path)?;
        let lock=std::fs::OpenOptions::new().write(true).share_mode(0).open(path.with_extension("lock"))?;
        assert!(first.pin(&a.key(),true).is_err());drop(lock);assert_eq!(std::fs::read(&path)?,original);
        std::fs::write(&path,b"invalid")?;assert!(first.pin(&a.key(),true).is_err());assert_eq!(std::fs::read(&path)?,b"invalid");
        for file in [path.clone(),path.with_extension("lock")] { std::fs::remove_file(file)?; }
        std::fs::remove_dir(directory)?;Ok(())
    }
    #[test]
    fn native_executable_receives_file_names_without_shell_evaluation() -> Result<()> {
        use std::os::windows::process::CommandExt;
        // Production prepares an explicitly breakaway-capable job before
        // opening applications. Isolate that process-wide setup from other tests.
        let output=std::process::Command::new(std::env::current_exe()?)
            .args(["--ignored","--exact","windows_backend::dock::tests::owned_dock_argument_runtime","--nocapture"])
            .env("PLEAMAR_DOCK_ARGUMENT_RUNTIME","1")
            .creation_flags(CREATE_NO_WINDOW.0).output()?;
        assert!(output.status.success(),"{}\n{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        Ok(())
    }
    #[test]
    #[ignore = "owned argument fixture with the real runtime job; no windows or input"]
    fn owned_dock_argument_runtime() -> Result<()> {
        use windows::Win32::System::JobObjects::*;
        assert_eq!(std::env::var("PLEAMAR_DOCK_ARGUMENT_RUNTIME").as_deref(),Ok("1"));
        // Keep the runtime-style job until this isolated subprocess exits.
        // Closing it inside the test would terminate the test process itself.
        let job=unsafe {CreateJobObjectW(None,PCWSTR::null())}?;
        let mut limits=JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags=JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE|JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        unsafe {
            SetInformationJobObject(job,JobObjectExtendedLimitInformation,&limits as *const _ as _,size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32)?;
            AssignProcessToJobObject(job,GetCurrentProcess())?;
        }
        let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let directory=std::env::temp_dir().join(format!("pleamar dock ñ ' {}-{nonce}",std::process::id()));
        std::fs::create_dir(&directory)?;
        let script=directory.join("arguments.vbs");let report=directory.join("result.txt");
        let code=format!("Set f = CreateObject(\"Scripting.FileSystemObject\").CreateTextFile(\"{}\", True, True)\r\nFor Each a In WScript.Arguments\r\nf.WriteLine a\r\nNext\r\nf.Close\r\n",report.to_string_lossy().replace('"',"\"\""));
        // Windows Script Host reads Unicode script files with a UTF-16 BOM.
        let bytes:Vec<u8>=std::iter::once(0xfeff).chain(code.encode_utf16()).flat_map(u16::to_le_bytes).collect();
        std::fs::write(&script,bytes)?;
        let executable=PathBuf::from(std::env::var_os("SystemRoot").ok_or("SystemRoot unavailable")?).join("System32/cscript.exe");
        let program=Program {target:Target::Executable(executable.to_string_lossy().into()),name:"Owned argument fixture".into()};
        let mut dock=Dock::new(directory.join("pins.json"))?;dock.remember(program.clone());
        let expected=vec![directory.join("it's ñ 海.txt").to_string_lossy().into_owned(),directory.join("$HOME; $(exit 9).txt").to_string_lossy().into_owned()];
        let mut files=vec![script.to_string_lossy().into_owned()];files.extend(expected.clone());
        let activations=Activations::new(wait::Wake::new()?)?;
        dock.open(&program.key(),&files,&activations)?;
        let deadline=Instant::now()+Duration::from_secs(20);
        let received=loop {
            if let Ok(data)=std::fs::read(&report) {
                let words:Vec<_>=data.chunks_exact(2).map(|b|u16::from_le_bytes([b[0],b[1]])).collect();
                let text=String::from_utf16_lossy(&words);
                let lines:Vec<String>=text.trim_start_matches('\u{feff}').lines().map(str::to_owned).collect();
                if lines.len()==expected.len() { break lines; }
            }
            assert!(Instant::now()<deadline,"native dock launch did not deliver its file arguments");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(received,expected);
        for file in [script,report] {
            loop {
                match std::fs::remove_file(&file) {
                    Ok(())=>break,
                    Err(error) if (error.kind()==std::io::ErrorKind::PermissionDenied || matches!(error.raw_os_error(),Some(32|33))) && Instant::now()<deadline=>
                        std::thread::sleep(Duration::from_millis(10)),
                    Err(error)=>return Err(error.into()),
                }
            }
        }
        std::fs::remove_dir(directory)?;Ok(())
    }
}
