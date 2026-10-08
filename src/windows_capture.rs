//! Persistent WGC sessions share one D3D11 device. A consumer requests a frame
//! only after the preceding frame has been consumed; the capture pool is bounded.
use pleamar::windows_texture::{SharedDevice, SharedTexture};
use std::{cell::Cell, marker::PhantomData, rc::Rc, time::{Duration,Instant}, sync::{Arc, atomic::{AtomicBool, Ordering}}};
use windows::{core::{Interface, Result}, Foundation::TypedEventHandler,
    Graphics::{Capture::*, DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat}, SizeInt32},
    Win32::{Foundation::*, Graphics::{Direct3D::*, Direct3D11::*, Dxgi::{IDXGIDevice, Common::*}},
        System::WinRT::{*, Direct3D11::*, Graphics::Capture::IGraphicsCaptureItemInterop}}};

fn keep_capture_code(factory: &IGraphicsCaptureSessionStatics) -> Result<()> {
    use std::sync::OnceLock;
    use windows::{core::{HRESULT, PCWSTR}, Win32::{Foundation::HMODULE, System::LibraryLoader::*}};
    static PINNED: OnceLock<std::result::Result<(), HRESULT>> = OnceLock::new();
    // Windows 11 can return from Close while internal WGC callbacks still use
    // GraphicsCapture.dll. Retiring the last MTA worker then unloads their code
    // (0xc0000005 in GraphicsCapture.dll_unloaded). Keep only that loaded module
    // until process exit, not its factories, COM apartments or GPU resources.
    // Resolve by the activated factory's code address, never a DLL search path.
    let result = PINNED.get_or_init(|| unsafe {
        let mut module = HMODULE::default();
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
            PCWSTR(factory.vtable().IsSupported as *const () as *const u16), &mut module).map_err(|e| e.code())
    });
    (*result).map_err(Into::into)
}

const MAX_PIXELS: u64 = 16_777_216;
const FRAME_BUFFERS: i32 = 1;

// Keep the renderer descriptor, but allocate capture resources only on demand.
// A short grace period avoids recreating D3D11 during overview page transitions.
pub(super) struct DeviceCache {
    active: Option<Rc<Device>>,
    wake: Option<Arc<super::wait::Wake>>,
    renderer: Option<SharedDevice>,
    retire_at: Option<Instant>,
}
impl DeviceCache {
    pub fn new(wake:Option<Arc<super::wait::Wake>>) -> Self {
        Self {active:None,wake,renderer:None,retire_at:None}
    }
    pub fn get(&mut self) -> Result<Rc<Device>> {
        self.retire_at=None;
        if let Some(device)=&self.active { return Ok(device.clone()); }
        let device=match Device::create(self.wake.clone(),self.renderer.clone()) {
            Ok(device) => device,
            Err(error) if self.renderer.is_some() => {
                eprintln!("windows preview: shared capture unavailable; trying CPU readback: {error}");
                Device::new(self.wake.clone())?
            },
            Err(error) => return Err(error),
        };
        eprintln!("windows preview: capture transport = {}",if device.shared() {"shared GPU textures"} else {"CPU readback"});
        self.active=Some(device.clone());
        Ok(device)
    }
    // Return whether existing captures must be replaced. On an allocation
    // failure the live device and its matching renderer remain unchanged.
    pub fn renderer(&mut self, shared:Option<SharedDevice>) -> Result<bool> {
        let replace=match &self.active {
            Some(device) => shared.is_some() || device.shared(),
            None => false,
        };
        if replace { self.active=Some(Device::create(self.wake.clone(),shared.clone())?); }
        self.renderer=shared;
        Ok(replace)
    }
    pub fn idle(&mut self, idle:bool, now:Instant) {
        if !idle || self.active.is_none() { self.retire_at=None;return; }
        let deadline=*self.retire_at.get_or_insert(now+Duration::from_secs(2));
        if now>=deadline {
            self.active=None;self.retire_at=None;
            eprintln!("windows preview: idle capture device retired");
        }
    }
    pub fn wait(&self, now:Instant) -> Option<Duration> {
        self.retire_at.map(|at|at.saturating_duration_since(now))
    }
}

