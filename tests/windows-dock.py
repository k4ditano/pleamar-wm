from pathlib import Path
import ctypes as C
from ctypes import wintypes as W
import hashlib, json, os, struct, subprocess, time, zlib

import argparse

parser = argparse.ArgumentParser(description="Exercise the native dock on an explicit non-primary monitor, without injecting input.")
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--fixture', type=Path, required=True)
parser.add_argument('--monitor', required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--scene-file', type=Path, help='Marea application dock scene, exercised with its actual Luau companion')
parser.add_argument('--ci-owned-desktop', action='store_true', help='Allow the disposable CI desktop; refused on local machines')
parser.add_argument('--idle-seconds', type=float, default=0, help='Sample the owned scene CPU and memory after settling (0 or 1..30 seconds)')
parser.add_argument('--ci-ole-source', type=Path, help='Owned engine test binary; requires the explicit disposable CI step')
options = parser.parse_args()
assert options.idle_seconds == 0 or 1 <= options.idle_seconds <= 30
ci_drop=options.ci_ole_source is not None
if ci_drop and (os.environ.get('GITHUB_ACTIONS')!='true' or os.environ.get('RUNNER_ENVIRONMENT')!='github-hosted' or os.environ.get('PLEAMAR_WM_CI_DOCK_DROP')!='1'):
    raise RuntimeError('OS file dragging is restricted to the disposable GitHub-hosted test')
if options.ci_owned_desktop and (os.environ.get('GITHUB_ACTIONS')!='true' or os.environ.get('RUNNER_ENVIRONMENT')!='github-hosted' or os.environ.get('PLEAMAR_WM_CI_DOCK')!='1'):
    raise RuntimeError('The primary desktop is restricted to the disposable GitHub-hosted test')
repo = Path(__file__).resolve().parents[1]
binary = options.binary.resolve(strict=True)
fixture = options.fixture.resolve(strict=True)
output = options.output.resolve()
assert not output.exists(), 'Use a fresh output directory; preserve previous evidence'
assert binary.is_file() and fixture.is_file()
flags = subprocess.CREATE_NO_WINDOW | subprocess.BELOW_NORMAL_PRIORITY_CLASS
screens = json.loads(subprocess.check_output([str(binary), 'monitors'], creationflags=flags, encoding='utf-8'))
matching = [s for s in screens if s['name'] == options.monitor and (ci_drop or options.ci_owned_desktop or not s['primary'])]
assert len(matching) == 1, 'The explicit non-primary monitor is unavailable'
screen = matching[0]
output.mkdir()
env = os.environ.copy()
env.update(PLEAMAR_CONFIG=str(output / 'config'), APPDATA=str(output / 'appdata'), LOCALAPPDATA=str(output / 'localappdata'),
           PLEAMAR_SOCKET_DIR=f'native-dock-acceptance-{os.getpid()}', PLEAMAR_NO_RELAUNCH='1',
           PLEAMAR_TEST_WINDOWS='1', PLEAMAR_DOCK_FIXTURE_MONITOR=screen['name'], PLEAMAR_DOCK_FIXTURE_ROOT=str(output))
if options.scene_file:
    original = options.scene_file.resolve(strict=True)
    assert original.name == 'windows-dock.plm'
    source = original.read_text(encoding='utf-8')
    assert source.count('keyboard: on_demand') == 1
    source = source.replace('keyboard: on_demand', 'keyboard: none')
    scene_file = output / original.name
    scene_file.with_suffix('.luau').write_bytes(original.with_suffix('.luau').read_bytes())
    env.update(MAREA_DOCK_MONITOR=screen['name'], MAREA_LOCALE='es', PATH=str(binary.parent)+os.pathsep+env.get('PATH',''))
else:
    source = (repo / 'examples/windows-dock.plm').read_text(encoding='utf-8')
    surface = 'surface { size: 900, 420; kind: window; title: "pleamar · native dock" }'
    assert source.count(surface) == 1
    source = source.replace(surface, 'surface { size: 900, 420; keyboard: none; anchor: center; reserve: 0; rate: 30 }')
    scene_file = output / 'native-dock.plm'
scene_file.write_text(source, encoding='utf-8')
subprocess.run([str(binary), '--check', str(scene_file)], check=True, env=env, creationflags=flags, capture_output=True)

user = C.WinDLL('user32', use_last_error=True)
gdi = C.WinDLL('gdi32', use_last_error=True)
callback = C.WINFUNCTYPE(W.BOOL, W.HWND, W.LPARAM)
for lib, name, result, args in [
    (user,'EnumWindows',W.BOOL,[callback,W.LPARAM]), (user,'GetWindowThreadProcessId',W.DWORD,[W.HWND,C.POINTER(W.DWORD)]),
    (user,'GetWindowTextW',C.c_int,[W.HWND,W.LPWSTR,C.c_int]), (user,'IsWindowVisible',W.BOOL,[W.HWND]), (user,'GetWindowRect',W.BOOL,[W.HWND,C.POINTER(W.RECT)]),
    (user,'GetForegroundWindow',W.HWND,[]), (user,'SetProcessDpiAwarenessContext',W.BOOL,[W.HANDLE]),
    (user,'PostMessageW',W.BOOL,[W.HWND,W.UINT,W.WPARAM,W.LPARAM]), (user,'GetDC',W.HDC,[W.HWND]),
    (user,'ReleaseDC',C.c_int,[W.HWND,W.HDC]), (gdi,'CreateCompatibleDC',W.HDC,[W.HDC]),
    (gdi,'CreateCompatibleBitmap',W.HBITMAP,[W.HDC,C.c_int,C.c_int]), (gdi,'SelectObject',W.HANDLE,[W.HDC,W.HANDLE]),
    (gdi,'BitBlt',W.BOOL,[W.HDC,C.c_int,C.c_int,C.c_int,C.c_int,W.HDC,C.c_int,C.c_int,W.DWORD]),
    (gdi,'DeleteDC',W.BOOL,[W.HDC]), (gdi,'DeleteObject',W.BOOL,[W.HANDLE]),
    (gdi,'GetDIBits',C.c_int,[W.HDC,W.HBITMAP,W.UINT,W.UINT,C.c_void_p,C.c_void_p,W.UINT])]:
    fn = getattr(lib,name); fn.restype=result; fn.argtypes=args
assert user.SetProcessDpiAwarenessContext(W.HANDLE(-4))
class Header(C.Structure):
    _fields_=[('size',W.DWORD),('width',W.LONG),('height',W.LONG),('planes',W.WORD),('bits',W.WORD),
              ('compression',W.DWORD),('image_size',W.DWORD),('xppm',W.LONG),('yppm',W.LONG),('used',W.DWORD),('important',W.DWORD)]
class Info(C.Structure):
    _fields_=[('header',Header),('colors',W.DWORD*3)]
owned_pids=set()
images=[]
commands=[]
scene=None
initial=None
report=dict(passed=False,monitor=screen,physical_input=ci_drop,manual_input=False,installed_product_changed=False,stages=[],images=images,
            binaries={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in [binary,fixture]})

def windows(pid):
    found=[]
    @callback
    def visit(hwnd,_):
        process=W.DWORD();user.GetWindowThreadProcessId(hwnd,C.byref(process))
        if process.value==pid and user.IsWindowVisible(hwnd): found.append(hwnd)
        return True
    assert user.EnumWindows(visit,0)
    return found

def canvases(pid):
    result=[]
    for hwnd in windows(pid):
        title=C.create_unicode_buffer(512);user.GetWindowTextW(hwnd,title,len(title))
        if not title.value.startswith('pleamar input'):result.append(hwnd)
    return result

def guard():
    foreground=user.GetForegroundWindow();pid=W.DWORD();user.GetWindowThreadProcessId(foreground,C.byref(pid))
    assert pid.value not in owned_pids, 'An owned test window unexpectedly took foreground'
    for process in owned_pids:
        for hwnd in windows(process):
            box=W.RECT();assert user.GetWindowRect(hwnd,C.byref(box))
            # The transparent dock surface includes the taskbar margin; its
            # visible bar and input zones are above that margin.
            work=screen['bounds'] if options.scene_file and scene is not None and process==scene.pid else screen['work']
            assert work['x']<=box.left<box.right<=work['x']+work['width']
            assert work['y']<=box.top<box.bottom<=work['y']+work['height']

def wait(predicate, label, seconds=20):
    until=time.monotonic()+seconds;last=None
    while time.monotonic()<until:
        guard()
        try:
            last=predicate()
            if last:return last
        except (RuntimeError,subprocess.TimeoutExpired) as error:last=str(error)
        time.sleep(.06)
    raise RuntimeError(f'{label}: {last}')

def ask(line):
    result=subprocess.run([str(binary),'--say',scene_file.stem,line],env=env,creationflags=flags,capture_output=True,encoding='utf-8',timeout=5)
    if result.returncode:raise RuntimeError(result.stderr)
    answer=result.stdout.strip()
    if answer.startswith('?'):raise RuntimeError(answer)
    return answer

def press(zone):
    result=ask('press '+zone);assert 'pressed' in result,result
    commands.append({'zone':zone,'result':result});guard()

def start_scene():
    global scene
    log=(output/f'scene-{len(report["stages"])}.log').open('wb')
    scene=subprocess.Popen([str(binary),'--scene',str(scene_file),'--screen',screen['name'],'--preview-monitor',screen['name'],
                            '--window-actions','--no-hud','--stall','0','--seconds','90'],env=env,creationflags=flags,stdout=log,stderr=subprocess.STDOUT)
    log.close();owned_pids.add(scene.pid)
    wait(lambda:ask('get win.docks.0'),'scene endpoint')
    wait(lambda:len(canvases(scene.pid))==1,'scene window')
    if options.scene_file:wait(lambda:ask('get dock_ready') in ('1','true'),'native dock work area')

def stop_scene():
    global scene
    if scene is not None and scene.poll() is None:
        try:ask('quit');scene.wait(timeout=8)
        except Exception:scene.kill();scene.wait(timeout=5)
    scene=None

def fixture_index():
    count=int(float(ask('get win.docks.0')))
    matches=[i for i in range(min(count,12)) if 'windows-dock-fixture' in ask(f'get win.dock.0.{i}.name').lower()]
    return matches[0]+1 if len(matches)==1 else 0

def launches():
    result=[]
    for path in output.glob('launch-*.json'):
        try:value=json.loads(path.read_text(encoding='utf-8'))
        except json.JSONDecodeError:continue
        assert value['monitor']==screen['name'];owned_pids.add(value['pid']);result.append(value)
    return result

def close_fixture(entry):
    pid=W.DWORD();user.GetWindowThreadProcessId(entry['hwnd'],C.byref(pid));assert pid.value==entry['pid']
    assert user.PostMessageW(entry['hwnd'],0x0010,0,0)
    wait(lambda:not windows(entry['pid']),'owned fixture close')

def capture(name):
    guard();hwnd=canvases(scene.pid)[0];box=W.RECT();assert user.GetWindowRect(hwnd,C.byref(box))
    width,height=box.right-box.left,box.bottom-box.top
    dc=user.GetDC(None);memory=gdi.CreateCompatibleDC(dc);bitmap=gdi.CreateCompatibleBitmap(dc,width,height);previous=gdi.SelectObject(memory,bitmap)
    try:
        assert gdi.BitBlt(memory,0,0,width,height,dc,box.left,box.top,0x00CC0020|0x40000000)
        gdi.SelectObject(memory,previous);previous=None
        info=Info();info.header=Header(C.sizeof(Header),width,-height,1,32,0,width*height*4,0,0,0,0)
        data=C.create_string_buffer(width*height*4)
        assert gdi.GetDIBits(memory,bitmap,0,height,data,C.byref(info),0)==height
        raw=data.raw;rgb=bytearray(width*height*3)
        rgb[0::3]=raw[2::4];rgb[1::3]=raw[1::4];rgb[2::3]=raw[0::4]
        def chunk(kind,payload):
            return struct.pack('>I',len(payload))+kind+payload+struct.pack('>I',zlib.crc32(kind+payload)&0xffffffff)
        rows=b''.join(b'\0'+rgb[y*width*3:(y+1)*width*3] for y in range(height))
        png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',width,height,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(rows))+chunk(b'IEND',b'')
        path=output/(name+'.png');path.write_bytes(png)
        images.append(dict(file=path.name,sha256=hashlib.sha256(path.read_bytes()).hexdigest(),width=width,height=height))
    finally:
        if previous:gdi.SelectObject(memory,previous)
        gdi.DeleteObject(bitmap);gdi.DeleteDC(memory);user.ReleaseDC(None,dc)

def idle_sample():
    kernel=C.WinDLL('kernel32',use_last_error=True)
    psapi=C.WinDLL('psapi',use_last_error=True)
    class Memory(C.Structure):
        _fields_=[('cb',W.DWORD),('faults',W.DWORD)]+[(name,C.c_size_t) for name in
            ['peak_working','working','peak_paged','paged','peak_nonpaged','nonpaged','pagefile','peak_pagefile','private']]
    kernel.OpenProcess.argtypes=[W.DWORD,W.BOOL,W.DWORD];kernel.OpenProcess.restype=W.HANDLE
    kernel.CloseHandle.argtypes=[W.HANDLE];kernel.CloseHandle.restype=W.BOOL
    kernel.GetProcessTimes.argtypes=[W.HANDLE]+[C.POINTER(W.FILETIME)]*4;kernel.GetProcessTimes.restype=W.BOOL
    psapi.GetProcessMemoryInfo.argtypes=[W.HANDLE,C.POINTER(Memory),W.DWORD];psapi.GetProcessMemoryInfo.restype=W.BOOL
    process=kernel.OpenProcess(0x410,False,scene.pid);assert process
    def read():
        times=[W.FILETIME() for _ in range(4)]
        assert kernel.GetProcessTimes(process,*[C.byref(t) for t in times])
        memory=Memory();memory.cb=C.sizeof(memory)
        assert psapi.GetProcessMemoryInfo(process,C.byref(memory),C.sizeof(memory))
        return dict(cpu=sum(t.dwLowDateTime+(t.dwHighDateTime<<32) for t in times[2:])/10_000_000,
                    private_bytes=memory.private,working_bytes=memory.working)
    try:
        time.sleep(2);guard();before=read();start=time.monotonic()
        while time.monotonic()-start < options.idle_seconds:
            guard();time.sleep(.1)
        elapsed=time.monotonic()-start;after=read()
        report['idle_sample']=dict(seconds=elapsed,before=before,after=after,
            one_core_cpu_percent=100*(after['cpu']-before['cpu'])/elapsed)
    finally:kernel.CloseHandle(process)

try:
    with (output/'fixture-start.log').open('wb') as log:
        initial=subprocess.Popen([str(fixture)],env=env,creationflags=flags,stdout=log,stderr=subprocess.STDOUT)
    owned_pids.add(initial.pid)
    def first_launch():
        value=next((x for x in launches() if x['pid']==initial.pid),None)
        assert value is not None or initial.poll() is None, f'Owned fixture exited ({initial.returncode}); see fixture-start.log'
        return value
    first=wait(first_launch,'initial fixture')
    start_scene();index=wait(fixture_index,'dock metadata')-1
    time.sleep(.7);capture('01-populated')
    if options.idle_seconds:idle_sample()
    if options.scene_file:
        press(f'open.{index} right');capture('00-pin-menu')
    press(f'keep.{index}')
    pins=output/'config/wm/windows-dock.json'
    wait(lambda:pins.exists() and len(json.loads(pins.read_text())['pins'])==1,'persisted pin')
    wait(lambda:ask('get win.dock.0.0.pinned') in ('1','true'),'renderer pinned state');capture('02-pinned')
    close_fixture(first);initial.wait(timeout=5)
    wait(lambda:ask('get win.dock.0.0.windows')=='0','closed source retained as a pin');capture('03-pinned-closed')
    report['stages'].append('pin-survives-window-close')
    stop_scene();start_scene();wait(lambda:ask('get win.dock.0.0.pinned') in ('1','true'),'pin survives scene restart');capture('04-restarted')
    assert ask('get win.dock.0.0.windows')=='0'
    press('open.0')
    second=wait(lambda:next((x for x in launches() if x['pid']!=initial.pid),None),'real relaunch')
    wait(lambda:ask('get win.dock.0.0.windows')=='1','relaunched native window');time.sleep(.4);capture('05-relaunched')
    assert second['args']==[];report['stages'].append('pin-restarts-actual-native-program')
    stop_scene()
    assert windows(second['pid']), 'Closing the dock killed its application'
    start_scene();wait(lambda:ask('get win.dock.0.0.windows')=='1','application survives dock restart')
    report['stages'].append('application-survives-dock-close-and-restart')
    active=second
    if ci_drop:
        close_fixture(second)
        wait(lambda:ask('get win.dock.0.0.windows')=='0','closed app ready for file drop')
        files=[output/"owned ñ 海 ' #1.txt",output/'$HOME; $(exit 9).txt']
        for path in files:path.write_text('Owned OLE test file',encoding='utf-8')
        box=W.RECT();assert user.GetWindowRect(canvases(scene.pid)[0],C.byref(box))
        request=dict(pid=scene.pid,x=box.left+round(70*screen['scale']),y=box.top+round(135*screen['scale']),files=[str(p) for p in files])
        drop_env=dict(env,PLEAMAR_DOCK_OLE_REQUEST=json.dumps(request))
        helper=options.ci_ole_source.resolve(strict=True)
        with (output/'ole-source.log').open('w',encoding='utf-8') as log:
            result=subprocess.run([str(helper),'--ignored','--exact','platform::windows::drag::ole_tests::native_ole_file_source','--nocapture','--test-threads=1'],
                                  env=drop_env,creationflags=flags,stdout=log,stderr=subprocess.STDOUT,timeout=20)
        assert result.returncode==0,'The OS OLE source did not complete; see ole-source.log'
        active=wait(lambda:next((x for x in launches() if x['pid'] not in [first['pid'],second['pid']]),None),'file-drop application launch')
        assert active['args']==[str(p) for p in files],active
        wait(lambda:ask('get win.dock.0.0.windows')=='1','file-drop native window')
        capture('07-file-drop');report['stages'].append('actual-ole-drop-delivers-both-file-paths')
        report['ole_source']=dict(binary=str(helper),sha256=hashlib.sha256(helper.read_bytes()).hexdigest(),request=request)
    if options.scene_file:
        press('open.0 right');capture('08-unpin-menu')
    press('unkeep.0');wait(lambda:json.loads(pins.read_text())['pins']==[],'persistent unpin');capture('06-unpinned')
    close_fixture(active);report['stages'].append('unpin-updates-storage-and-ui');guard()
    if options.scene_file:
        # Its Luau quit can close the pipe before the press reply is written.
        try:press('leave')
        except RuntimeError as error:report['hide_reply']=str(error)
        assert scene.wait(timeout=8)==0, 'The hide action did not exit the owned dock cleanly'
        report['stages'].append('marea-dock-hide-exits-through-luau')
        report['marea_source']={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in [original,original.with_suffix('.luau')]}
    report.update(passed=True,foreground_owned_at_checks=False,commands=commands,launches=launches(),packaged_apps_tested=False,os_drag_drop_tested=ci_drop)
finally:
    stop_scene()
    for entry in launches():
        if windows(entry['pid']):close_fixture(entry)
    if initial is not None and initial.poll() is None:initial.kill();initial.wait(timeout=5)
    report['remaining_owned_windows']=[pid for pid in owned_pids if windows(pid)]
    (output/'report.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
print(json.dumps(report,indent=2))
