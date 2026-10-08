"""Native cross-process fullscreen and crash recovery, using only owned windows."""
from pathlib import Path
import argparse, ctypes as C, hashlib, json, os, subprocess, time
from ctypes import wintypes as W

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary',type=Path,required=True)
parser.add_argument('--fixture',type=Path,required=True)
parser.add_argument('--monitor',required=True)
parser.add_argument('--output',type=Path,required=True)
parser.add_argument('--ci-owned-desktop',action='store_true')
args=parser.parse_args()
if args.ci_owned_desktop:
    assert all(os.environ.get(k)==v for k,v in [('GITHUB_ACTIONS','true'),('RUNNER_ENVIRONMENT','github-hosted'),('PLEAMAR_WM_CI_DOCK','1')]), 'Primary output is restricted to disposable CI'
binary=args.binary.resolve(strict=True);fixture=args.fixture.resolve(strict=True)
output=args.output.resolve();assert not output.exists(), 'Preserve earlier evidence'
flags=subprocess.CREATE_NO_WINDOW|subprocess.BELOW_NORMAL_PRIORITY_CLASS
env=dict(os.environ,PLEAMAR_WM_NAMESPACE='fullscreen-'+str(os.getpid()),PLEAMAR_CONFIG=str(output/'config'),
    PLEAMAR_DOCK_FIXTURE_MONITOR=args.monitor,PLEAMAR_DOCK_FIXTURE_ROOT=str(output))

def command(*words):
    p=subprocess.run([str(binary),*words],env=env,capture_output=True,encoding='utf-8',creationflags=flags,timeout=8)
    if p.returncode:raise RuntimeError(p.stderr+p.stdout)
    return json.loads(p.stdout)

screen=next((s for s in command('monitors') if s['name']==args.monitor and (args.ci_owned_desktop or not s['primary'])),None)
assert screen is not None, 'An explicit secondary monitor is required'
output.mkdir();rules=output/'empty.conf';rules.write_text('',encoding='utf-8')
user=C.WinDLL('user32',use_last_error=True)
user.GetForegroundWindow.restype=W.HWND
user.GetWindowThreadProcessId.argtypes=[W.HWND,C.POINTER(W.DWORD)]
user.GetWindowLongPtrW.argtypes=[W.HWND,C.c_int];user.GetWindowLongPtrW.restype=C.c_ssize_t
user.SetProcessDpiAwarenessContext.argtypes=[W.HANDLE];user.SetProcessDpiAwarenessContext.restype=W.BOOL
assert user.SetProcessDpiAwarenessContext(W.HANDLE(-4))
app=None;session=None;owned=set();identity=None
report=dict(passed=False,monitor=screen,os_input=False,installed_product_changed=False,stages=[],
    binaries={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in [binary,fixture]})

def guard():
    pid=W.DWORD();user.GetWindowThreadProcessId(user.GetForegroundWindow(),C.byref(pid))
    assert pid.value not in owned, 'Owned process took foreground'
    if identity:
        for w in command('windows'):
            if w['id']!=identity:continue
            b=w['bounds'];m=screen['bounds']
            assert m['x']<=b['x'] and m['y']<=b['y'] and b['x']+b['width']<=m['x']+m['width'] and b['y']+b['height']<=m['y']+m['height']

def wait(check,label):
    end=time.monotonic()+12;last=None
    while time.monotonic()<end:
        guard()
        try:
            last=check()
            if last:return last
        except (RuntimeError,subprocess.TimeoutExpired) as error:last=str(error)
        time.sleep(.05)
    raise RuntimeError(f'{label}: {last}')

def status(line='status'):return command('--say','wm',line)
def window():return next(w for w in command('windows') if w['id']==identity)
def styles():return [user.GetWindowLongPtrW(hwnd,key) for key in [-16,-20]]
def start_session():
    with (output/'session.log').open('ab') as log:
        p=subprocess.Popen([str(binary),'session','--monitor',args.monitor,'--process',str(app.pid),'--owner',str(os.getpid()),
            '--state',str(output/'recovery.json'),'--rules',str(rules),'--seconds','60'],env=env,stdout=log,stderr=subprocess.STDOUT,creationflags=flags)
    owned.add(p.pid)
    return p

try:
    with (output/'fixture.log').open('wb') as log:
        app=subprocess.Popen([str(fixture)],env=env,stdout=log,stderr=subprocess.STDOUT,creationflags=flags)
    owned.add(app.pid)
    original=wait(lambda:next((w for w in command('windows') if w['process']==app.pid),None),'owned fixture')
    identity=original['id'];hwnd=int(identity.split(':')[2],16);before=styles()
    session=start_session();wait(lambda:status().get('running'),'native session')
    assert status('fullscreen '+identity)['fullscreen_windows']==[identity]
    assert window()['bounds']==screen['bounds'];assert styles()!=before;guard()
    assert status('fullscreen '+identity)['fullscreen_windows']==[]
    assert window()['bounds']==original['bounds'] and styles()==before
    report['stages'].append('cross-process-fullscreen-and-frame-restoration')
    status('fullscreen '+identity);assert window()['bounds']==screen['bounds']
    session.kill();session.wait(timeout=5)
    assert window()['bounds']==screen['bounds']
    session=start_session();wait(lambda:status().get('running'),'replacement session')
    assert window()['bounds']==original['bounds'] and styles()==before
    assert status()['fullscreen_windows']==[] and status()['saved_windows']==0
    report['stages'].append('forced-daemon-exit-recovers-journal-and-frame')
    status('fullscreen '+identity);status('quit');session.wait(timeout=5)
    assert window()['bounds']==original['bounds'] and styles()==before;guard()
    report['stages'].append('normal-session-exit-restores-foreign-owned-window')
    report.update(passed=True,foreground_owned_at_checks=False)
finally:
    if session and session.poll() is None:
        try:status('quit');session.wait(timeout=5)
        except Exception:session.kill();session.wait(timeout=5)
    if app and app.poll() is None:app.kill();app.wait(timeout=5)
    report['remaining_owned_windows']=[w['id'] for w in command('windows') if w['process'] in owned]
    (output/'report.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
assert not report['remaining_owned_windows']
print(json.dumps(report,indent=2))
