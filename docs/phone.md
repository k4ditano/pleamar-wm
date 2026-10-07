# Your session, in your pocket

`pleamar-wm remote` shows a monitor in a browser elsewhere. From a phone that
is a 1920×1080 desktop squeezed into a hand, with fingers playing mouse. This
note is about the other thing: **taking the session with you**. The same
windows, with whatever was half done in them, become a phone's apps while you
are away; back at the desk, everything is where you left it, plus what you did
from the phone.

Nobody does this. Remote desktops show the computer as it is (GNOME's and
Sunshine's virtual monitors match the client's size, but the desktop stays a
desktop). Phosh and Plasma Mobile are phone shells, not a way into your desk.
DeX turns a phone into a desktop, the other way round. Apple's Continuity
hands over one document at a time. Here the session is one, and it has two
shapes: changing shape is changing scene, which is only possible because the
window manager and the shell are scenes.

## What happens

1. **You open the page on the phone.** It says what it is: its size in pixels,
   its scale, that it is touch. The server asks the session for a **phone
   monitor** of that size (`P W H SCALE` on the agent socket), and the session
   puts one up: a monitor with no screen, painted like the headless ones,
   placed far to the right of the real ones so that no mouse wanders into it.
   The page streams that monitor, not a real one.
2. **The windows come to the phone.** The scene is told which monitor is the
   phone (`phone`), notes where every window was —monitor, pool—, and sends
   them there. On the phone each one is an app: all of the screen under a
   status line, one at a time. A swipe up from the bottom shows the open ones
   as cards; a tap brings one, a flick up closes it, a swipe along the bottom
   edge goes to the next. The window with the keyboard comes first.
3. **The desk is covered.** While the session is away, the real monitors show
   a curtain: whoever walks past your desk sees that it is in use from the
   phone, not what you are doing.
4. **Back at the desk**, any key or movement of the mouse there takes the
   session back: the curtain asks you to unlock (Marea's lock: the session
   was out of your hands), the windows go back to their monitors and pools,
   and the phone says the session went back to the desk. Touching the phone
   again takes it away again.
5. **The phone leaves** (the page closed, the network gone for long enough):
   the same as coming back, without the lock — nobody left the desk exposed.

## Pieces

| Piece | Where | |
| --- | --- | --- |
| A monitor with no screen, put up and taken down while running | `session.rs`, `headless.rs`, `phone.rs` | the same road as a monitor plugged in: the scene's copies are given again |
| `P W H SCALE` · `P off` | `agent.rs` → `layers` → the session's loop | asked by the remote server |
| `phone` · `phone_cards`, `phone_next`, `phone_prev` | a fact and events of the scene | which monitor is the phone (-1: none); the gestures from its bottom edge (`E` on the agent socket) |
| `U HEX` | `agent.rs` | the phone's keyboard: any text into the window with the keyboard, as the person's own typing |
| Apps, cards, status line, gestures | `session.plm` | the phone's copy of the scene |
| The curtain | `session.plm` | a surface over the real monitors, left out of captures |
| Touch | `remote.html` | taps, holds, scrolls with momentum, swipes from the edges; the phone's keyboard types |

## A tablet

A tablet (its short side 600 CSS pixels or more: every iPad, Android
tablets) gets the same session with more room. Its points are its own (the
scale is its pixel ratio: an iPad Air is 820 × 1180 points), so every pixel
of an app is one of the screen: a larger point (× 1.1 was tried) left the
programs that only draw at whole scales, like Telegram, drawn at 3 and
shrunk to 2.2, and soft.

- **Turned on its side** —a tablet or a phone—, the page asks for the new
  size and the phone's monitor takes it where it is: the windows stay on it
  and move to their new places. Meanwhile the old picture blurs under a
  small turning mark. Nothing is locked to standing up any more.
- **Lying down** (at least 900 × 560 points), **two apps side by side**: the
  one in front and the one before it in the deck, or the one asked for.
  Touching the other one puts it in front (each stays on its side). The line
  between them moves with a finger sideways (the wheel sideways, which the
  scene now hears), with a finger held a moment and moved, or with a
  pointer; let go, it settles at a third, the middle or two thirds, and both
  wait under a veil with their names while it moves. Taken to an edge, the
  app on that side goes and the other has all of the screen.
- **The cards**, lying down, go along in a row; each card but the one in
  front has a button to put it **beside** the one in front.
- **A keyboard of its own** (a Magic Keyboard, a Bluetooth one): its keys go
  home as keys from the first one, without opening the one on the screen;
  on an iPad ⌘ is Ctrl at home (⌘C, ⌘V, ⌘T). A trackpad or a mouse is a
  pointer, its clicks and its wheel, as on the desk.

