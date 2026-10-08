# Native Windows backend (in development)

Windows support now belongs to pleamar-wm as well as pleamar and Marea. This
checkout starts the native desktop companion; it does **not** yet provide a
complete Windows equivalent of the Linux compositor session. DWM continues to
compose applications. No WSL, Wayland server or Unix shell is required.

Upstream through `871808b` (0.2.28) is integrated, including its workspace-wave
fix, independent Linux agent keyboard, Chromium emoji/clipboard handling and
desktop cursor socket. The socket remains Linux-only; Windows reports global
cursor positions through pleamar's native event loop and does not need it.
Windows already emits UTF-16 keyboard input through the matching pleamar
backend; it does not adopt the Linux Ctrl+Shift+U/clipboard workaround.
Chromium/Discord typing acceptance is still pending. These merges do not
provide Windows with an independent input seat. The 0.2.28 panel-cursor and
opening-window keyboard fixes remain part of the Linux compositor. Native scene commands now
expose the latest element labels, roles, values and states; their validation
and the remaining capabilities are described below.

The subsequent upstream changes through `987c85b` are also integrated, paired
with pleamar's `a248e1e2` press-position correction. Tablet rotation, horizontal
scrolling and side-by-side remote apps are preserved in the Linux implementation.
The shared session scene is checked on both platforms; this is language validation,
not a native remote-session acceptance test. The new phone session includes
its virtual output, gestures, return curtain and Linux session-lock behavior.
Those implementations use the Linux compositor and remain Linux-only here;
`phone_monitor: false` makes this explicit in the native capabilities. Windows
still needs a remote transport, remote input routing and virtual-output
integration before it can offer the same phone session. Its local, opt-in
foreground input broker does not supply those remote interfaces. Merging the sources
and checking the session scene does not establish that feature on Windows.

Upstream `d9f571c` adds the remote page's home-screen manifest, icons and update
handoff. These remain part of the Linux remote server: its executable watching
and in-place restart use Unix APIs. The Windows `remote` command still reports
that the service is unavailable; the browser assets alone do not provide it.

Upstream through `ba9c03ac` adds the separate `pleamar-wm-stream` encoder,
asynchronous compositor readback, WebCodecs data-channel playback with a video-track
fallback, and native tablet scale. These source changes are preserved for Linux.
The new workspace keeps `pleamar-wm` as its default member, so the normal Windows
build still needs neither FFmpeg development libraries nor Wayland. The optional
stream encoder itself currently uses wlr-screencopy and Linux shared memory; it is
not a Windows streaming backend. Building all workspace members on Windows is not
supported yet. The upstream phone latency figures are not Windows measurements.

The upstream remote fixes through `a4e4f20d` are also preserved: browser/decoder
diagnostics, data-channel backlog pacing and Safari decoder reconfiguration when
H.264 parameters change. These files still belong to the Linux remote backend;
they do not enable the Windows remote command or virtual phone output.

## Build and run from PowerShell

Install the stable Rust x64 MSVC toolchain and Visual Studio Build Tools with
Desktop development with C++ and the Windows SDK. Keep this checkout next to
the matching `pleamar` Windows port (`../pleamar`). The current CI pins pleamar
commit specified in `.github/workflows/native.yml` from the Windows PR; upstream
pleamar alone does not yet include that backend.

```powershell
cargo build --release --locked --features windows-host
cargo test --release --locked
./target/release/pleamar-wm.exe capabilities
./target/release/pleamar-wm.exe monitors
./target/release/pleamar-wm.exe windows
```

Luau stays enabled through the default pleamar dependency. `--scene FILE`
starts a native pleamar scene with its normal hot reload. Ship the same
verified graphics runtime DLLs as pleamar when distributing this executable.
The matching Marea preview installer packages the companion and supervises its
lifetime. This checkout's Cargo build does not install it or enable startup.

## Named scene commands

`emit fullscreen` toggles borderless fullscreen for the active application on
a managed monitor. Marea's optional dedicated Windows-key layer maps it to
Win+F. `fullscreen ID` applies the same operation to an explicit catalog identity
without activating it; a scene's `window ... fullscreen` action uses this route
through the running native session. View-only and process/monitor scopes still
apply. Native fullscreen readback updates `win.$i.fullscreen` in preview scenes.

The version-3 recovery journal stores the frame and placement before mutation
and still reads older journals. Fullscreen windows are excluded from tiling and
initial geometry rules. Exiting restores their previous frame/placement;
normal session exit and restarting after interruption also recover them. An
application that refuses the geometry is rolled back and reports an error.
This is a DWM borderless window, not an application's exclusive graphics mode.

Local acceptance on secondary DISPLAY1 passed normal/free and tiled transitions,
restoration after reopening the journal and geometry-refusal rollback, without
input injection or owned foreground activation. Maximized restoration and focus
sentinels run only in disposable Windows CI; physical Win+F acceptance remains
pending. The opt-in native test is:

```powershell
$env:PLEAMAR_WM_TEST_MONITOR = '\\.\DISPLAY1'
$env:PLEAMAR_WM_TEST_OUTPUT = 'C:/Temp/wm-fullscreen-evidence'
cargo test --release --locked --bin pleamar-wm windows_backend::session::fullscreen_tests::native_fullscreen_and_recovery -- --ignored --exact --nocapture --test-threads=1
```

The automatic session accepts `--say wm 'emit minimize'` and
`--say wm 'emit restore_last'`, matching Marea's optional Win+M / Win+Shift+M
bindings. Minimize targets the actual foreground application and requires its
monitor to be managed. In tiled mode it leaves the layout while retaining its
original free position; restore rejoins the layout and reflows the remaining
windows. Restore uses actual minimize events
from that session, including application title-bar buttons; its history keeps
at most 64 identities and retires them on restore/destruction. A session restart
starts a new history. Disconnected displays and a different process scope are
never substituted for the requested window. Restoration preserves Windows'
restore-to-maximized behavior and requests activation under normal foreground
rules. An empty history or native failure is reported to Marea.

`emit focus_next` and `emit focus_previous` cycle eligible application windows
on the active window's managed monitor. Tiled mode follows its layout order;
free mode keeps a stable cycle despite activation changing the Windows Z order.
Minimized, hidden, disabled and out-of-scope identities are excluded. Navigation
does not leave an active owned dialog. It supports up to 256 live windows per
monitor and reports foreground denial instead of injecting input to bypass it.

