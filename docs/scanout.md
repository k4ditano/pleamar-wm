# Direct scanout (a window straight to the screen)

When a window fills a monitor and nothing of the scene is over it —a game or a video at full screen—, the window's own buffer can go on the monitor's primary plane. Nothing is then put together and nothing is copied for that monitor. niri, wlroots and KWin do this. It saves the card a full-screen pass and a copy every frame (memory bandwidth, power, a little latency).

**State: complete, off by default, not yet tried on a real monitor.** Everything that can be checked without a DRM session is tested (`cargo test scanout direct_tests` here, `cargo test sole_window` in pleamar). With it off, nothing of it runs: the switch is read once, and every new path starts by asking it.

It needs pleamar with `Frames::sole_window` (pleamar 0.7.28 or later). With an older pleamar it builds the same and never goes straight.

## Turning it on and off

- **On:** `PLEAMAR_WM_SCANOUT=1` in the environment the session starts with, or `scanout on` in `~/.config/pleamar-wm/session.conf`. It is read once, when the session starts: restart the session after changing it.
- **Off:** remove the line or write `scanout off` (or `PLEAMAR_WM_SCANOUT=0`), and restart the session. If something goes wrong while it is on (a black or frozen monitor), go to another TTY and back (Ctrl+Alt+F2, then the session's): coming back always puts the monitor together again.

## What to look for in the log

At the start:

```
scanout · on: a window that fills a monitor with nothing over it goes straight to the screen (…)
scanout · DP-1: its primary plane takes 42 formats and layouts
```

If a monitor says `this monitor cannot show a program's buffer itself`, it is turned, or its driver does not say what its plane takes: that monitor is always put together.

Then, every time the decision changes (never once per frame):

```
scanout · DP-1: off — no window covers it
scanout · DP-1: window 2 (mpv) could go straight to the screen: asking the card first (XR24, layout 0x300000000606014)
scanout · DP-1: the card takes XR24 with layout 0x300000000606014 on its plane
scanout · DP-1: on — window 2 (mpv) straight to the screen (XR24, layout 0x300000000606014)
scanout · DP-1: off — a program's surface is over it (a bar, a notification)
```

The card is asked once for each format and layout (an atomic commit with `TEST_ONLY`). A refusal is said once and that format is not tried again during the session:

```
scanout · DP-1: off — the card refused AR30 with layout 0x0: Invalid argument
```

The reasons it does not go straight, in the order they are checked:

| Reason in the log | What it means |
| --- | --- |
| `no window covers it`, `the scene draws over the window` | No window fills the monitor, or the scene draws something over it: the bar, a glass, a popup, a dialog. |
| `the window does not cover the monitor exactly`, `…turned or scaled`, `…clipped`, `…not at full opacity`, `…in a group with opacity or effects` | The window is not the whole frame pixel for pixel (it is still fading in, for example: it goes straight once it settles). |
| `the window's buffer is not on the card (shared memory)` | The program draws in memory, not on the card. |
| `the window's buffer is not the monitor's size, pixel for pixel`, `its buffer is …, the monitor …` | The program draws at another size (a game at a lower resolution, scaled up). |
| `its buffer has 2 planes` | A video buffer (NV12): it is put together. |
| `the window is not opaque` | ARGB without an opaque region over all of it. |
| `the primary plane does not take … with layout …` | The plane's `IN_FORMATS` do not list it. |
| `a program's surface is over it`, `another surface of the scene is over it` | A layer-shell program (a notification, another bar) or a surface of the scene is on top. |
| `somebody is copying the monitor` | A screenshot, a recording, screen sharing. |
| `the monitor was given back` | Back from another TTY: put together once, then it goes straight again. |
| `the card refused it`, `the flip to … failed` | The card said no; that format is not tried again. |

## Trying it

1. Turn it on and restart the session. Check the two start lines.
2. **A video:** `mpv --fs` on a file. The log should say `on — window N (mpv)`. With mpv's own hardware decoding into an RGB swapchain it goes; if mpv hands NV12 straight to the compositor it says `its buffer has 2 planes` and is put together, as before.
3. **A game** at full screen at the monitor's resolution (a Vulkan game in Steam, or `vkcube --fullscreen`): `on — window N (…)`.
4. **Leaving:** while it is on, show the bar, open a notification, take a screenshot (`grim`), press Super+F to leave full screen. Each one should say `off — …` and the monitor should be put together at once, with nothing left of before and without a black frame. Coming back to full screen says `on` again (without asking the card: it already answered).

**What should go down while it says `on`** (compare the same video with it off and on):

- `PLEAMAR_TIMING=1` in the session's environment: the line `screen · DP-1: put together N times a second, …` of that monitor stops coming (it is written only while it is put together) or comes with a small N; with it off it says about the video's frame rate.
- The card's load and power: `nvidia-smi dmon -s pu` (NVIDIA) or `radeontop` / `cat /sys/class/drm/card*/device/gpu_busy_percent` (AMD): a few points less while the video plays.
- What does not change: the frames per second of the game or the video, and what is seen.

If what is seen is wrong (a stretched, black or frozen picture) with `on` in the log, turn it off and keep the log lines around that moment.

## How it works

1. **pleamar's render** (`Frames::sole_window`, pleamar 0.7.28): before painting a monitor's frame, for a platform that asks for it (`wants_sole`), it looks at what it composed for that monitor. The last thing drawn there has to be a piece of one window that covers the monitor exactly, pixel for pixel, upright, unclipped, at full opacity and not inside a group with opacity or effects; whatever is under it is hidden. It names the program's buffer (`SoleWindow`). If the platform shows it itself, the render neither paints nor presents that frame, and forgets what its frames held, so that the next one painted —leaving— is painted whole. No platform asks: nothing changes.
2. **The monitor's side** (`screen.rs`, `LayerFrames::sole_window`): the scene's main surface covers the monitor, nothing is stacked over it (layer-shell programs, other surfaces), nobody is copying it, and `scanout::decide` agrees: the buffer is a dmabuf of one plane of the monitor's size, opaque (by its format, or an opaque region over all of it, then read as its XRGB twin), in a format and layout the primary plane lists in its `IN_FORMATS`.
3. **The card's word** (`session.rs`, `Output::test_direct`): the buffer is made a framebuffer (`framebuffer_from_dmabuf`, kept while it is used) and tried with an atomic `TEST_ONLY` commit on the primary plane. Until it has answered, the frame is put together as always. Without atomic commits the flip itself is the test: a refused flip changes nothing on screen.
4. **The flip** (`Output::show_direct`): a page flip to the program's framebuffer, from the monitor's thread. Its landing tells the render and the programs, as any frame's does.
5. **The buffer stays lent** (`scanout::Lending`) from the moment it is chosen until the flip that replaces it lands. What the render hands back meanwhile (it still copies the window into its texture) is kept, and only then given back to its program: its `wl_buffer.release`, or its `linux-drm-syncobj` release point. A monitor unplugged, or the screen given to another TTY, gives back everything it held.
6. **Leaving**: the render saying the frame is not one window, something stacked over it, a capture, the monitor given back, a flip refused. The frame is put together whole (`changed_all`, and the render repaints). When it is the monitor's side that leaves, nothing is put together until the render has painted a fresh frame, so the old picture of the scene never shows for a frame.

## Not done (yet)

- The render still copies the window's buffer into its own texture every frame while it goes straight (the copy it would need on leaving). Skipping it would save one more full-screen copy.
- Video in two planes (NV12) and turned monitors are always put together.
- The cursor is assumed to be on its own plane (it is, on every card the session has run on).
- Tried on a real monitor: not yet. That is what the steps above are for.