struct Apartment(PhantomData<Rc<()>>);
impl Drop for Apartment { fn drop(&mut self) { unsafe { RoUninitialize() }; } }

pub(super) struct Device {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    runtime: IDirect3DDevice,
    wake: Option<Arc<super::wait::Wake>>,
    shared: Option<SharedDevice>,
    fence: Option<ID3D11Fence>,
    context4: Option<ID3D11DeviceContext4>,
    sequence: Cell<u64>,
    _apartment: Apartment,
}
impl Device {
    pub fn new(wake:Option<Arc<super::wait::Wake>>) -> Result<Rc<Self>> { Self::create(wake,None) }
    pub fn shared(&self) -> bool { self.shared.is_some() }
    fn create(wake:Option<Arc<super::wait::Wake>>, shared:Option<SharedDevice>) -> Result<Rc<Self>> { unsafe {
        RoInitialize(RO_INIT_MULTITHREADED)?;
        let apartment = Apartment(PhantomData);
        // Avoid the generated global factory cache surviving its COM apartment.
        let factory = windows::core::factory::<GraphicsCaptureSession, IGraphicsCaptureSessionStatics>()?;
        keep_capture_code(&factory)?;
        let mut supported = false;
        (factory.vtable().IsSupported)(factory.as_raw(), &mut supported).ok()?;
        if !supported { return Err(E_NOTIMPL.into()); }
        let (mut device, mut context) = (None, None);
        let adapter = shared.as_ref().map(|shared| {
            let factory:windows::Win32::Graphics::Dxgi::IDXGIFactory4 = windows::Win32::Graphics::Dxgi::CreateDXGIFactory1()?;
            factory.EnumAdapterByLuid::<windows::Win32::Graphics::Dxgi::IDXGIAdapter>(shared.adapter())
        }).transpose()?;
        D3D11CreateDevice(adapter.as_ref(), if adapter.is_some() { D3D_DRIVER_TYPE_UNKNOWN } else { D3D_DRIVER_TYPE_HARDWARE }, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
        let device = device.ok_or(E_POINTER)?;
        let context = context.ok_or(E_POINTER)?;
        let _ = context.cast::<ID3D11Multithread>()?.SetMultithreadProtected(true);
        let runtime = CreateDirect3D11DeviceFromDXGIDevice(&device.cast::<IDXGIDevice>()?)?.cast()?;
        let (mut fence,mut context4) = (None,None);
        if shared.is_some() {
            device.cast::<ID3D11Device5>()?.CreateFence(0,D3D11_FENCE_FLAG_NONE,&mut fence)?;
            context4=Some(context.cast()?);
        }
        Ok(Rc::new(Self { device, context, runtime, wake, shared, fence, context4, sequence:Cell::new(0), _apartment:apartment }))
    } }
}

fn bounded(size:SizeInt32) -> Result<(u32,u32)> {
    if size.Width < 1 || size.Height < 1 || size.Width > 8192 || size.Height > 8192
        || size.Width as u64 * size.Height as u64 > MAX_PIXELS { return Err(E_INVALIDARG.into()); }
    Ok((size.Width as u32,size.Height as u32))
}

pub(super) struct Picture { pub size:(u32,u32), pub pixels:Vec<u8>, pub shared:Option<Arc<SharedTexture>> }
struct SharedBuffer { image:Arc<SharedTexture>, texture:ID3D11Texture2D }
struct Frame(Direct3D11CaptureFrame);
impl Drop for Frame { fn drop(&mut self) { let _ = self.0.Close(); } }
struct Mapped<'a>(&'a ID3D11DeviceContext,&'a ID3D11Texture2D);
impl Drop for Mapped<'_> { fn drop(&mut self) { unsafe { self.0.Unmap(self.1,0) }; } }

pub(super) struct Capture {
    // Drop WinRT objects before the device/apartment, including on failure.
    item: GraphicsCaptureItem,
    session: GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    closed_token: i64,
    frame_token: i64,
    closed: Arc<AtomicBool>,
    dirty: Arc<AtomicBool>,
    pending: bool,
    pending_since: Instant,
    staging: Option<ID3D11Texture2D>,
    shared: Vec<SharedBuffer>,
    gpu_pending: Option<(usize,u64)>,
    waiting_buffer: bool,
    cpu_fallback: bool,
    size: SizeInt32,
    device: Rc<Device>,
}
impl Capture {
    pub fn new(device:Rc<Device>, hwnd:HWND, budget:u64) -> Result<Self> { unsafe {
        let interop:IGraphicsCaptureItemInterop = windows::core::factory::<GraphicsCaptureItem,_>()?;
        let item:GraphicsCaptureItem = interop.CreateForWindow(hwnd)?;
        let size = item.Size()?;
        bounded(size)?;
        if size.Width as u64 * size.Height as u64 > budget {
            return Err(windows::core::Error::new(E_OUTOFMEMORY,"aggregate window capture budget exceeded"));
        }
        Self::from_item(device,item,size)
    } }
    fn from_item(device:Rc<Device>,item:GraphicsCaptureItem,size:SizeInt32) -> Result<Self> { unsafe {
        bounded(size)?;
        let factory = windows::core::factory::<Direct3D11CaptureFramePool,IDirect3D11CaptureFramePoolStatics2>()?;
        let mut raw = std::ptr::null_mut();
        (factory.vtable().CreateFreeThreaded)(factory.as_raw(), device.runtime.as_raw(),
            DirectXPixelFormat::B8G8R8A8UIntNormalized, FRAME_BUFFERS, size, &mut raw).ok()?;
        if raw.is_null() { return Err(E_POINTER.into()); }
        let pool = Direct3D11CaptureFramePool::from_raw(raw);
        let session = match pool.CreateCaptureSession(&item) {
            Ok(session) => session,
            Err(error) => { let _ = pool.Close(); return Err(error); }
        };
        let mut capture = Self { item, session, pool, closed_token:0, frame_token:0,
            closed:Arc::new(AtomicBool::new(false)), dirty:Arc::new(AtomicBool::new(true)),
            pending:false, pending_since:Instant::now(), staging:None, shared:Vec::new(), gpu_pending:None, waiting_buffer:false, cpu_fallback:false, size, device };
        let closed = capture.closed.clone();
        let wake = capture.device.wake.clone();
        capture.closed_token = capture.item.Closed(&TypedEventHandler::new(move |_,_| {
            closed.store(true,Ordering::Release); if let Some(wake)=&wake { wake.signal(); } Ok(())
        }))?;
        let dirty = capture.dirty.clone();
        let wake = capture.device.wake.clone();
        capture.frame_token = capture.pool.FrameArrived(&TypedEventHandler::new(move |_,_| {
            dirty.store(true,Ordering::Release); if let Some(wake)=&wake { wake.signal(); } Ok(())
        }))?;
        capture.session.SetIsCursorCaptureEnabled(false)?;
        capture.session.StartCapture()?;
        Ok(capture)
    } }