`emit close` posts a normal close request to the active window, including an
explicit owned dialog. Applications retain their unsaved-work prompts and can
decline; the result reports `close_requested`, not a completed close. Neither
command substitutes another process when its target changes. Marea's dedicated
Win layer maps Left/Up and Right/Down to previous/next, and Win+Q to close.

The native CLI discovers pleamar scenes through logon-scoped named pipes.
Panels such as Marea appear in `agent scenes` even when the ordinary window
catalog excludes them. Use its current PID and the element names in its tree:

```powershell
./target/release/pleamar-wm.exe agent scenes
./target/release/pleamar-wm.exe agent tree 1234 json
./target/release/pleamar-wm.exe agent press 1234 save
./target/release/pleamar-wm.exe agent say 1234 'type query España ñ'
./target/release/pleamar-wm.exe agent wait 1234 'saved == true 3s'
./target/release/pleamar-wm.exe agent watch 1234 10
```

Replace `1234` and the example element/fact names with those from your scene.
For an ordinary native application, `agent look 1234 'C:\Pictures\Window ñ.png'`
captures its visible window through Windows Graphics Capture without focusing
or restoring it. An HWND rejected by WGC with E_INVALIDARG uses the same bounded
GDI fallback as broker captures, including exact modal/tool windows. The
read-only worker creates no input session, broker or permit. A disabled owner
remains readable and is not redirected to its dialog. Continuous preview scenes
still require WGC. When the process has several windows, pass the exact
`window.id` returned by `agent windows` instead of the PID. `PID.N` is a scene
selector and is not accepted for native capture. Omitting the output path creates
a PNG in the current user's temporary directory; the command prints its path
and dimensions. Existing files are never replaced.

Hidden, minimized, closed and capture-excluded windows are refused. Capture
is bounded to six seconds, 8192 pixels per dimension and 16,777,216 pixels in
total; the target identity and geometry are checked again before writing. The
shared decoder also verifies the PNG dimensions before creating the output. This does not
capture an entire desktop or panels excluded from the ordinary window catalog.
`capabilities` reports `agent_window_capture: true`, `agent_native_input: true`
and `agent_input_mode: "opt-in-foreground"`. Application input requires the
separate, explicitly enabled broker described below.

`agent say` also supports named drag, wheel, hold and key commands. Commands
use the scene's own input and Luau logic; they do not inject the OS mouse or
keyboard. A watch streams while other commands continue. The matching engine
retires waits, watches and actions on reload or client cancellation.

Discovery compares `hello` with the native pipe server PID, then verifies the
PID again before sending an action. A filename containing protocol delimiter
words does not change its identity. Multiple endpoints in the same process
require `scene:ENDPOINT`; selecting only that PID fails as ambiguous. `PID.N`
addresses the process's single scene, not a native child-window input seat.

