"""Rendered scene -> named press -> native child, on a disposable CI desktop only."""
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
import zlib


def require_ci():
    if (sys.platform != 'win32' or os.environ.get('GITHUB_ACTIONS') != 'true'
            or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'
            or os.environ.get('PLEAMAR_WM_CI_SCENE_LAUNCH') != '1'):
        raise RuntimeError('This visible fixture requires the explicit step on a disposable GitHub-hosted Windows runner')


class Desktop:
    def __init__(self):
        self.user = C.WinDLL('user32', use_last_error=True)
        self.gdi = C.WinDLL('gdi32', use_last_error=True)
        self.kernel = C.WinDLL('kernel32', use_last_error=True)
        self.callback = C.WINFUNCTYPE(W.BOOL, W.HWND, W.LPARAM)
        for lib, name, result, args in [
            (self.user, 'EnumWindows', W.BOOL, [self.callback, W.LPARAM]),
            (self.user, 'GetWindowThreadProcessId', W.DWORD, [W.HWND, C.POINTER(W.DWORD)]),
            (self.user, 'GetAncestor', W.HWND, [W.HWND, W.UINT]),
            (self.user, 'GetWindowTextW', C.c_int, [W.HWND, W.LPWSTR, C.c_int]),
            (self.user, 'IsWindowVisible', W.BOOL, [W.HWND]),
            (self.user, 'SetProcessDpiAwarenessContext', W.BOOL, [W.HANDLE]),
            (self.user, 'SetWindowPos', W.BOOL, [W.HWND, W.HWND, C.c_int, C.c_int, C.c_int, C.c_int, W.UINT]),
            (self.user, 'GetClientRect', W.BOOL, [W.HWND, C.POINTER(W.RECT)]),
            (self.user, 'GetWindowRect', W.BOOL, [W.HWND, C.POINTER(W.RECT)]),
            (self.user, 'GetForegroundWindow', W.HWND, []),
            (self.user, 'ClientToScreen', W.BOOL, [W.HWND, C.POINTER(W.POINT)]),
            (self.user, 'GetDC', W.HDC, [W.HWND]),
            (self.user, 'ReleaseDC', C.c_int, [W.HWND, W.HDC]),
            (self.user, 'PostMessageW', W.BOOL, [W.HWND, W.UINT, W.WPARAM, W.LPARAM]),
            (self.gdi, 'CreateCompatibleDC', W.HDC, [W.HDC]),
            (self.gdi, 'CreateCompatibleBitmap', W.HBITMAP, [W.HDC, C.c_int, C.c_int]),
            (self.gdi, 'SelectObject', W.HANDLE, [W.HDC, W.HANDLE]),
            (self.gdi, 'BitBlt', W.BOOL, [W.HDC, C.c_int, C.c_int, C.c_int, C.c_int, W.HDC, C.c_int, C.c_int, W.DWORD]),
            (self.gdi, 'GetDIBits', C.c_int, [W.HDC, W.HBITMAP, W.UINT, W.UINT, C.c_void_p, C.c_void_p, W.UINT]),
            (self.gdi, 'DeleteObject', W.BOOL, [W.HANDLE]),
            (self.gdi, 'DeleteDC', W.BOOL, [W.HDC]),
            (self.kernel, 'OpenProcess', W.HANDLE, [W.DWORD, W.BOOL, W.DWORD]),
            (self.kernel, 'WaitForSingleObject', W.DWORD, [W.HANDLE, W.DWORD]),
            (self.kernel, 'CloseHandle', W.BOOL, [W.HANDLE]),
        ]:
            function = getattr(lib, name)
            function.restype, function.argtypes = result, args
        if not self.user.SetProcessDpiAwarenessContext(W.HANDLE(-4)):
            raise C.WinError(C.get_last_error())

    def pid(self, hwnd):
        pid = W.DWORD()
        assert self.user.GetWindowThreadProcessId(hwnd, C.byref(pid))
        return pid.value

    def window(self, pid, title):
        found = []

        @self.callback
        def visit(hwnd, _):
            text = C.create_unicode_buffer(256)
            self.user.GetWindowTextW(hwnd, text, len(text))
            if text.value == title and self.pid(hwnd) == pid and self.user.IsWindowVisible(hwnd):
                found.append(hwnd)
            return True

        assert self.user.EnumWindows(visit, 0)
        assert len(found) <= 1
        return found[0] if found else None

    def pixels(self, hwnd):
        rect, point = W.RECT(), W.POINT()
        assert self.user.GetClientRect(hwnd, C.byref(rect))
        assert self.user.ClientToScreen(hwnd, C.byref(point))
        width, height = rect.right, rect.bottom
        assert 100 <= width <= 1600 and 100 <= height <= 1200
        source = self.user.GetDC(None)
        target = self.gdi.CreateCompatibleDC(source)
        bitmap = self.gdi.CreateCompatibleBitmap(source, width, height)
        assert source and target and bitmap
        old = self.gdi.SelectObject(target, bitmap)
        try:
            assert self.gdi.BitBlt(target, 0, 0, width, height, source, point.x, point.y, 0x00CC0020)
            self.gdi.SelectObject(target, old)
            old = None
            header = C.create_string_buffer(struct.pack('<IiiHHIIiiII', 40, width, -height, 1, 32, 0, 0, 0, 0, 0, 0))
            pixels = C.create_string_buffer(width * height * 4)
            assert self.gdi.GetDIBits(target, bitmap, 0, height, pixels, header, 0) == height
            return width, height, pixels.raw
        finally:
            if old:
                self.gdi.SelectObject(target, old)
            self.gdi.DeleteObject(bitmap)
            self.gdi.DeleteDC(target)
            self.user.ReleaseDC(None, source)

    def bounds(self, hwnd):
        rect = W.RECT()
        assert self.user.GetWindowRect(hwnd, C.byref(rect))
        return [rect.left, rect.top, rect.right, rect.bottom]