    pub fn size(&self) -> Result<(u32,u32)> { bounded(self.size) }
    pub fn closed(&self) -> bool { self.closed.load(Ordering::Acquire) }
    pub fn pending(&self) -> bool { self.pending || self.gpu_pending.is_some() || self.waiting_buffer }
    pub fn ready(&self) -> bool { self.pending() || self.closed() || self.dirty.load(Ordering::Acquire) }

    fn acquire(&mut self, budget:u64) -> Result<Option<Frame>> {
        if !self.dirty.swap(false,Ordering::AcqRel) { return Ok(None); }
            let frame = match self.pool.TryGetNextFrame() {
                Ok(frame) => Frame(frame),
                Err(e) if e.code() == E_POINTER || e.code() == S_OK => return Ok(None),
                Err(e) => return Err(e),
            };
            let content = frame.0.ContentSize()?;
            bounded(content)?;
            if content.Width as u64 * content.Height as u64 > budget {
                return Err(windows::core::Error::new(E_OUTOFMEMORY,"aggregate window capture budget exceeded"));
            }
            if content != self.size {
                #[cfg(test)] eprintln!("capture resize: {:?} -> {:?}",self.size,content);
                drop(frame);
                self.staging = None;
                // Recreate can discard the only update from a static source.
                // A fresh pool/session requests the complete resized picture;
                // reuse the capture item, never reopen a possibly recycled HWND.
                self.session.Close()?;
                *self=Self::from_item(self.device.clone(),self.item.clone(),content)?;
                return Ok(None);
            }
        Ok(Some(frame))
    }

