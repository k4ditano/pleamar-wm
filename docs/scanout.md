# Direct scanout (a window straight to the screen)

When a window fills a monitor and nothing of the scene is over it —a game or a video at full screen, the bar hidden—, the window's own buffer can go on the monitor's primary plane. Nothing is then put together and nothing is copied. niri, wlroots and KWin do this. It saves the card a full-screen pass and a copy every frame (memory bandwidth, power, a little latency).

**State: the decision only, off by default.** `src/scanout.rs` decides whether a monitor's frame can be a window's buffer, and why not. It is tested with unit tests (`cargo test scanout`). Nothing is put on a plane yet: the second part, below, needs a real DRM session to be tried, and none has been. The switch for it is ready: `PLEAMAR_WM_SCANOUT=1`, or `scanout on` in `session.conf`.

## What is asked

The first condition that fails is the reason given:

1. **The render says the monitor's frame would be exactly one window's picture.** That means one piece, opaque, at full opacity, upright, covering the whole monitor pixel for pixel, with nothing drawn over it: no bar, no glass, no popup, no subsurface.
2. **The monitor is upright.** A turned one is put together upright and then turned.
3. **The buffer is a dmabuf of one plane, of the monitor's size.**
4. **The primary plane takes its format with its layout (modifier),** as its `IN_FORMATS` property lists them (`scanout::in_formats` reads that blob). An ARGB buffer that the program says is opaque is shown as its XRGB twin.
5. **Nobody is copying the monitor** (screencopy, screen sharing, the lens). They read the frame that was put together, and there would not be one.

Each change of the decision is said once in the log:

```
scanout · DP-1: on — window 2 (mpv) straight to the screen (XR24, layout 0x300000000606014)
scanout · DP-1: off — the bar is over it
```

## What is left (the second part)

1. **pleamar (the render).** After putting the drawing together, for a monitor of the session's (`Target::Frames`), it says whether the frame is exactly one window piece, and which buffer that piece is: `Frames::sole_window(Result<buffer number, why>)`. It is said only to a platform that asks for it, so it costs nothing when off. If the platform answers that it showed the buffer itself, the render does not paint or present that monitor that round. What changed while it was not painted is kept as owed, as for a skipped round, so that going back to putting it together paints what is missing.
2. **pleamar-wm (`nest.rs` → `session.rs`).** The dmabufs of the programs' buffers by the number the render knows them by: size, format, modifier, planes, and whether the program's opaque region covers it. They are shared with the DRM side.
3. **`DrmOutput`.**
   - Import the buffer as a framebuffer (`gbm` import with its modifier, `add_planar_framebuffer`), kept while the buffer lives.
   - Test it with an atomic commit with `TEST_ONLY` on the primary plane (enable the atomic capability; `FB_ID`, `SRC_*`, `CRTC_*`).
   - Flip to it.
   - The buffer stays lent until the flip that replaces it lands. Only then is it given back, and its `linux-drm-syncobj` release point signalled. The acquire point is already waited on before the commit is taken (the blocker in `nest.rs`).
4. **Leaving.** The first frame that cannot go straight is put together whole (`painted_as` cleared), so nothing of before stays on screen. The switch back is one flip, like any other, so it does not flicker.
5. **Trying it.** In a real session, with `PLEAMAR_WM_SCANOUT=1`: mpv or a game at full screen, the bar hidden and shown again, a popup over it, sharing the screen, a turned monitor. The log says what was decided each time.