def png(path, picture):
    width, height, bgra = picture
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        row = bgra[y * width * 4:(y + 1) * width * 4]
        for i in range(0, len(row), 4):
            rows.extend((row[i + 2], row[i + 1], row[i]))

    def chunk(name, data):
        return struct.pack('>I', len(data)) + name + data + struct.pack('>I', zlib.crc32(name + data))

    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
                     + chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b''))


def leaf(output):
    # Tk supplies an ordinary native app window. No OS input is injected.
    import tkinter as tk
    window = tk.Tk()
    window.title('Owned launch child ñ 海')
    window.geometry('180x100+700+450')
    window.configure(background='#20c060')
    label = tk.Label(window, text='Native child\nEspaña ñ 海', background='#20c060')
    label.pack(padx=12, pady=20)
    def repaint():
        color = '#d03080' if (output / 'repaint').exists() else '#20c060'
        window.configure(background=color)
        label.configure(background=color)
        window.after(150, repaint)
    window.after(150, repaint)
    window.update()
    hwnd = C.WinDLL('user32').GetAncestor
    hwnd.argtypes, hwnd.restype = [W.HWND, W.UINT], W.HWND
    info = {'pid': os.getpid(), 'hwnd': hwnd(window.winfo_id(), 2)}
    (output / 'child.tmp').write_text(json.dumps(info), encoding='utf-8')
    (output / 'child.tmp').replace(output / 'child.json')
    window.after(120_000, window.destroy)
    window.mainloop()


def scene_source(command, generation='A'):
    return '''scene NativeLaunchCI {
    surface { size: 640, 360; kind: window; title: "Native launch CI"; keyboard: none; rate: 30 }
    windows win max 16
    fact ready = false
    fact count = 0
    fact previews = false
    fact preview_slot = -1
    event invoked ->
    event toggle_preview ->
    event send_here
    event send_missing
    on send_here { send win(preview_slot) to screen.index }
    on send_missing { send win(preview_slot) to 3 }
    box { from: 0, 0; size: screen.width, screen.height; color: __BACKGROUND__ }
    text "Native launch · Generation __GENERATION__" { at: 24, 24; size: 22; color: #eeeeee }
    box open_program { from: 24, 90; size: 260, 60; color: #36514b; label: "Open owned program __GENERATION__" }
    text "Open owned program" { at: 40, 112; size: 16; color: #ffffff }
    on press open_program { emit invoked; launch __COMMAND__ }
    text "Luau actions: {count}" { at: 24, 180; size: 16; color: #ffffff }
    box preview_toggle { from: 24, 230; size: 260, 44; color: #36514b; label: "Toggle owned preview" }
    text "Toggle owned preview" { at: 40, 244; size: 16; color: #ffffff }
    on press preview_toggle { emit toggle_preview }
    repeat i in 0..16 {
        window win.$i { at: 310, 80; size: 300, 220; ask: -1, -1;
            show: previews and win.$i.open and i == preview_slot }
    }
}
'''.replace('__COMMAND__', json.dumps(command, ensure_ascii=False)).replace('__GENERATION__', generation).replace(
        '__BACKGROUND__', '#12171b' if generation == 'A' else '#1b2840')


