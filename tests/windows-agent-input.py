"""Exercise the production WM CLI against owned Win32 controls on disposable CI."""
from pathlib import Path
import argparse
import ctypes as C
from ctypes import wintypes as W
import hashlib
import json
import os
import struct
import subprocess
import sys
import time


def printed_pixels(path):
    result = subprocess.run(['powershell.exe', '-NoLogo', '-NoProfile', '-NonInteractive',
                             '-File', str(Path(__file__).with_name('windows-capture-pixels.ps1')),
                             '-Path', str(path)], capture_output=True, text=True, encoding='utf-8', errors='replace',
                            timeout=10, creationflags=subprocess.CREATE_NO_WINDOW)
    assert result.returncode == 0, result.stdout + result.stderr
    return json.loads(result.stdout)


def require_ci():
    if (sys.platform != 'win32' or os.environ.get('GITHUB_ACTIONS') != 'true'
            or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'
            or os.environ.get('PLEAMAR_WM_CI_AGENT_INPUT') != '1'):
        raise RuntimeError('Native input requires the explicit step on a disposable GitHub-hosted Windows runner')


def wait(predicate, label, seconds=8):
    start = time.monotonic()
    while time.monotonic() - start < seconds:
        value = predicate()
        if value:
            return value
        time.sleep(.025)
    raise AssertionError('Timed out: ' + label)


def read(path):
    try:
        return json.loads(path.read_text(encoding='utf-8'))
    except (FileNotFoundError, json.JSONDecodeError, PermissionError):
        return {}


class Desktop:
    def __init__(self):
        self.user = C.WinDLL('user32', use_last_error=True)
        self.dwm = C.WinDLL('dwmapi', use_last_error=True)
        self.callback = C.WINFUNCTYPE(W.BOOL, W.HWND, W.LPARAM)
        signatures = [
            (self.user, 'SetProcessDpiAwarenessContext', W.BOOL, [W.HANDLE]),
            (self.user, 'EnumWindows', W.BOOL, [self.callback, W.LPARAM]),
            (self.user, 'GetWindowThreadProcessId', W.DWORD, [W.HWND, C.POINTER(W.DWORD)]),
            (self.user, 'GetWindowTextW', C.c_int, [W.HWND, W.LPWSTR, C.c_int]),
            (self.user, 'GetForegroundWindow', W.HWND, []),
            (self.user, 'SetForegroundWindow', W.BOOL, [W.HWND]),
            (self.user, 'GetWindowRect', W.BOOL, [W.HWND, C.POINTER(W.RECT)]),
            (self.user, 'SetWindowPos', W.BOOL, [W.HWND, W.HWND, C.c_int, C.c_int, C.c_int, C.c_int, W.UINT]),
            (self.user, 'GetCursorPos', W.BOOL, [C.POINTER(W.POINT)]),
            (self.user, 'GetAsyncKeyState', C.c_short, [C.c_int]),
            (self.dwm, 'DwmGetWindowAttribute', C.c_long, [W.HWND, W.DWORD, C.c_void_p, W.DWORD]),
        ]
        for library, name, result, args in signatures:
            function = getattr(library, name)
            function.restype, function.argtypes = result, args
        assert self.user.SetProcessDpiAwarenessContext(W.HANDLE(-4)), C.get_last_error()

    def pid(self, hwnd):
        result = W.DWORD()
        assert self.user.GetWindowThreadProcessId(hwnd, C.byref(result))
        return result.value

    def owned(self, process):
        found = []

        @self.callback
        def visit(hwnd, _):
            title = C.create_unicode_buffer(256)
            self.user.GetWindowTextW(hwnd, title, len(title))
            if self.pid(hwnd) == process.pid and title.value.startswith(f'Pleamar input fixture {process.pid} '):
                found.append(hwnd)
            return True

        assert process.poll() is None
        assert self.user.EnumWindows(visit, 0)
        assert len(found) == 1, found
        return found[0]

    def bounds(self, hwnd, visible=True):
        rect = W.RECT()
        if visible:
            assert self.dwm.DwmGetWindowAttribute(hwnd, 9, C.byref(rect), C.sizeof(rect)) == 0
        else:
            assert self.user.GetWindowRect(hwnd, C.byref(rect))
        return [rect.left, rect.top, rect.right, rect.bottom]

    def idle(self):
        assert all(self.user.GetAsyncKeyState(key) >= 0 for key in [16, 17, 18, 91, 92, 1, 2, 4, 5, 6, 27]), 'held keys/buttons or Escape'