| | |
| --- | --- |
| `ph.wide`, `ph.row` | lying down with room for two; lying down at all (the cards in a row) |
| `ph.side`, `ph.fl`, `ph.ratio`, `ph.solo` | the one beside (-1: the one before in the deck), which side the one in front is on, where the line is, asked to be alone |
| `ph_div` | the line: pressed and moved, or the wheel over it |

## Marea, and other panels

Marea follows you onto the phone (`screens: each max 3`): she lives at the
top of it, in the middle of the status line, like an island, and her card
opens there. A panel wider than the phone —her surface is 820 points, for
her card and its shadow— is shown smaller on the phone's monitor, as if it
were 580 points across (`phone_zoom`): the compositor scales where its
pieces go and where the pointer touches them, and the program draws as
ever. Programs that run through XWayland are drawn at scale 1 and enlarged
on the phone: softer than the rest (Discord and the browsers are native).

## Locked at the desk

The lock screen is only on the real monitors, so a locked session does not
go to the phone: the phone shows the desk as it is —its lock screen— with a
note on top, the phone's keyboard types the password there (each character
as the key of the desk's own layout), and the session comes the moment it
is unlocked.

## An app on the home screen

The page is an app too: «Add to Home Screen» (Safari's share menu on an
iPhone or an iPad; «Install app» in Chrome on Android) leaves the orb on the
home screen, and from it the session opens with all of the screen, without
the browser's bars. On an iPhone or an iPad the app keeps a session of its
own: it is signed in once more, the first time.

**Updates.** The page knows its own version and the server says its own on
connecting: a page left open for days that hears another one offers to load
the new one. And `pleamar-wm remote` watches its own binary: when an update
is installed, it starts again from it by itself if no page is connected, or
the pages show «pleamar was updated at home» with an Update button, which
starts it again and loads the new page. Either way the session stays on the
phone meanwhile (the phone's monitor waits for its page as when the network
goes a moment), and the page brings it back by itself.

## Settings

`session.conf`: `phone lock COMMAND` is what locks the session when it comes
back to the desk (by default `marea lock`; `phone lock none`, nothing). The
page tells it is on a phone or a tablet by itself (an iPad's Safari, which
says it is a Mac, by its fingers); `?phone=1` or `?phone=0` says so.

## Trying it without a phone

`PLEAMAR_HEADLESS_PHONE="1080x2400@2.5 3 20"` puts the phone's monitor up on
a headless desktop by itself (3 s in, down at 20 s); `grim -o PHONE-1`
takes its picture. With `PLEAMAR_HEADLESS_INPUT_FIFO=path` on the headless
desktop and `PLEAMAR_REMOTE_HANDS_TO=path` on `pleamar-wm remote`, the
page's taps go down that pipe instead of to real devices: an emulated phone
(Chrome's device mode) can drive the whole thing without touching the real
session's mouse.

## Measured

Headless, four windows, three runs (2026-10-07):

| | |
| --- | --- |
| Asked for → every window on the phone | 188–197 ms |
| Given back → every window in its monitor and pool | 58–264 ms |
| The arrival, as it is seen | the deck at once; the app in front opens out of it 1.3 s later |

From a key to its picture (2026-10-07, headless, a tablet's 1640 × 2360,
a terminal that changes colour with each key; `PLEAMAR_DEBUG_CAPTURE=1` on
the session and `PLEAMAR_REMOTE_TRACE=1`, `PLEAMAR_STREAM_TRACE=1` on the
remote say where the time goes):

| | before | now |
| --- | --- | --- |
| The key at home → its frame out of the encoder | 220–430 ms after the change (wf-recorder) | 75–130 ms in all |
| The key on the page → its frame on the page | 450–850 ms | 80–107 ms (median 90) |
| Frames a second, text scrolling as fast as it comes | ~18 | ~51 |
| Frames sent with an app still in front | ~16 a second | none |

What did it: the video made by `pleamar-wm-stream` (a monitor's change
copied, encoded and out at once; two copies asked for at a time; a new
bitrate or a whole frame without starting again); the monitor's pictures
read straight from what was put together, into buffers kept, on a thread of
their own; the phone's monitor on a steady 60 Hz clock; the frames on a
data channel, decoded by the page (no jitter buffer); and nothing moving
where it is not seen (the curtain at 20 frames a second, the water behind
the apps still).

The phone's monitor is painted at the phone's own pixels (1080 × 2344 at
scale 2.45 on a 393-point-wide phone: about 440 points across), so text is
as sharp as the phone's own. Still to measure on a real phone: from a tap
to the picture changing.