Independent seats, cursor glide and background program launch remain unavailable
here. They return errors. These limits do not prevent named actions on pleamar scenes. The matching engine documents
the [protocol and interaction guards](https://github.com/SamuelHinestrosa/pleamar/blob/codex/windows-native-026/docs/windows-scene-commands.md).

## Explicit foreground application input

This is shared Windows input, not Linux's independent agent seat. Start the
broker in another PowerShell terminal, naming a connected display exactly as
shown by `agent monitors`. An optional process restricts its scope further:

```powershell
./target/release/pleamar-wm.exe agent serve --input foreground --monitor '\\.\DISPLAY2' --process 1234 --seconds 300
```

The broker is separate from the automatic layout session. It accepts same-user,
same-logon named-pipe commands and exits after its lease (1–3600 seconds,
default 300). A process scope holds its process object and creation identity;
exiting ends the broker and PID reuse cannot substitute another application.
It starts without creating a window, taking focus or injecting
input. `PLEAMAR_WM_NAMESPACE` separates brokers as well as layout sessions.
An idle broker waits on native request, process-exit and cancellation events;
it does not poll the desktop. Only an active request checks its expiration flag
at a bounded interval. Its lease remains a deadline while it sleeps.

```powershell
./target/release/pleamar-wm.exe agent input-status
./target/release/pleamar-wm.exe agent focus 1234
./target/release/pleamar-wm.exe agent look 1234 'C:\Pictures\Before ñ.png'
./target/release/pleamar-wm.exe agent click 1234 120 90
./target/release/pleamar-wm.exe agent look 1234 'C:\Pictures\After click ñ.png'
./target/release/pleamar-wm.exe agent type 1234 'Café 海'
./target/release/pleamar-wm.exe agent done
./target/release/pleamar-wm.exe agent stop
```

Use your actual PID, capture coordinates and new file paths. While a broker
runs, `look` returns JSON with the file, dimensions and selected identity.
Its picture permits one action within 30 seconds. `move`, `click`, `drag` and `scroll`
use physical pixels measured from the captured visible frame, including its
title bar; these are not client-area or logical DPI coordinates. `type`, `key`
and `hotkey` also require a fresh picture and the target in the foreground.
`type PID -` reads up to 4000 Unicode characters from stdin. Hotkey modifiers
are `ctrl`, `alt` and `shift`. `agent help` lists the complete command forms.

The target must remain wholly inside the named monitor and optional process
scope. `agent windows` includes owned dialogs, which automatic tiling still
excludes. A parent and its dialog make a PID ambiguous: select the dialog's
exact `window.id`. The broker never redirects an action from a disabled owner. The broker refuses minimized windows, implicit modal redirection, stale
or reused identities, changed geometry, covered points, cursor confinement and
user-held modifiers/buttons/Escape. Windows can refuse foreground activation or
input into protected/elevated applications; an error does not trigger retries.
An explicit `focus` command forwards the CLI caller's foreground eligibility
only to its connected broker with `AllowSetForegroundWindow`. This remains
subject to [Windows foreground rules](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-allowsetforegroundwindow).
An acknowledgement means Windows accepted the submitted events, not that an
application completed the requested task. Observe it with another `look`.

`done` discards all pictures. `stop` uses a separate pipe, so it can cancel a
capture or pending input guard while the ordinary command pipe is occupied.
Already delivered events cannot be undone; each remaining balanced gesture is
inserted as one batch. The broker also cancels when its lease or request expires.
If no broker exists, `look` retains its read-only behavior and grants no input
permit. A running broker's refusal never falls back to a different capture path.

Validation of this new CLI path is still in progress. Engine input acceptance
does not by itself prove this broker/CLI integration; native end-to-end evidence
must be recorded before treating it as verified.

The Windows Server 2022 native tests exposed `CreateForWindow` failures for both
ordinary owned dialogs and floating tool windows (`0x80070057`). The backend
now tries the target's native GDI `PrintWindow` rendering in a disposable child.
The earlier direct `WM_PRINT` message failed to draw into the other process's DC.
This uses the exact HWND and its child controls, with no desktop rectangle,
owner substitution or window-style mutation. The helper ends on cancellation
or after three seconds; protected, untouched and unavailable images remain
errors and grant no input permit. PrintWindow can supply cached content and its
success does not certify every application pixel was freshly painted. The
result's `capture_method` identifies WGC versus `window-print`; GPU-only apps
can return incomplete or blank GDI images.
The modal capture and button click passed in
[run 37638516551](https://github.com/SamuelHinestrosa/pleamar-wm/actions/runs/37638516551),
including blue client/red patch pixel checks. Its next negative test incorrectly
assumed that ignoring WM_PRINT prevented PrintWindow from obtaining an image.
The revised fixture stalls the actual painting thread once. Hung/protected
capture rejection and the remaining tool-window tests still need a passing run.

## Launching from a scene

The authored `launch "..."` action now starts a native PowerShell command when
the preview has `--window-actions`. It rejects view-only and single-process
preview scopes. See `examples/windows-launch.plm`; its button and Alt+Return
open Notepad. For a quoted executable path, use PowerShell's call operator:
`launch "& 'C:\\Program Files\\Example\\app.exe' '--option'"`.
The monitor option selects capture sources; it does not promise that a newly
launched application will open on that monitor or avoid taking focus.

Each command enters a private Windows Job Object at creation. Its ordinary
descendants remain owned after the shell exits, and end when this preview
closes or its process terminates. Existing app instances and processes started
through a separate system broker are not made part of that group. Save work
before closing a scene whose commands started applications. This is distinct
from Marea's application service and its persistent app-launcher behavior.
At most sixteen launch groups run concurrently. Completed groups release their
handles; polling stops when no group remains. Blank, NUL-containing and oversized
commands fail before execution. A nonzero shell exit is reported in the log.
The shell process handle is released after its exit code is read. The job stays
alive for descendants and retires only after Windows reports no active member.
The native test waits for that bounded transition; a signalled shell alone is
not evidence that all job accounting has completed. This follows the separate
[job accounting lifetime](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_accounting_information).
`capabilities` reports `scene_launch: true` and `agent_background_launch: false`;
this provides no independent agent seat, background typing or Unix shell.

The ordinary Windows suite tests real native process creation with accented
and supplementary Unicode, quotes, spaces, a surviving child after its shell
exits, group cleanup, unaffected sibling processes and forced owner termination.
Its helpers create no windows and send no desktop input. The example is compile
checked; pressing its button and visual launch acceptance remain pending.
Process ownership uses Microsoft's
[creation-time job assignment](https://devblogs.microsoft.com/oldnewthing/20230209-00/?p=107812)
and [nested job lifetime](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs).

`tests/windows-scene-launch.py` is a separate visible integration fixture for
the disposable GitHub-hosted Windows runner. Its explicit workflow step checks
presented scene pixels, a named button reaching Luau and native process launch,
view-only refusal, hot reload and child cleanup after normal or forced scene
exit. It keeps PNGs, command logs and a case report as the
`wm-native-scene-launch` artifact. The guard rejects ordinary local execution;
it is not an installer or a test to run on somebody's active desktop. A pending
or failed workflow is not evidence that these cases pass. Even a pass does not
cover physical input, mixed-DPI monitors or the full Marea walkthrough.

All three cases passed in [the initial CI run](https://github.com/SamuelHinestrosa/pleamar-wm/actions/runs/37582929156)
on Windows Server 2022 using D3D12's Microsoft Basic Render Driver. The scene
PNGs were inspected: its control and Luau count remain visible after reload.
That run used the debug binary, including its diagnostic overlay and deliberate
logic delay; it is functional evidence, not a release-performance measurement.
The workflow now repeats this fixture with the release binary. Its result must
be checked separately after the current source merge.

## Explicit window layouts

The matching Marea profile includes a paged **Window overview** on its selected
monitor. The session advertises `window_overview`; `capabilities` advertises
`visible_window_capture`. The renderer sends demand independently of input
placement, including before a window's first image. Hidden pages stop native
capture and clear retained pictures, while keeping window identities and
titles. The paired Windows renderer releases its peak texture capacity once
all demand and retained pictures are gone; page switches retain it. The Linux compositor
ignores this resource-demand message and keeps its existing client-buffer and
input-placement behavior.

The Marea scene shows four windows at a time, up to the PLM language's 32-slot
limit. Uncapturable/minimized windows remain reachable. Source captures still
share the 16,777,216-pixel budget; multiple very large windows can exceed it.
This is an overview of native windows, not redirected input into their pictures.

The backend manages normal, resizable application windows. It ignores desktop
surfaces, tool windows and windows on other virtual desktops. Automatic
management is opt-in per monitor; explicit one-shot layouts are also available.

Get IDs from `windows` and a monitor name from `monitors`, then:

```powershell
# Replace these placeholders with current catalog IDs.
./target/release/pleamar-wm.exe tile '\\.\DISPLAY2' grid --save './positions ñ.json' ID1 ID2
./target/release/pleamar-wm.exe restore-layout './positions ñ.json'
./target/release/pleamar-wm.exe window ID1 minimize
./target/release/pleamar-wm.exe window ID1 restore
```

`left`, `right`, `columns`, `rows` and `grid` use the chosen monitor's work
area, physical coordinates and DPI-scaled gaps. Windows must already be
wholly on that monitor; maximized/minimized windows must first be restored.
The undo file must not exist. Original positions are saved and flushed before
moving anything. A rejected geometry triggers rollback and a nonzero exit;
the recovery file is retained. Restoring refuses disconnected monitors,
closed windows and windows that moved to a different monitor. An application
can constrain its size or refuse movement; these are reported as failures.

Position and state changes preserve focus and Z order. IDs include native
handle, process, thread and process creation time; they are checked again at
each operation. They are short-lived desktop references, not persisted app
identities or an authorization boundary. Do not reuse an old catalog across
application restarts.

## Persistent session and Marea

```powershell
# Keep this running; all monitors initially stay in free mode.
./target/release/pleamar-wm.exe session --monitor all

# From another PowerShell window:
./target/release/pleamar-wm.exe --say wm 'status'
./target/release/pleamar-wm.exe --say wm 'layout \\.\DISPLAY2 grid'
./target/release/pleamar-wm.exe --say wm 'toggle \\.\DISPLAY2'
./target/release/pleamar-wm.exe --say wm 'quit'
```

Use repeated `--monitor NAME` options to limit the session to certain displays.
`--process PID` further limits it to that application's current process identity.
`--owner PID` binds the session lifetime to an existing process object: normal
exit or termination restores positions and ends the session. PID reuse cannot
keep it alive. The package uses Marea as the owner.
New, closed, restored and minimized windows update active layouts through
native window events. The desktop catalog is not polled continuously; display
topology is checked every two seconds. A resize rejection returns the monitor
to free mode, restores positions and reports the error in `status`.

The session stores original positions under pleamar's configuration directory
in `wm/windows-session.json`, with an exclusive file lock and atomic updates.
Named sessions use `wm/windows-session-NAMESPACE.json`; the namespace is
validated before forming a path. Recovery holds at most 64 managed windows
across monitors and rejects overflow before moving additional windows.
`--state FILE` selects an isolated journal. `quit` restores positions while
preserving minimized state. If the process crashes, the next session restores
the journal before accepting commands. Unavailable/hidden windows and changed
display geometry can leave pending recovery entries, which `status` reports;
real hotplug and multi-monitor show-state acceptance are still pending.

The Windows CI workflow also runs an opt-in native recovery regression on its
disposable runner: maximized, minimized and minimized-from-maximized windows,
return to free layout and recovery from the saved journal. It checks the free
rectangle, restored show state and focus separately. Its report is the
`wm-recovery-acceptance` artifact; compiling the helper or leaving it ignored
does not count as a pass. It changes focus between its own fixture windows and
refuses to run outside the explicit GitHub-hosted CI step. This API regression
does not replace a mixed-DPI, multi-monitor or physical desktop walkthrough.
All six state/restart cases passed on Windows Server 2022 in
[the first recovery run](https://github.com/SamuelHinestrosa/pleamar-wm/actions/runs/37575372834).

Commands use a local named pipe restricted to the current Windows user and
session; remote pipe clients are rejected. Frames, waits and cancellation are
bounded. One request runs on the desktop thread at a time. Idle connections
wait on kernel events. `PLEAMAR_WM_NAMESPACE` separates test sessions.

For a persistent background session, `pleamar-wm-host.exe --monitor all` runs
the same manager as a GUI-subsystem process with no console. It omits the
scene renderer from its executable; use `pleamar-wm.exe` for commands and
scenes. Build this optional Windows-only packaging target with
`--features windows-host`. Default Linux builds/installations keep their
original single executable.

The companion Marea branch now detects this session and offers its supported
layout/restore actions in the menu and finder. It targets the screen where
Marea lives, requires a native acknowledgement, and keeps pending effects
hidden. Its package includes both executables and starts a free session tied
to Marea's process. The owner-exit cleanup works even after the PowerShell
supervisor exits. Building this WM checkout alone does not install it.

## Window rules

The native session reads the `window` lines in pleamar's `session.conf` at
startup (`%APPDATA%/pleamar/session.conf`, or under `PLEAMAR_CONFIG`). Set
`PLEAMAR_WM_CONFIG` or pass `session --rules 'C:/my config/session.conf'`
to choose another UTF-8 file. A missing default file means no rules; a missing
explicit file, invalid window rule, `private` or `workspace` fails startup.
Other Linux session directives are not interpreted by this Windows adapter.

```text
window app=Spotify.exe float size 900x650
window app=firefox.exe title="*Picture-in-Picture*" float
window app=notepad.exe monitor \\.\DISPLAY2
```

`app=` matches the executable filename, including `.exe`, reported by `windows`.
This distinguishes applications that share a native window class. If Windows
denies the executable query, `app` is empty; no class or fake name is substituted.
Selectors are case-insensitive, accept `*`, and combine app/title with AND.
Matching lines merge in order; later sizes/monitors override earlier ones.
Quotes preserve spaces and `#` in titles. Backslashes are literal.

Rules apply once per window after a nonempty action matches, including titles
that arrive later. `float` excludes it from automatic layouts. `size` sets the
outer window rectangle in logical pixels, scaled at the destination display;
this includes the native frame, unlike a Linux client's content size. A monitor
can be its name or zero-based left-to-right index. Transfers are centered in
the destination work area. Oversize, rejected or out-of-session destinations
are reported in `status.rule_errors`; they do not silently redirect elsewhere.
Both source and destination must belong to the explicitly managed displays,
and the process restriction still applies. Geometry waits for normal windows;
it does not unminimize or unmaximize applications.

An initial rule defines its new free position. Tiling and returning to free
mode preserve that position, and quitting does not undo a successful initial
rule. A late floating match restores its previous free bounds first. Changes
are journaled before movement; failed transactions roll back, and interrupted
ones recover at next startup. The version-2 journal also reads version 1;
older companions reject version 2 rather than misinterpreting a transfer.
Errors are retained for that window until it closes or the session restarts;
fix the rule and restart to retry. Restart to reread the file. At most 256
matched/error identities are retained, with the existing 64-window recovery
bound. An unchanged desktop is not continually scanned; with no rules, free
mode still avoids catalog scans entirely.

## Live window previews

```powershell
./target/release/pleamar-wm.exe --scene examples/windows-preview.plm --preview-monitor '\\.\DISPLAY2'
# Capture only one current process on that display:
./target/release/pleamar-wm.exe --scene examples/windows-preview.plm --preview-monitor '\\.\DISPLAY2' --preview-process 1234
```

Choose the source monitor explicitly. `--screen NAME` additionally chooses the
scene's output; it is independent of the source monitor. Source windows belong
to the scene copy on their monitor, or to its first live copy when previewing
an explicitly selected source on a different display.
These are actual Windows Graphics Capture pictures with a shared D3D11 device,
rendered by pleamar through D3D12. Titles, counts, closing, resizing and scene
reload use the normal `windows` scene API. `--preview-process` retains the
current process creation identity; a reused PID does not expand its scope.

For separate copies on every represented monitor:

```powershell
./target/release/pleamar-wm.exe --scene examples/windows-monitors.plm --preview-monitor all
```

The scene uses `screens: each max 4`; `win.N.screen` follows the actual output
name of each scene copy, independently of native monitor enumeration order.
`all` includes only source monitors represented by a live main scene copy.
The example draws each monitor's windows in that copy; named panels and popups
do not redefine the mapping. A source that moves between represented monitors
keeps its slot. Removing a represented output removes its source slots; hot
reload republishes the output mapping.

Each picture converts physical capture dimensions with its source monitor's
DPI. Changing DPI also updates retained-picture geometry without requiring a
new source repaint. The same aggregate capture budget applies across all
monitors. This requires the matching engine with `ToNest::WindowsScreens`.
Mapping/order/removal and per-source DPI conversions have unit coverage; the
new example is compile-checked. Mixed-DPI movement, physical hotplug and the
multi-output scene still require native acceptance. This does not enable
redirected application input or the pending compositor effects.

Without `--window-actions`, this mode is **view-only**. With that explicit flag,
the normal scene actions `focus`, `minimize`, `restore` and `close` operate on
the corresponding native window:

```powershell
./target/release/pleamar-wm.exe --scene examples/windows-overview.plm --screen '\\.\DISPLAY2' --preview-monitor '\\.\DISPLAY2' --window-actions
```

The overview example has six slots. Each action rechecks the window's identity,
monitor and optional process scope. `close` requests a normal application close;
an application that cancels or asks to save remains listed until it actually
closes. Minimized windows keep their slot and last picture, release their capture
resources, and resume capture when restored. Initially minimized windows are
listed without a picture until restored. Capture failures also leave the real
window listed; they do not invent an image or pretend it closed. A failed
capture retries after 1, 2, 4, 8, 16 and then at most every 30 seconds while its
page is visible and the source is not minimized. Only receiving pixels resets
that backoff; a driver that starts but produces no frame cannot spin. Reopening
the page or restoring the source requests an immediate attempt.

The capture device is now created when the first visible, non-minimized picture
is requested. Catalog discovery and scene launch do not require WGC device
initialization. After capture demand ends and the renderer acknowledges pending
frames, the cached D3D11 device retires following a two-second grace period.
Reopening within that interval reuses it; later requests recreate it with the
current renderer adapter. Shared transport failures retain the existing device
or fall back to CPU capture on a fresh allocation, with a diagnostic.

The no-window native device regression exercised eight allocation/retirement
cycles and independently held references. In that short local fixture,
retirement released about 33 MiB of private process commit per cycle. Windows
and driver allocations remained above the initial baseline. This is not a
whole-product RAM/VRAM benchmark or a test of captured-image reopening; native
overview pixels and sustained performance still require acceptance after this
change. CI runs the device regression separately from ordinary unit tests.

Window events, including native cloaking/uncloaking, refresh the catalog. The
two-second timer checks display topology, DPI and work areas, but no longer
enumerates all applications when those values are unchanged. Unit tests cover
retry deadlines and suspended views. Post-change native capture/reopen and
sustained CPU measurements remain pending; earlier measurements below predate
these changes.

`focus` selects the original native window for normal application input. It
respects Windows' foreground restrictions and reports a rejected activation to
stderr; no injected key or input-queue attachment bypasses them. The scene's
focus facts follow observed native focus. Pointer/keyboard forwarding through
the picture remains unavailable. Scene launch is available with the explicit
`--window-actions` opt-in described above.
Restore alone does not request keyboard focus. This does not replace the native
layout session or enable Marea's pending effects.
Capture can be denied by an application or unavailable on a Windows installation;
errors are reported and no synthetic picture is substituted.

With `--window-actions`, a window's `ask:` also requests a real native resize.
`examples/windows-resize.plm` demonstrates compact, large and app-selected
sizes. The dimensions are logical pixels of the **outer native window**,
including its frame, scaled for its source monitor. This differs from a Linux
client's content size. A zero dimension retains that dimension; `ask: 0, 0`
leaves size selection to the application, and `ask: -1, -1` remains picture-only.
No original size is restored when the scene exits.

Resizing preserves Z order and does not activate the window. Growth shifts it
only as much as necessary to stay in its current work area. Straddling windows,
oversize requests and requests exceeding the shared capture budget are rejected.
Minimized/maximized windows are left in that state; the latest request waits
until they return to normal. Use a free/floating window when another layout
manager is active, since two managers can request conflicting geometry.

Each slot retains only its latest size and one outstanding native request, with
at most 30 requests per second. Completion is checked without blocking capture
or other slots. A constrained or unresponsive application is reported to stderr
after one second; it is not continuously retried or reported as resized. Its
actual native size/picture remains authoritative. Small transient animation
sizes below 32 logical pixels are ignored, as on Linux. Windows may still process
an already posted asynchronous resize after the timeout or a subsequent
`ask: 0, 0`; the API cannot retract a posted request.

The D3D12 renderer offers its actual device to the provider. On a compatible
adapter, WGC frames are copied into shared images on the GPU, then copied into
the renderer's window array. Each capture reuses at most two shared images.
Producer fences and consumer GPU-completion guards prevent an image from being
overwritten while it is displayed or being copied. There is no CPU pixel
readback or upload on this transport; it still performs two GPU copies.

The existing CPU transport is retained if sharing is unavailable or fails.
The process log reports the selected transport and failures. For a driver
diagnostic or an A/B comparison, set `$env:PLEAMAR_WM_CAPTURE_CPU = '1'` before
launching the scene; remove that environment variable to restore automatic
negotiation.

Both transports are capped at 30 updates per window per second. A render
acknowledgement bounds queued batches; capture dimensions are bounded to 8192
per edge and 16,777,216 source pixels in aggregate, with at most 64 slots.
Retained minimized pictures count towards that bound. This is not a total GPU
memory cap: WGC, shared images, pending copies and the padded renderer array
also need storage. Capture callbacks and native waitable timers wake the worker;
it does not change the system timer period. Full-product sustained performance
and compositor-effect parity remain separate work.

With matching engine `d4c50a4`, the default-Luau release build and 18 ordinary
WM tests pass on Windows x64/MSVC. An owned DISPLAY2 fixture checks pixels,
resize, Luau, watched reload and closure with both transports. The actual Marea
overview also passes pages 0/1/0/1 with six owned windows, hidden capture demand
and fresh pixels when returning to a page. No physical input or focus change
was involved. A same-executable 32-second comparison used 1.52 CPU seconds
with shared images versus 3.64 with CPU readback, and about 33 MiB less private
commit. Resident memory had an unexplained outlier in an earlier run, so no
consistent RSS saving is claimed. [Commands, hashes, captures and all measurements](https://github.com/SamuelHinestrosa/pleamar/blob/d4c50a40b11ad41bb5d31aa38ee7c31591f974fa/docs/windows-shared-capture.md)
are retained; these short tests do not establish full desktop parity.

## Sending a scene window to another monitor

`send win.$i to 1` now queues a native move to scene output 1. Run
`examples/windows-send.plm` with `--preview-monitor all --window-actions`.
Output indices follow the scene's display names, not the order returned by
the native monitor enumerator. Missing or ambiguous outputs fail explicitly.
With a single-monitor preview scope, sending outside that scope is refused.

Only normal, resizable windows can be sent; restore minimized or maximized
windows first. The move preserves logical outer-frame size using both displays'
DPI and relative position in their work areas. A smaller destination clamps
the size to keep the entire window reachable. Capture dimensions and the shared
pixel budget remain bounded. Application size constraints can reject the move.

Moves and scene size requests share one asynchronous placement pipeline. The
latest destination replaces an unsent request; an in-flight move finishes before
another size request is issued. Each operation revalidates the native window
identity, scope, connected display and scene output. It does not request focus
or change Z order. The catalog reports the monitor actually observed after
movement, not an optimistic assignment. Explicit sends persist when the scene
closes; they are not temporary automatic tiling rules.

The geometry tests cover negative origins, unequal DPI, smaller work areas,
capture limits, disconnected output mappings and overflow. The disposable
one-monitor CI fixture checks same-output routing and unknown-output refusal
on its owned native window. Physical transfer between different monitors,
application DPI responses and mixed-DPI capture continuity remain unverified.
This implements monitor sends, not workspace/pool or private-window support.

The same placement geometry is available to the native CLI:

```powershell
./target/release/pleamar-wm.exe agent monitors
./target/release/pleamar-wm.exe agent windows
./target/release/pleamar-wm.exe agent send 1234 1
```

Use the exact `window.id` instead of a PID when an application has several
windows. Unlike scene output indices, this monitor number comes from the
current `agent monitors` list; a native display name is also accepted. The
command revalidates identity and normal window state, waits for Windows to
confirm placement and returns actual bounds/monitor JSON. It does not request
foreground activation. A request for the current monitor preserves position
and size. Disconnected destinations and ambiguous PIDs fail explicitly.
`agent_window_send` advertises this command independently of native input or
background launch. Foreground input needs its own opt-in broker; background
launch is unavailable. Physical cross-monitor acceptance is still pending.

## Status and remaining parity work

| Area | Windows status |
| --- | --- |
| Monitor/catalog queries and `hyprctl` compatibility reads | Native implementation |
| Explicit five-layout arrangement, undo, minimize/restore | Passed native tests with three owned windows on DISPLAY2; broad application acceptance pending |
| Pleamar scenes, Luau and hot reload | Native D3D12 scene, Luau callbacks and saved logic/scene reloads verified on DISPLAY2 |
| Automatic per-monitor session | Native creation/closure, minimize, failure rollback, shutdown and crash recovery verified on DISPLAY2 |
| Window rules | Native app/title, float, initial size and monitor rules; DISPLAY2 lifecycle tests passed; transfers between physical displays still need acceptance |
| Live scene layouts; private/workspace rules | Pending |
| Live window previews | Experimental native capture/render transport; one source or represented scene monitors, optional native focus/close/minimize/restore and bounded scene size requests; new multi-output mapping awaits native acceptance |
| Marea menu/finder bridge | Module tested with real Luau, IPC and owned Windows windows; complete Marea UI acceptance pending; package lifecycle tested separately |
| Native dock | Experimental: classic-app icons, pin/unpin, restart persistence and relaunch verified on secondary DISPLAY1; actual OS file drop and owned packaged multi-file activation verified in Windows CI; Marea application bar source integrated and tested on secondary DISPLAY1; installed walkthrough and physical-input acceptance pending |
| Rain, snow, ride, animated window transitions | Pending native equivalents |
| Per-monitor tide pools and overview | Pending; Windows virtual desktops are not the same model |
| Independent agent pointer/keyboard, glow and stop UI | Pending; Marea currently uses guarded shared Windows input |
| Agent program launch on another monitor without taking the keyboard | Pending native implementation; the upstream 0.2.19 behavior is still Linux-only |
| Remote desktop/WebRTC and sharing integration | Pending native capture/input/encoder adapters |
| DRM, libinput, PipeWire, Wayland protocols and login session | Linux components; Windows owns the corresponding system facilities |

The native dock is exercised by `examples/windows-dock.plm`, with an explicit
`--screen NAME --preview-monitor NAME --window-actions` scope. Program metadata
runs on a bounded background worker. Pins use a locked atomic read/modify/replace
of `wm/windows-dock.json` in pleamar's Windows configuration directory, separate
from Linux's `session.conf`. View-only and process-scoped previews refuse changes.

Classic programs use `CreateProcessW` with typed executable/file arguments and
explicit breakaway from the runtime job. User applications survive the dock
closing; authored scene helpers still have creation-time Job Object ownership.
An enclosing job that refuses breakaway returns its error instead of launching
an app that would die with the dock. Package launches retain their OS-managed
lifetime. Packaged file activation uses one shell item array per drop. If the app
lacks UWP's `Windows.File` contract, its exact registered Open With handler runs
on a separate STA thread. The file's default app is never substituted. Shell
paths preserve file identity while converting canonical DOS/UNC prefixes; paths
that would alias another file are refused. The native [file activation API](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-iapplicationactivationmanager-activateforfile)
accepts the entire selection in one call. This avoids retrying an unsupported
contract between classic-handler launches.

Package opens use a separate worker with at most four queued requests. Slow
shell activation no longer blocks the preview/capture pump. A full queue is
reported immediately; asynchronous failures reach the existing dock error log.
Closing the preview discards queued requests without waiting for a shell
extension. A request already executing in Windows can still complete, with the
application's OS-managed lifetime. The worker sleeps on a channel when idle.

The 2026-10-07 local scene acceptance rendered icons on NVIDIA GeForce RTX
5070/D3D12 on secondary DISPLAY1. Named actions passed pin, close, scene restart,
actual relaunch and unpin. Captures were inspected; no OS input was injected,
no owned window held foreground at checks, and cleanup left none behind.

[Windows/Linux CI at `fc03d3b`](https://github.com/SamuelHinestrosa/pleamar-wm/actions/runs/37671672948)
passed with default Luau. Its actual OLE test used `DoDragDrop`, accepted a COPY,
and verified both Unicode/metacharacter file paths in the destination app's
arguments. The drag took 549 ms, including the fixture's deliberate movement
and release delays; this is not a UI performance benchmark. Pin/restart/unpin
also passed, and no owned windows remained. Only this explicit disposable CI
step may inject OS input; local invocations refuse it. The initial OLE fixture
needed to process its own queued mouse-down before starting the drag.

[Packaged-app CI at `e624af1`](https://github.com/SamuelHinestrosa/pleamar-wm/actions/runs/37673937182)
passed actual owned MSIX registration, app identity, pin/reopen and both file
activations, with exact Unicode/metacharacter paths and native window identities.
The preceding revision sent separate UWP contract requests between classic
handler invocations; one window disappeared. One array per activation fixed
that failing acceptance. Earlier failures exposed canonical shell paths, the
missing UWP contract and STA requirements. The package and test certificate
were removed. No machine policy, local package registration, Store account or
download is involved. The asynchronous-worker revision also passed the
[same native acceptance](https://github.com/SamuelHinestrosa/pleamar-wm/actions/runs/37675713712).
The fixture does not establish every Store/UWP app. `application_dock: true`
exposes native application icons, pins and activation to Marea; the broader
Linux workspace `dock` capability remains false.

To run the local scene regression on an explicitly named non-primary output:

```powershell
cargo build --release --locked --bin pleamar-wm --example windows-dock-fixture
python tests/windows-dock.py --binary target/release/pleamar-wm.exe `
  --fixture target/release/examples/windows-dock-fixture.exe `
  --monitor '\\.\DISPLAY1' --output C:/Temp/pleamar-dock-acceptance
```

Choose the monitor name from `pleamar-wm monitors` and a fresh output directory.
The harness isolates configuration and cleans up its own windows. This does not
replace an installed Marea walkthrough or physical multi-monitor acceptance.

Unavailable WM commands exit with an error. `capabilities` states their status
explicitly; no rain, independent input seat or compositor session is simulated.
The existing Marea installation is not changed by building this checkout.

## Native acceptance (separate from CI)

The current 0.2.28 Windows x64/MSVC build passes 20 CLI unit tests and builds
both CLI and console-free host with default Luau, against pleamar 0.2.27.
Nine desktop helpers remain opt-in. The CLI executable SHA-256 is
`d6e046081738cb0080bc8e4760d2846ee3652d65c902a5fe39b16a9abb6c5cae`.
Earlier native scene-command tests passed discovery, Unicode names, named
actions, concurrent wait/watch, cancellation and reload. Two later regressions
found stale input destinations when an overlay appeared; the engine correction
passes unit tests, but its native rerun is pending. The latest attempt found
only DISPLAY2 marked primary, and the non-primary-only harness refused to
launch. Exact-head Windows/Linux CI is also pending. Older native records below
do not establish acceptance of the current input guard or full desktop parity.

The opt-in test creates three of its own ordinary native windows only on an
explicitly named **non-primary** display. It tests all five arrangements,
physical bounds, undo, Unicode names and paths, minimize/restore, stale IDs,
focus preservation and failure cases, then destroys its windows. It sends no
mouse or keyboard input. Close any applications that might interfere with the
chosen secondary display before running it:

```powershell
$env:PLEAMAR_WM_TEST_MONITOR = '\\.\DISPLAY2'
cargo test --release --locked native_layouts_and_restore_on_secondary_monitor -- --ignored --nocapture --test-threads=1
$env:PLEAMAR_WM_TEST_BINARY = (Resolve-Path ./target/release/pleamar-wm.exe).Path
$env:PLEAMAR_WM_TEST_HOST = (Resolve-Path ./target/release/pleamar-wm-host.exe).Path
cargo test --release --locked native_session_lifecycle -- --ignored --nocapture --test-threads=1
cargo test --release --locked native_window_rules -- --ignored --nocapture --test-threads=1
cargo test --release --locked native_persistent_window_capture -- --ignored --nocapture --test-threads=1
cargo test --release --locked native_window_preview_repaints -- --ignored --nocapture --test-threads=1
cargo test --release --locked native_window_preview_actions -- --ignored --nocapture --test-threads=1
cargo test --release --locked native_window_scene_sizes -- --ignored --nocapture --test-threads=1
# Optional integration with the matching Marea checkout:
$env:PLEAMAR_WM_TEST_OVERVIEW = (Resolve-Path '../marea-plm/tools/windows-overview.plm').Path
cargo test --release --locked native_marea_overview_pages -- --ignored --nocapture --test-threads=1
```

[Windows and Ubuntu CI](https://github.com/SamuelHinestrosa/pleamar-wm/actions/runs/37337638887)
passed at `f68911b3386faa3d98e426d593a5e30ea46efa2b`, including default Luau,
ordinary unit tests and the Windows background host. The interactive test
stays ignored in CI. Neither compilation nor the geometry
test proves completed visual effects, interaction parity or a complete WM port.

On 2026-10-05, the release build with default Luau support succeeded on Windows
x64/MSVC. Ten ordinary unit tests passed, including IPC malformed-client
recovery, exclusive ownership and cancellation on shutdown. Native tests passed on
`\\.\DISPLAY2`: all five layouts and their undo, actual minimize/restore,
Unicode paths, closed-window rejection, and rollback after a test application
rejected its new size. It created three owned windows, sent no physical input,
left foreground focus unchanged, and closed all three windows. This does not
validate other applications, mixed-DPI hotplug or the complete compositor UI.
The executable also rendered a real scene on DISPLAY2 through D3D12. Five
captures checked startup, a Luau event, a saved Luau change, a saved scene
change and a subsequent callback. The counter reached seven, accented text
rendered correctly, and the process closed successfully without changing
foreground focus. This was passive validation, not a physical input test.
The separate session test verified automatic create/close handling, keeping a
minimized window minimized when releasing its layout, crash recovery and clean
shutdown. The same test passed with the small background host. In its
five-second idle sample, it performed zero catalog scans and used 1.34 MiB
private commit / 8.95 MiB working set. CPU time did not increase
at the Windows counter's resolution. This short sample covers only the WM
daemon, not Marea, GPU rendering or sustained desktop use; working set includes
shared pages.
The capture test verifies real pixels from two owned windows, static updates
after a consumer pause, resizing, budget rejection, closure and three capture
device/apartment reopenings. The preview regression starts an actual D3D12
scene with a view-only window image. Changing only the source's pixels must
repaint that image, and closing the source must remove it. Both tests run only
on an explicit non-primary display, send no input, and check foreground focus.
Separate local captures also verified Luau callbacks and scene reload with two
live source windows. These checks do not prove interactive window composition.

The scene-size implementation passes 18 ordinary Windows tests. Its separate
native test uses the actual size example and two owned windows on DISPLAY2 at
125% scale: view-only rejection, scaled resizing near the monitor edge, a zero
dimension, tiny/oversize rejection, releasing size control, a refused size,
recovery with a later request, and deferred sizing while minimized. The other
window's real picture continues updating during the refusal. The tested canvas
uses no keyboard and no physical input is sent; foreground-event observation
checks that neither test process activates. This does not establish button-click
acceptance, mixed-DPI transitions, maximized-state behavior or arbitrary apps.

The rule addition passes 14 ordinary Windows unit tests and the separate native
rule test on DISPLAY2. That test covers executable/title selection, DPI-scaled
size, exclusion from tiling, late titles, new windows, rejection/rollback,
scope enforcement, version-1/2 recovery and a three-second idle sample with no
catalog scans. An actual WGC image of its own floating fixture was inspected.
The first attempt failed the end-to-end focus-equality assertion while external
input occurred. The updated observer records foreground process transitions;
the passing run observed no transition, no test-process activation and unchanged
focus, while still recording external input. No physical input was injected.
Transfer between physical monitors, mixed-DPI acceptance and minimized/maximized
rule application remain unverified. That rule revision passed the CI above.
The optional overview actions are a subsequent change and require their own
cross-platform CI; the earlier run does not validate them. Locally, 16 ordinary
tests pass. The separate native overview test passed on DISPLAY2 with actual
WGC-to-D3D12 pictures: initially minimized windows, retained slots, restoration
and subsequent pixel updates, view-only rejection, an application cancelling
close, confirmed close and a command addressing a closed slot. Its product
cards were captured and inspected. The test invokes scene actions over IPC;
physical clicks and successful foreground activation remain untested. The
foreground observer recorded no activation or focus change.

The two-window preview regression also passed actual source repaint, resize,
Luau callbacks, scene reload and closure after this change. In its short
eight-second animation sample, the scene process kept 657 handles and private
commit changed from 186.5 to 187.3 MiB; it consumed 0.688 CPU seconds (about
8.6% of one core). This is a small fixture, not an all-day Marea measurement,
presentation frame rate, or proof that full desktop composition is optimized.

The demand-driven capture change passes 18 ordinary WM tests and the matching
engine's 155 library tests. Its native Marea overview test ran on DISPLAY2 at
125% scaling with six owned windows and four page changes: only the current
page retained image geometry, all six identities remained present, and returning
to a changed source showed fresh real pixels. Actual WGC captures of both pages
were inspected, including Spanish controls and preserved image proportions.
Closing through the scene's exported Luau event ended the process successfully.
The existing native action regression also passed after this change. Neither
test injected input or activated its windows. Successful click-to-focus remains
unverified; unit tests cover the denied-focus and failed-close logic.


### Upstream 0.2.25 compatibility (2026-10-06)

The 0.2.25 upstream release is merged. Both native CLI and console-free host
build with default Luau and locked dependencies against pleamar `a8d641a`
(0.2.24). Eighteen distinct ordinary tests pass in each binary's test target;
nine native helpers remain opt-in. All three Windows example scenes pass
`--check`. The native preview repaint test also passed on non-primary DISPLAY2:
actual WGC pixels changed when its owned source repainted, source closure was
reflected, the scene exited normally and foreground remained unchanged. No
physical input was sent. This rerun does not replace the outstanding interactive
or hardware acceptance listed above.


The subsequent 2026-10-07 lifetime regression passed 56 local default-feature
release tests (18 opt-in helper tests excluded). The real Marea dock scene was
then exercised with `--scene-file ../marea-plm/tools/windows-dock.plm` added to
the command above. Its Spanish menus, native pin/relaunch/unpin, application
survival after the scene exited, and Hide action passed on secondary DISPLAY1.
The harness copies its Luau companion, disables activation for the test,
isolates state and confines all owned windows to that monitor. The actual
Windows taskbar work-area inset is handled by Marea's dock logic. Local primary
monitor acceptance is refused; only the explicit GitHub-hosted fixture may use
`--ci-owned-desktop`. These named scene actions do not inject OS input or prove
physical keyboard/mouse dispatch.