def exercise(binary, tests, folder):
    desktop = Desktop()
    env = dict(os.environ, PLEAMAR_WM_NAMESPACE=f'input-ci-{os.getpid()}', PLEAMAR_CI_DESKTOP_INPUT='1')
    report = dict(passed=False, environment='github-hosted', only_owned_targets=True,
                  physical_input_sent=True, whole_product_acceptance=False, physical_mixed_dpi=False,
                  binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), commands=[], images=[])
    processes, logs, fixtures = [], [], []
    started = time.monotonic()

    def save():
        (folder / 'report.json').write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding='utf-8')

    def spawn(args, name, environment=env):
        log = (folder / f'{name}.log').open('x', encoding='utf-8')
        logs.append(log)
        process = subprocess.Popen([str(a) for a in args], env=environment, stdout=log, stderr=subprocess.STDOUT,
                                   creationflags=subprocess.CREATE_NO_WINDOW | subprocess.BELOW_NORMAL_PRIORITY_CLASS)
        processes.append(process)
        return process

    def run(*args, fail=None, stdin=None):
        desktop.idle()
        result = subprocess.run([str(binary), 'agent', *map(str, args)], env=env, input=stdin,
                                capture_output=True, text=True, encoding='utf-8', errors='strict', timeout=18,
                                creationflags=subprocess.CREATE_NO_WINDOW)
        report['commands'].append(dict(args=list(args), exit=result.returncode, stdout=result.stdout, stderr=result.stderr))
        save()
        if fail:
            assert result.returncode != 0 and fail in result.stderr, result.stderr
            return None
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)

    def read_only_picture(label, selector=None):
        selector = selector or identity
        path = folder / (label + '.png')
        foreground = desktop.user.GetForegroundWindow()
        cursor = W.POINT()
        assert desktop.user.GetCursorPos(C.byref(cursor))
        result = subprocess.run([str(binary), 'agent', 'look', selector, str(path)], env=env,
                                capture_output=True, text=True, encoding='utf-8', timeout=18,
                                creationflags=subprocess.CREATE_NO_WINDOW)
        report['commands'].append(dict(args=['look', selector, str(path)], exit=result.returncode,
                                       stdout=result.stdout, stderr=result.stderr))
        save()
        assert result.returncode == 0, result.stderr
        data = path.read_bytes()
        assert data[:8] == b'\x89PNG\r\n\x1a\n'
        width, height = struct.unpack('>II', data[16:24])
        assert result.stdout.strip() == f'{path} {width}x{height}'
        report.setdefault('printed_pixels', {})[path.name] = printed_pixels(path)
        report['images'].append(dict(file=path.name, width=width, height=height,
                                    capture_method='read-only', sha256=hashlib.sha256(data).hexdigest()))
        assert desktop.user.GetForegroundWindow() == foreground
        after = W.POINT()
        assert desktop.user.GetCursorPos(C.byref(after)) and (after.x, after.y) == (cursor.x, cursor.y)
        run('input-status', fail='unavailable')
        run('type', selector, 'read-only must not enable input', fail='unavailable')
        report.setdefault('read_only', []).append(dict(case=label, no_broker=True, no_input=True, focus_unchanged=True, pointer_unchanged=True))
        save()
        return data

    def fixture(directory, name, dialog=False, tool=False):
        directory.mkdir()
        child_env = dict(env, PLEAMAR_OWNED_INPUT_FIXTURE=str(directory), PLEAMAR_INPUT_TEST_PARENT=str(os.getpid()),
                         PLEAMAR_INPUT_TEST_DIALOG='tool' if tool else '1' if dialog else '0')
        child = spawn([tests, '--ignored', '--exact', 'platform::windows_desktop::ci_input_tests::owned_input_fixture', '--nocapture'], name, child_env)
        fixtures.append((child, directory))
        wait(lambda: read(directory / 'state.json').get('ready'), 'owned Win32 controls')
        return child, desktop.owned(child)

    def control(directory, command):
        (directory / 'control.pending').write_text(command, encoding='utf-8')
        (directory / 'control.pending').replace(directory / 'control')
        wait(lambda: read(directory / 'state.json').get('command') == command, 'fixture command ' + command)

    def broker(monitor, pid, name, seconds=100):
        child = spawn([binary, 'agent', 'serve', '--input', 'foreground', '--monitor', monitor,
                       '--process', str(pid), '--seconds', str(seconds)], name)
        def ready():
            assert child.poll() is None, 'input broker exited before ready'
            result = subprocess.run([str(binary), 'agent', 'input-status'], env=env, capture_output=True,
                                    text=True, encoding='utf-8', timeout=18, creationflags=subprocess.CREATE_NO_WINDOW)
            if result.returncode == 0:
                return json.loads(result.stdout)
        status = wait(ready, 'input broker pipe')
        assert status['input'] == 'foreground' and not status['independent_seat'] and status['process'] == pid
        return child

    # The engine's child fixture requires a direct RUNNER_TEMP child. Keep its
    # directory separate from the report so no existing evidence is overwritten.
    target_folder = folder.with_name(folder.name + '-target')
    outside_folder = folder.with_name(folder.name + '-outside')
    try:
        target, hwnd = fixture(target_folder, 'target')
        state = lambda: read(target_folder / 'state.json')
        catalog = run('windows')
        window = next(entry['window'] for entry in catalog if entry['window']['process'] == target.pid)
        identity, monitor = window['id'], window['monitor']
        read_only_picture('read-only-normal')
        service = broker(monitor, target.pid, 'broker')
        assert desktop.pid(hwnd) == target.pid
        report.update(monitor=monitor, target_pid=target.pid, target_id=identity)
        run('type', identity, 'must not appear', fail='desktop.look must precede input')
        assert state()['text'] == ''

        def picture(label):
            assert target.poll() is None and desktop.owned(target) == hwnd
            path = folder / f'{len(report["images"]):02}-{label}.png'
            start = time.monotonic()
            while True:
                result = subprocess.run([str(binary), 'agent', 'look', identity, str(path)], env=env, capture_output=True,
                                        text=True, encoding='utf-8', timeout=18, creationflags=subprocess.CREATE_NO_WINDOW)
                report['commands'].append(dict(args=['look', identity, str(path)], exit=result.returncode,
                                               stdout=result.stdout, stderr=result.stderr))
                save()
                if result.returncode == 0:
                    data = json.loads(result.stdout)
                    break
                assert not path.exists() and 'capture and window frame coordinates differ' in result.stderr and time.monotonic() - start < 3, result.stderr
                time.sleep(.05)
            png = path.read_bytes()
            assert png[:8] == b'\x89PNG\r\n\x1a\n'
            width, height = struct.unpack('>II', png[16:24])
            assert (width, height) == (data['width'], data['height']) and data['window'] == identity
            assert data['capture_method'] in ['windows-graphics-capture', 'window-print']
            if data['capture_method'] == 'window-print':
                report.setdefault('printed_pixels', {})[path.name] = printed_pixels(path)
            report['images'].append(dict(file=path.name, width=width, height=height,
                                        capture_method=data['capture_method'], sha256=hashlib.sha256(png).hexdigest()))
            save()
            return path

        old_foreground = desktop.user.GetForegroundWindow()
        path = picture('read-only-focus')
        assert desktop.user.GetForegroundWindow() == old_foreground
        run('look', identity, str(path), fail='os error 80')
        run('type', identity, 'no unseen permit', fail='desktop.look must precede input')

        # Only the owned CI HWND is activated by this bootstrap. Every following
        # pointer/keyboard gesture goes through the production CLI and broker.
        assert desktop.user.SetForegroundWindow(hwnd), 'Windows rejected owned fixture bootstrap focus'
        wait(lambda: state().get('foreground'), 'owned target foreground')
        report['bootstrap'] = 'SetForegroundWindow on the owned target; no bootstrap keyboard or mouse injection'
        run('focus', identity)
        wait(lambda: desktop.user.GetForegroundWindow() == hwnd, 'CLI foreground acknowledgement')

        def point(x, y):
            offset = state()['client']
            return [x + offset[0], y + offset[1]]

        def act(label, operation, *args, stdin=None):
            assert desktop.pid(hwnd) == target.pid and not state()['cancelled']
            picture(label)
            run(operation, identity, *args, stdin=stdin)

        act('hover', 'move', *point(480, 320))
        cursor = W.POINT()
        assert desktop.user.GetCursorPos(C.byref(cursor))
        frame = desktop.bounds(hwnd)
        assert [cursor.x, cursor.y] == [frame[0] + point(480, 320)[0], frame[1] + point(480, 320)[1]]
        act('button', 'click', *point(390, 50))
        wait(lambda: state()['clicks'] == 1, 'native button click')
        act('edit', 'click', *point(130, 198))
        message = 'Hola, España 🎵 日本語'
        act('unicode', 'type', message)
        wait(lambda: state()['text'] == message, 'Unicode typing')
        run('key', identity, 'backspace', fail='desktop.look must precede input')
        assert state()['text'] == message
        act('backspace', 'key', 'backspace')
        wait(lambda: state()['text'] == 'Hola, España 🎵 日本', 'native Backspace')
        act('select', 'hotkey', 'ctrl+a')
        wait(lambda: state()['selected_text'] == 'Hola, España 🎵 日本', 'actual Rich Edit selection')
        replacement = 'Nuevo: café ☕'
        act('stdin', 'type', '-', stdin=replacement)
        wait(lambda: state()['text'] == replacement, 'stdin selection replacement')
        act('canvas', 'click', *point(480, 320))
        act('vertical-wheel', 'scroll', *point(480, 320), 'up', 3)
        wait(lambda: state()['wheel'] == 360, 'vertical wheel')
        act('horizontal-wheel', 'scroll', *point(480, 320), 'right', 2)
        wait(lambda: state()['horizontal_wheel'] == 240, 'horizontal wheel')
        act('drag', 'drag', *point(30, 30), *point(180, 100))
        wait(lambda: state()['drags'] == 1, 'native drag')
        assert state()['patch'] == [170, 90] and not state()['dragging']
        act('right-click', 'click', *point(480, 320), 'right')
        wait(lambda: state()['right_clicks'] == 1, 'right click')
        act('middle-click', 'click', *point(480, 320), 'middle')
        wait(lambda: state()['middle_clicks'] == 1, 'middle click')
        picture('final-controls')

        old = desktop.bounds(hwnd, False)
        assert desktop.user.SetWindowPos(hwnd, None, old[0] + 12, old[1], 0, 0, 0x0015)
        try:
            run('type', identity, 'stale geometry', fail='picture changed')
            assert state()['text'] == replacement
        finally:
            assert desktop.user.SetWindowPos(hwnd, None, old[0], old[1], 0, 0, 0x0015)
        picture('before-done')
        run('done')
        run('type', identity, 'retired permit', fail='desktop.look must precede input')
        assert state()['text'] == replacement

        outside, other_hwnd = fixture(outside_folder, 'outside')
        other_id = next(entry['window']['id'] for entry in run('windows') if entry['window']['process'] == outside.pid)
        foreground = desktop.user.GetForegroundWindow()
        run('focus', other_id, fail='process scope')
        outside_path = folder / 'outside-must-not-exist.png'
        run('look', other_id, str(outside_path), fail='process scope')
        assert not outside_path.exists() and desktop.user.GetForegroundWindow() == foreground
        assert read(outside_folder / 'state.json')['text'] == '' and desktop.pid(other_hwnd) == outside.pid
        control(outside_folder, 'quit')
        assert outside.wait(timeout=5) == 0
        report['final_controls'] = state()
        run('stop')
        assert service.wait(timeout=5) == 0
        run('input-status', fail='unavailable')

        expiring = broker(monitor, target.pid, 'lease', seconds=1)
        assert expiring.wait(timeout=5) == 0
        bound = broker(monitor, target.pid, 'process-lifetime')
        # Preserve the test parent's eligibility for the next owned fixture.
        # Once this foreground window exits, the controller cannot simply
        # assume Windows will let it activate another application's dialog.
        assert desktop.user.GetForegroundWindow() == hwnd
        control(target_folder, 'allow-parent')
        control(target_folder, 'quit')
        assert target.wait(timeout=5) == 0
        assert bound.wait(timeout=5) == 0
        run('input-status', fail='unavailable')
        # An ordinary application may open a modal dialog in the same process.
        # Select that actual HWND; the disabled owner's permit cannot migrate.
        target_folder = folder.with_name(folder.name + '-dialog')
        target, hwnd = fixture(target_folder, 'dialog', dialog=True)
        state = lambda: read(target_folder / 'state.json')
        catalog = run('windows')
        same_process = [entry['window'] for entry in catalog if entry['window']['process'] == target.pid]
        assert len(same_process) == 2
        owner_id = next(w['id'] for w in same_process if w['title'].startswith('Owned input parent '))
        window = next(w for w in same_process if w['title'].startswith('Pleamar input fixture '))
        identity, monitor = window['id'], window['monitor']
        owner_png = read_only_picture('read-only-disabled-owner', owner_id)
        modal_png = read_only_picture('read-only-modal')
        assert owner_png != modal_png, 'read-only owner capture must not redirect to its dialog'
        modal_service = broker(monitor, target.pid, 'modal-broker')
        ambiguous = folder / 'ambiguous-must-not-exist.png'
        run('look', str(target.pid), str(ambiguous), fail='process has several windows')
        assert not ambiguous.exists()
        assert desktop.user.SetForegroundWindow(hwnd), 'owned modal bootstrap focus'
        wait(lambda: state().get('foreground'), 'owned modal foreground')
        # A focus CLI command can forward eligibility to its broker. Bootstrap
        # the owned target before that transfer, then prove the owner is refused.
        run('focus', owner_id, fail='blocked by a dialog')
        assert desktop.user.GetForegroundWindow() == hwnd
        run('focus', identity)
        act('modal-button', 'click', *point(390, 50))
        wait(lambda: state()['clicks'] == 1, 'explicit modal button click')
        picture('modal-result')
        assert report['images'][-1]['capture_method'] == 'window-print', 'modal case must exercise the fallback on this runner'
        report['modal'] = dict(pid=target.pid, owner=owner_id, dialog=identity, clicks=state()['clicks'], implicit_redirection=False)
        for command, expected in [('print-hang', 'window print capture timed out'),
                                  ('protect-capture', 'the window excludes capture')]:
            control(target_folder, command)
            refused = folder / (command + '-must-not-exist.png')
            began = time.monotonic()
            run('look', identity, str(refused), fail=expected)
            elapsed = time.monotonic() - began
            assert elapsed < 5 and not refused.exists(), (command, elapsed)
            run('type', identity, 'no input from failed capture', fail='desktop.look must precede input')
            report.setdefault('print_refusals', []).append(dict(case=command, seconds=elapsed, no_file=True, no_input_permit=True))
            control(target_folder, 'allow-capture' if command == 'protect-capture' else 'print-ok')
            assert target.poll() is None and state()['text'] == ''
            if command == 'print-hang':
                assert state()['paint_stalls'] == 1, 'the fixture must actually stall its painting thread'
        picture('modal-after-refusals')
        run('stop')
        assert modal_service.wait(timeout=5) == 0
        control(target_folder, 'allow-parent')
        control(target_folder, 'quit')
        assert target.wait(timeout=5) == 0
        # Keep the tool-window case that exposed the WGC limitation. It must
        # never silently capture its owner or leave an unseen input permit.
        target_folder = folder.with_name(folder.name + '-tool')
        target, hwnd = fixture(target_folder, 'tool', tool=True)
        state = lambda: read(target_folder / 'state.json')
        window = next(entry['window'] for entry in run('windows')
                      if entry['window']['process'] == target.pid and entry['window']['title'].startswith('Pleamar input fixture '))
        identity, monitor = window['id'], window['monitor']
        read_only_picture('read-only-tool')
        tool_service = broker(monitor, target.pid, 'tool-broker')
        tool_path = folder / 'tool-window.png'
        result = subprocess.run([str(binary), 'agent', 'look', identity, str(tool_path)], env=env,
                                capture_output=True, text=True, encoding='utf-8', timeout=18,
                                creationflags=subprocess.CREATE_NO_WINDOW)
        report['commands'].append(dict(args=['look', identity, str(tool_path)], exit=result.returncode,
                                       stdout=result.stdout, stderr=result.stderr))
        assert result.returncode == 0, result.stderr
        data = json.loads(result.stdout)
        png = tool_path.read_bytes()
        assert data['window'] == identity and png[:8] == b'\x89PNG\r\n\x1a\n'
        width, height = struct.unpack('>II', png[16:24])
        assert (width, height) == (data['width'], data['height'])
        assert data['capture_method'] == 'window-print', 'tool case must exercise the fallback on this runner'
        report.setdefault('printed_pixels', {})[tool_path.name] = printed_pixels(tool_path)
        report['images'].append(dict(file=tool_path.name, width=width, height=height,
                                    capture_method=data['capture_method'], sha256=hashlib.sha256(png).hexdigest()))
        assert desktop.user.SetForegroundWindow(hwnd), 'owned tool bootstrap focus'
        wait(lambda: state().get('foreground'), 'owned tool foreground')
        run('focus', identity)
        act('tool-button', 'click', *point(390, 50))
        wait(lambda: state()['clicks'] == 1, 'explicit tool button click')
        picture('tool-result')
        report['tool_window'] = dict(capture=True, clicks=state()['clicks'], implicit_redirection=False)
        run('stop')
        assert tool_service.wait(timeout=5) == 0
        control(target_folder, 'quit')
        assert target.wait(timeout=5) == 0
        report.update(passed=True, seconds=time.monotonic() - started,
                      checks=['real CLI hover/click/Unicode/selection/keys/wheels/drag/right/middle',
                              'single-use and done revoke input permits', 'stale geometry refusal',
                              'new-file capture and process scope refusal', 'stop, expiry and process exit close broker',
                              'explicit modal identity, disabled owner refusal and actual dialog button',
                              'tool-window capture and button input without owner substitution',
                              'timed-out and protected captures create no file or input permit'],
                      clean_exit=True)
    finally:
        for process, directory in fixtures:
            if process.poll() is None:
                (directory / 'control').write_text('quit', encoding='utf-8')
        for process in reversed(processes):
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        for log in logs:
            log.close()
        save()


def main():
    require_ci()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--tests', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    folder = args.output.resolve()
    if folder.parent != Path(os.environ['RUNNER_TEMP']).resolve() or folder.exists():
        raise RuntimeError('Evidence needs a new direct child of RUNNER_TEMP')
    binary, tests = args.binary.resolve(strict=True), args.tests.resolve(strict=True)
    folder.mkdir()
    exercise(binary, tests, folder)
    print(json.dumps(dict(passed=True, output=str(folder))))


if __name__ == '__main__':
    main()