    fn next_shared(&mut self, budget:u64) -> Result<Option<Picture>> { unsafe {
        if self.closed() { return Err(RO_E_CLOSED.into()); }
        let size=bounded(self.size)?;
        if size.0 as u64 * size.1 as u64 > budget { return Err(E_OUTOFMEMORY.into()); }
        if self.gpu_pending.is_none() {
            // Two immutable images: the displayed image and the next capture.
            // GPU submissions also hold an Arc; a CPU acknowledgement alone is
            // never permission to overwrite a texture still being copied.
            let index = if let Some(i)=self.shared.iter().position(|b|Arc::strong_count(&b.image)==1) { i }
            else if self.shared.len()<2 {
                let image=self.device.shared.as_ref().ok_or(E_POINTER)?.texture(size)?;
                let texture=image.open(&self.device.device)?;
                self.shared.push(SharedBuffer {image,texture});self.shared.len()-1
            } else { self.waiting_buffer=true;return Ok(None); };
            self.waiting_buffer=false;
            let Some(frame)=self.acquire(budget)? else { return Ok(None); };
            let source:ID3D11Texture2D=frame.0.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?.GetInterface()?;
            self.device.context.CopyResource(&self.shared[index].texture,&source);
            let value=self.device.sequence.get().checked_add(1).ok_or(E_FAIL)?;
            self.device.sequence.set(value);
            self.device.context4.as_ref().ok_or(E_POINTER)?.Signal(self.device.fence.as_ref().ok_or(E_POINTER)?,value)?;
            self.device.context.Flush();
            drop(frame);
            self.gpu_pending=Some((index,value));self.pending_since=Instant::now();
        }
        let (index,value)=self.gpu_pending.ok_or(E_POINTER)?;
        let completed=self.device.fence.as_ref().ok_or(E_POINTER)?.GetCompletedValue();
        if completed==u64::MAX { return Err(windows::Win32::Graphics::Dxgi::DXGI_ERROR_DEVICE_REMOVED.into()); }
        if completed<value {
            if self.pending_since.elapsed()>Duration::from_secs(5) {
                return Err(windows::core::Error::new(E_FAIL,"window capture GPU copy timed out"));
            }
            return Ok(None);
        }
        self.gpu_pending=None;
        Ok(Some(Picture {size, pixels:Vec::new(), shared:Some(self.shared[index].image.clone())}))
    } }

