"""Native minimize/restore commands on owned disposable CI windows only."""
from pathlib import Path
import argparse
import ctypes as C
from ctypes import wintypes as W
import hashlib
import importlib.util
import json
import os
import struct
import subprocess
import sys
import time


def main():
    if (sys.platform != 'win32' or os.environ.get('GITHUB_ACTIONS') != 'true'
            or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'
            or os.environ.get('PLEAMAR_WM_CI_SHORTCUTS') != '1'):
        raise RuntimeError('Window shortcuts require the explicit step on a disposable GitHub-hosted Windows runner')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--tests', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    folder = args.output.resolve()
    if folder.parent != Path(os.environ['RUNNER_TEMP']).resolve() or folder.exists():
        raise RuntimeError('Evidence requires a new direct child of RUNNER_TEMP')
    binary, tests = args.binary.resolve(strict=True), args.tests.resolve(strict=True)
    spec = importlib.util.spec_from_file_location('owned_input', Path(__file__).with_name('windows-agent-input.py'))
    owned = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(owned)
    desktop = owned.Desktop()
    for name, result, parameters in [
        ('IsIconic', W.BOOL, [W.HWND]), ('IsZoomed', W.BOOL, [W.HWND]),
        ('ShowWindow', W.BOOL, [W.HWND, C.c_int]),
        ('AllowSetForegroundWindow', W.BOOL, [W.DWORD]),
    ]:
        function = getattr(desktop.user, name)
        function.restype, function.argtypes = result, parameters
    folder.mkdir()
    env = dict(os.environ, PLEAMAR_WM_NAMESPACE=f'shortcuts-ci-{os.getpid()}', PLEAMAR_CI_DESKTOP_INPUT='1')
    report = dict(passed=False, environment='github-hosted', physical_input=False,
                  only_owned_targets=True, whole_product_acceptance=False, commands=[], images=[])
    processes, fixtures, logs = [], [], []
    def save():
        (folder / 'report.json').write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding='utf-8')
    def spawn(command, name, environment=env):
        log = (folder / (name + '.log')).open('x', encoding='utf-8')
        logs.append(log)
        child = subprocess.Popen(list(map(str, command)), env=environment, stdout=log, stderr=subprocess.STDOUT,
                                 creationflags=subprocess.CREATE_NO_WINDOW | subprocess.BELOW_NORMAL_PRIORITY_CLASS)
        processes.append(child)
        return child
    def fixture(name):
        path = folder.with_name(folder.name + '-' + name)
        path.mkdir()
        child = spawn([tests, '--ignored', '--exact', 'platform::windows_desktop::ci_input_tests::owned_input_fixture', '--nocapture'],
                      name, dict(env, PLEAMAR_OWNED_INPUT_FIXTURE=str(path), PLEAMAR_INPUT_TEST_PARENT=str(os.getpid())))
        fixtures.append((child, path))
        owned.wait(lambda: owned.read(path / 'state.json').get('ready'), 'owned fixture')
        return child, desktop.owned(child), path
    def control(path, command):
        if owned.read(path / 'state.json').get('command') == command:
            control(path, 'checkpoint')
        (path / 'control.pending').write_text(command, encoding='utf-8')
        (path / 'control.pending').replace(path / 'control')
        owned.wait(lambda: owned.read(path / 'state.json').get('command') == command, command)
    def run(*arguments, fail=None):
        result = subprocess.run([str(binary), *map(str, arguments)], env=env, capture_output=True,
                                text=True, encoding='utf-8', timeout=18, creationflags=subprocess.CREATE_NO_WINDOW)
        report['commands'].append(dict(args=list(arguments), exit=result.returncode, stdout=result.stdout, stderr=result.stderr))
        save()
        if fail:
            assert result.returncode != 0 and fail in result.stderr, result.stderr
            return None
        assert result.returncode == 0, result.stderr
        if arguments[:2] == ('agent', 'look'): return result.stdout
        return json.loads(result.stdout)
    def wm(command, fail=None): return run('--say', 'wm', command, fail=fail)
    def capture(identity, name):
        path = folder / (name + '.png')
        run('agent', 'look', identity, str(path))
        png = path.read_bytes()
        assert png[:8] == b'\x89PNG\r\n\x1a\n'
        width, height = struct.unpack('>II', png[16:24])
        report['images'].append(dict(file=path.name, width=width, height=height, sha256=hashlib.sha256(png).hexdigest()))
        save()
    try:
        target, hwnd, target_path = fixture('target')
        outside, other_hwnd, outside_path = fixture('outside')
        window = next(w for w in run('windows') if w['process'] == target.pid)
        identity, monitor = window['id'], window['monitor']
        rules = folder / 'empty rules ñ.conf'
        rules.write_text('', encoding='utf-8')
        service = spawn([binary, 'session', '--monitor', monitor, '--process', target.pid, '--owner', os.getpid(),
                         '--rules', rules, '--state', folder / 'session ñ.json', '--seconds', '90'], 'session')
        def ready():
            assert service.poll() is None
            result = subprocess.run([str(binary), '--say', 'wm', 'status'], env=env, capture_output=True,
                                    text=True, encoding='utf-8', timeout=18, creationflags=subprocess.CREATE_NO_WINDOW)
            return result.returncode == 0
        owned.wait(ready, 'session')
        assert wm('status')['last_minimized'] is None
        wm('emit restore_last', fail='no recently minimized window')
        assert desktop.user.SetForegroundWindow(other_hwnd), 'owned outside bootstrap'
        wm('emit minimize', fail='outside this WM session')
        for command in ('focus_next', 'focus_previous', 'close'):
            wm('emit ' + command, fail='outside this WM session')
            assert desktop.user.GetForegroundWindow() == other_hwnd
        assert not desktop.user.IsIconic(other_hwnd) and not desktop.user.IsIconic(hwnd)
        control(outside_path, 'allow-parent')
        assert desktop.user.SetForegroundWindow(hwnd)
        owned.wait(lambda: desktop.user.GetForegroundWindow() == hwnd, 'owned target foreground')
        capture(identity, '01-before-minimize')
        control(target_path, 'allow-parent')
        assert desktop.user.AllowSetForegroundWindow(service.pid)
        for command in ('focus_next', 'focus_previous'):
            assert wm('emit ' + command)['focused_window'] == identity
            assert desktop.user.GetForegroundWindow() == hwnd
        status = wm('emit minimize')
        assert status['last_minimized'] == identity and desktop.user.IsIconic(hwnd)
        assert desktop.user.GetForegroundWindow() != hwnd
        wm('emit restore_last')
        owned.wait(lambda: not desktop.user.IsIconic(hwnd), 'normal restore')
        assert not desktop.user.IsZoomed(hwnd)
        capture(identity, '02-restored-normal')
        assert wm('status')['last_minimized'] is None
        # The title-bar/native minimize route must update the same session history.
        desktop.user.ShowWindow(hwnd, 3)  # SW_SHOWMAXIMIZED
        owned.wait(lambda: desktop.user.IsZoomed(hwnd), 'maximized state')
        desktop.user.ShowWindow(hwnd, 6)  # SW_MINIMIZE
        owned.wait(lambda: wm('status')['last_minimized'] == identity, 'actual minimize event')
        wm('emit restore_last')
        owned.wait(lambda: not desktop.user.IsIconic(hwnd) and desktop.user.IsZoomed(hwnd), 'restore to maximized')
        capture(identity, '03-restored-maximized')
        desktop.user.ShowWindow(hwnd, 4)  # SW_SHOWNOACTIVATE
        owned.wait(lambda: not desktop.user.IsZoomed(hwnd), 'normal geometry')
        control(target_path, 'allow-parent')
        assert desktop.user.SetForegroundWindow(hwnd)
        free_bounds = next(w['bounds'] for w in run('windows') if w['id'] == identity)
        wm('layout ' + monitor + ' grid')
        status = wm('emit minimize')
        assert desktop.user.IsIconic(hwnd) and status['last_minimized'] == identity
        assert status['saved_windows'] == 1, 'minimizing lost the free-position journal'
        assert status['monitors'][0]['tiled'] and status['monitors'][0]['windows'] == 0
        status = wm('emit restore_last')
        assert not desktop.user.IsIconic(hwnd) and not desktop.user.IsZoomed(hwnd)
        assert status['last_minimized'] is None and status['monitors'][0]['windows'] == 1
        assert status['monitors'][0]['tiled'] and status['saved_windows'] == 1
        capture(identity, '04-restored-into-layout')
        wm('free ' + monitor)
        assert next(w['bounds'] for w in run('windows') if w['id'] == identity) == free_bounds
        assert wm('status')['saved_windows'] == 0
        desktop.user.ShowWindow(other_hwnd, 6)
        time.sleep(.15)
        assert wm('status')['last_minimized'] is None, 'out-of-scope minimize entered history'
        desktop.user.ShowWindow(hwnd, 6)
        owned.wait(lambda: wm('status')['last_minimized'] == identity, 'last minimize before close')
        control(target_path, 'quit')
        assert target.wait(timeout=5) == 0
        owned.wait(lambda: wm('status')['last_minimized'] is None, 'destroyed identity retirement')
        wm('emit restore_last', fail='no recently minimized window')
        assert desktop.user.IsIconic(other_hwnd), 'restore substituted an unrelated window'
        wm('quit')
        assert service.wait(timeout=5) == 0
        report.update(passed=True, checks=['scoped foreground minimize', 'normal and maximized restoration',
                      'actual minimize/destroy events', 'tiled minimize/rejoin and free-position recovery', 'out-of-scope refusal',
                      'no unrelated restore after target exit'], physical_key_dispatch=False)
    finally:
        for child, path in fixtures:
            if child.poll() is None: (path / 'control').write_text('quit', encoding='utf-8')
        for child in reversed(processes):
            try: child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill(); child.wait(timeout=5)
        for log in logs: log.close()
        save()
    print(json.dumps(dict(passed=True, output=str(folder))))


if __name__ == '__main__':
    main()