def exercise(binary, output):
    desktop = Desktop()
    report = dict(passed=False, environment='github-hosted', physical_input=False,
                  full_desktop_acceptance=False, binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), cases=[])
    try:
        for mode in ('view-only', 'normal', 'forced'):
            folder = output / mode
            folder.mkdir()
            scene = folder / "launch ñ ' 海.plm"
            quoted = lambda value: "'" + str(value).replace("'", "''") + "'"
            command = '& ' + ' '.join(map(quoted, (sys.executable, Path(__file__).resolve(), '--leaf', '--output', folder)))
            scene.write_text(scene_source(command), encoding='utf-8')
            scene.with_suffix('.luau').write_text('fact.ready = true\non("invoked", function() fact.count += 1 end)\non("toggle_preview", function() fact.previews = not fact.previews end)\n', encoding='utf-8')
            env = dict(os.environ, PLEAMAR_CONFIG=str(folder / 'config'), APPDATA=str(folder / 'state'),
                       PLEAMAR_SOCKET_DIR=f'native-launch-ci-{os.getpid()}-{mode}')
            flags = subprocess.CREATE_NO_WINDOW | subprocess.BELOW_NORMAL_PRIORITY_CLASS
            process, child_handle = None, None
            with (folder / 'scene.log').open('w', encoding='utf-8') as log:
                def run(arguments):
                    result = subprocess.run([str(binary), *arguments], env=env, stdout=subprocess.PIPE,
                                            stderr=subprocess.PIPE, encoding='utf-8', errors='replace', timeout=15, creationflags=flags)
                    with (folder / 'commands.log').open('a', encoding='utf-8') as trace:
                        trace.write(json.dumps(arguments, ensure_ascii=False) + '\n' + result.stdout + result.stderr)
                    if result.returncode or result.stdout.lstrip().startswith('?'):
                        raise RuntimeError(result.stdout + result.stderr)
                    return result.stdout.strip()

                def until(predicate, label):
                    end, last = time.monotonic() + 35, None
                    while time.monotonic() < end:
                        try:
                            value = predicate()
                            if value:
                                return value
                        except (AssertionError, RuntimeError, FileNotFoundError, json.JSONDecodeError) as error:
                            last = error
                        if process and process.poll() is not None:
                            raise RuntimeError(f'scene exited ({process.returncode}) while waiting for {label}')
                        time.sleep(.15)
                    raise TimeoutError(f'{label}: {last}')

                try:
                    run(['--check', str(scene)])
                    args = [str(binary), '--scene', str(scene), '--preview-monitor', 'all', '--no-hud']
                    if mode != 'view-only':
                        args.append('--window-actions')
                    process = subprocess.Popen(args, env=env, stdout=log, stderr=log, creationflags=flags)
                    hwnd = until(lambda: desktop.window(process.pid, 'Native launch CI'), 'native scene HWND')
                    assert desktop.user.SetWindowPos(hwnd, None, 20, 20, 0, 0, 0x0015)  # no size, Z order or activation change
                    ask = lambda line: run(['agent', 'say', str(process.pid), line])
                    until(lambda: ask('get ready') == 'true', 'default Luau startup')

                    def rendered(generation):
                        picture = desktop.pixels(hwnd)
                        width, _, bgra = picture
                        i = (8 * width + 8) * 4
                        expected = (0x12, 0x17, 0x1b) if generation == 'A' else (0x1b, 0x28, 0x40)
                        actual = (bgra[i + 2], bgra[i + 1], bgra[i])
                        if any(abs(a - b) > 3 for a, b in zip(actual, expected)):
                            return False
                        # The background can arrive before the font workshop.
                        # Require the title and button glyphs in the saved frame.
                        if not scene_labels_ready(picture):
                            return False
                        png(folder / f'frame-{generation}.png', picture)
                        return True

                    until(lambda: rendered('A'), 'presented GPU scene pixels')
                    run(['agent', 'press', str(process.pid), 'open_program'])
                    until(lambda: ask('get count') == '1', 'press reaches Luau')
                    if mode == 'view-only':
                        until(lambda: 'scene is view-only' in (folder / 'scene.log').read_text(encoding='utf-8'), 'explicit launch refusal')
                        assert not (folder / 'child.json').exists()
                    else:
                        child = until(lambda: json.loads((folder / 'child.json').read_text(encoding='utf-8')), 'native child window')
                        assert desktop.pid(child['hwnd']) == child['pid'] and desktop.user.IsWindowVisible(child['hwnd'])
                        child_handle = desktop.kernel.OpenProcess(0x00100000, False, child['pid'])
                        assert child_handle and desktop.kernel.WaitForSingleObject(child_handle, 0) == 258
                        catalog = json.loads(run(['agent', 'windows']))
                        native_id = next(entry['window']['id'] for entry in catalog if entry['window']['process'] == child['pid'])

                        def look(selector, name):
                            path = folder / name
                            reply = run(['agent', 'look', selector, str(path)])
                            data = path.read_bytes()
                            assert reply.startswith(str(path) + ' ') and data[:8] == b'\x89PNG\r\n\x1a\n'
                            width, height = struct.unpack('>II', data[16:24])
                            assert 100 <= width <= 500 and 50 <= height <= 400
                            return data

                        initial_look = look(str(child['pid']), 'native-look-initial.png')
                        try:
                            run(['agent', 'look', native_id, str(folder / 'native-look-initial.png')])
                        except RuntimeError:
                            pass
                        else:
                            raise AssertionError('look replaced an existing file')
                        assert (folder / 'native-look-initial.png').read_bytes() == initial_look
                        # Resolve the owned child's title before enabling one
                        # slot. The child stays alive throughout capture checks.
                        def owned_slot():
                            for slot in range(16):
                                if ask(f'get win.{slot}.title') == 'Owned launch child ñ 海':
                                    return str(slot)
                            return None
                        slot = until(owned_slot, 'owned child in the native catalog')
                        ask(f'fact preview_slot {slot}')
                        until(lambda: ask('get preview_slot') == slot, 'owned slot selection acknowledged')
                        trace = lambda: (folder / 'scene.log').read_text(encoding='utf-8')
                        # This runner has one display. Verify action routing and
                        # refusal on a real HWND, without claiming a transfer.
                        before = desktop.bounds(child['hwnd'])
                        foreground = desktop.user.GetForegroundWindow()
                        ask('emit send_here')
                        ask('emit send_missing')
                        until(lambda: 'the destination has no live scene output' in trace(), 'unknown output refusal')
                        assert 'window actions are unavailable' not in trace()
                        assert desktop.bounds(child['hwnd']) == before
                        assert desktop.user.GetForegroundWindow() == foreground
                        monitors = json.loads(run(['agent', 'monitors']))
                        entry = next(entry['window'] for entry in catalog if entry['window']['id'] == native_id)
                        monitor_index = next(i for i, m in enumerate(monitors) if m['name'] == entry['monitor'])
                        sent = json.loads(run(['agent', 'send', native_id, str(monitor_index)]))
                        assert sent['id'] == native_id and sent['monitor'] == entry['monitor']
                        try:
                            run(['agent', 'send', str(child['pid']), '999'])
                        except RuntimeError as error:
                            assert 'destination is not connected' in str(error)
                        else:
                            raise AssertionError('native send accepted a missing monitor')
                        assert desktop.bounds(child['hwnd']) == before
                        assert desktop.user.GetForegroundWindow() == foreground
                        (folder / 'send-routing.json').write_text(json.dumps(dict(
                            same_output_preserves_bounds=True, missing_output_refused=True,
                            native_agent_send=True, native_agent_missing_output_refused=True,
                            foreground_unchanged=True, physical_transfer_tested=False,
                            bounds=before)), encoding='utf-8')
                        assert 'capture transport =' not in trace(), 'hidden previews allocated a capture device'
                        def preview_pixels(color, name, visible=True):
                            picture = desktop.pixels(hwnd)
                            width, _, bgra = picture
                            matches = 0
                            for y in range(80, 300):
                                for x in range(310, 610):
                                    i = (y * width + x) * 4
                                    if all(abs(a-b) <= 5 for a,b in zip((bgra[i+2],bgra[i+1],bgra[i]),color)):
                                        matches += 1
                            if (matches > 1000) != visible:
                                # Keep the last actual frame when WGC acceptance
                                # fails, not only successful screenshots.
                                png(folder / 'preview-last-mismatch.png', picture)
                                (folder / 'preview-last-mismatch.json').write_text(json.dumps(dict(
                                    expected=color, visible=visible, matching_pixels=matches,
                                    source_id=native_id, preview_slot=slot)), encoding='utf-8')
                                return False
                            png(folder / name, picture)
                            return True
                        run(['agent', 'press', str(process.pid), 'preview_toggle'])
                        until(lambda: preview_pixels((0x20,0xc0,0x60), 'preview-initial.png'), 'real WGC picture in scene')
                        allocations = trace().count('capture transport =')
                        run(['agent', 'press', str(process.pid), 'preview_toggle'])
                        until(lambda: preview_pixels((0x20,0xc0,0x60), 'preview-hidden.png', False), 'hidden preview removed')
                        until(lambda: 'idle capture device retired' in trace(), 'idle capture device retirement')
                        (folder / 'repaint').write_text('owned source only', encoding='utf-8')
                        run(['agent', 'press', str(process.pid), 'preview_toggle'])
                        until(lambda: preview_pixels((0xd0,0x30,0x80), 'preview-reopened.png'), 'fresh source pixels after device recreation')
                        assert trace().count('capture transport =') > allocations, 'preview did not recreate its device'
                        assert look(native_id, 'native-look-updated.png') != initial_look
                        scene.write_text(scene_source(command, 'B'), encoding='utf-8')
                        until(lambda: 'Open owned program B' in run(['agent', 'tree', str(process.pid), 'json']), 'scene hot reload')
                        until(lambda: rendered('B'), 'reloaded GPU scene pixels')
                        assert desktop.kernel.WaitForSingleObject(child_handle, 0) == 258, 'hot reload killed the application'
                    if mode == 'forced':
                        process.kill()
                    else:
                        assert desktop.pid(hwnd) == process.pid
                        assert desktop.user.PostMessageW(hwnd, 0x0010, 0, 0)
                    process.wait(timeout=15)
                    if mode != 'forced':
                        assert process.returncode == 0
                    if child_handle:
                        assert desktop.kernel.WaitForSingleObject(child_handle, 5000) == 0, 'scene left an owned child running'
                        try:
                            run(['agent', 'look', native_id, str(folder / 'closed-window.png')])
                        except RuntimeError:
                            pass
                        else:
                            raise AssertionError('look accepted a closed native window identity')
                        assert not (folder / 'closed-window.png').exists()
                    report['cases'].append(dict(mode=mode, rendered=True, luau=True,
                                              native_child=mode != 'view-only', hot_reload=mode != 'view-only', cleanup=True,
                                              lazy_capture=mode != 'view-only', idle_capture_retirement=mode != 'view-only',
                                              fresh_capture_after_reopen=mode != 'view-only',
                                              send_routing=mode != 'view-only', physical_monitor_transfer=False,
                                              native_agent_send=mode != 'view-only',
                                              native_agent_look=mode != 'view-only'))
                finally:
                    if process and process.poll() is None:
                        process.kill()
                        process.wait(timeout=10)
                    if child_handle:
                        desktop.kernel.CloseHandle(child_handle)
        report['passed'] = True
    finally:
        (output / 'report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--leaf', action='store_true')
    args = parser.parse_args()
    require_ci()
    output = args.output.resolve()
    temporary = Path(os.environ['RUNNER_TEMP']).resolve()
    if not output.is_relative_to(temporary) or output == temporary:
        raise ValueError('fixture output must be a new directory inside RUNNER_TEMP')
    if args.leaf:
        if not output.is_dir() or (output / 'child.json').exists():
            raise ValueError('native child requires its unused fixture directory')
        leaf(output)
    else:
        binary = args.binary.resolve(strict=True)
        output.mkdir()
        exercise(binary, output)


def scene_labels_ready(picture):
    width, height, bgra = picture
    scale = width / 640
    for left, top, right, bottom in [(24, 24, 500, 55), (40, 112, 275, 138)]:
        bright = sum(min(bgra[(y * width + x) * 4:(y * width + x) * 4 + 3]) > 200
                     for y in range(round(top * scale), min(round(bottom * scale), height))
                     for x in range(round(left * scale), min(round(right * scale), width)))
        if bright < 100 * scale * scale:
            return False
    return True


if __name__ == '__main__':
    main()