    /// No blocking readback: a busy GPU is retried on the next consumer tick.
    /// One WGC buffer, one staging texture and one CPU result per window.
    pub fn next(&mut self, budget:u64) -> Result<Option<Picture>> { unsafe {
        if self.device.shared() && !self.cpu_fallback {
            match self.next_shared(budget) {
                Ok(picture) => return Ok(picture),
                Err(error) => {
                    eprintln!("windows capture: GPU sharing failed, using CPU readback: {error}");
                    self.shared.clear();self.gpu_pending=None;self.waiting_buffer=false;
                    self.cpu_fallback=true;self.dirty.store(true,Ordering::Release);
                },
            }
        }
        if self.closed() { return Err(RO_E_CLOSED.into()); }
        if self.size.Width as u64 * self.size.Height as u64 > budget {
            return Err(windows::core::Error::new(E_OUTOFMEMORY,"aggregate window capture budget exceeded"));
        }
        if !self.pending {
            let Some(frame) = self.acquire(budget)? else { return Ok(None); };
            let (width,height) = bounded(self.size)?;
            if self.staging.is_none() {
                self.device.device.CreateTexture2D(&D3D11_TEXTURE2D_DESC {
                    Width:width, Height:height, MipLevels:1, ArraySize:1,
                    Format:DXGI_FORMAT_B8G8R8A8_UNORM, SampleDesc:DXGI_SAMPLE_DESC {Count:1,Quality:0},
                    Usage:D3D11_USAGE_STAGING, CPUAccessFlags:D3D11_CPU_ACCESS_READ.0 as u32,
                    ..Default::default()
                },None,Some(&mut self.staging))?;
            }
            let source:ID3D11Texture2D = frame.0.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?.GetInterface()?;
            self.device.context.CopyResource(self.staging.as_ref().ok_or(E_POINTER)?,&source);
            self.device.context.Flush();
            // The submitted GPU copy owns its resources. Return the WGC buffer
            // now: holding it during CPU readback can lose a static repaint.
            drop(frame);
            self.pending = true;
            self.pending_since = Instant::now();
        }
        let texture = self.staging.as_ref().ok_or(E_POINTER)?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        match self.device.context.Map(texture,0,D3D11_MAP_READ,D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,Some(&mut mapped)) {
            Ok(()) => {},
            Err(e) if e.code() == windows::Win32::Graphics::Dxgi::DXGI_ERROR_WAS_STILL_DRAWING => {
                if self.pending_since.elapsed()>Duration::from_secs(5) {
                    return Err(windows::core::Error::new(E_FAIL,"window capture GPU readback timed out"));
                }
                return Ok(None);
            },
            Err(e) => return Err(e),
        }
        let mapping = Mapped(&self.device.context,texture);
        let (width,height) = bounded(self.size)?;
        let stride = width as usize * 4;
        if mapped.pData.is_null() || (mapped.RowPitch as usize) < stride { return Err(E_FAIL.into()); }
        let mut pixels = vec![0u8;stride * height as usize];
        for y in 0..height as usize {
            let row = std::slice::from_raw_parts(mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),stride);
            pixels[y*stride..(y+1)*stride].copy_from_slice(row);
        }
        drop(mapping);
        self.pending = false;
        Ok(Some(Picture { size:(width,height),pixels,shared:None }))
    } }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        if self.frame_token != 0 { let _ = self.pool.RemoveFrameArrived(self.frame_token); }
        if self.closed_token != 0 { let _ = self.item.RemoveClosed(self.closed_token); }
        let _ = self.pool.Close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "creates and retires native D3D11 capture devices; no windows, pixels or input"]
    fn native_capture_device_idle_retirement() -> Result<()> {
        use windows::Win32::System::{ProcessStatus::*, Threading::GetCurrentProcess};
        fn memory() -> Result<usize> {
            let mut counters=PROCESS_MEMORY_COUNTERS_EX::default();
            unsafe { GetProcessMemoryInfo(GetCurrentProcess(),(&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
                std::mem::size_of_val(&counters) as u32)?; }
            Ok(counters.PrivateUsage)
        }
        let baseline=memory()?;
        let mut cache=DeviceCache::new(None);
        assert!(!cache.renderer(None)?);
        assert!(cache.active.is_none());
        assert!(cache.wait(Instant::now()).is_none());
        let lazy=memory()?;
        let mut samples=Vec::new();
        for _ in 0..8 {
            let device=cache.get()?;
            let weak=Rc::downgrade(&device);
            assert!(Rc::ptr_eq(&device,&cache.get()?));
            let allocated=memory()?;
            drop(device);
            let now=Instant::now();
            cache.idle(true,now);
            cache.idle(true,now+Duration::from_secs(1));
            assert!(weak.upgrade().is_some());
            cache.idle(false,now+Duration::from_secs(1));
            assert!(cache.wait(now).is_none());
            cache.idle(true,now+Duration::from_secs(2));
            cache.idle(true,now+Duration::from_secs(4));
            assert!(weak.upgrade().is_none(),"idle cache retained its D3D11 device");
            assert!(cache.wait(now).is_none());
            std::thread::sleep(Duration::from_millis(200));
            samples.push(serde_json::json!({"allocated_private_bytes":allocated,"retired_private_bytes":memory()?}));
        }
        // Capture/render owners may still hold their own references. Retiring
        // the cache must not invalidate them while another page reopens.
        let old=cache.get()?;
        let now=Instant::now();cache.idle(true,now);cache.idle(true,now+Duration::from_secs(2));
        assert!(cache.active.is_none());
        let fresh=cache.get()?;
        assert!(!Rc::ptr_eq(&old,&fresh));
        assert!(old.runtime.cast::<IDirect3DDevice>().is_ok());
        drop(old);drop(fresh);drop(cache);
        println!("{}",serde_json::json!({"native_device_cycles":8,"windows_created":0,"physical_input":false,
            "baseline_private_bytes":baseline,"lazy_private_bytes":lazy,"samples":samples}));
        Ok(())
    }
}
