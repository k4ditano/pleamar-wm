//! What `pleamar-wm agent help` says.

pub(super) const HELP: &str = "pleamar-wm agent: use the desktop with the agent's own pointer and keyboard
(needs `agent on` in ~/.config/pleamar/session.conf; your mouse and keyboard stay yours)

What a window does
  windows                         every window: its name (PID, or PID.N for one of a program's
                                  several), program, title, box, monitor, keyboard, a dialog's
                                  owner, and `does`: scene (a pleamar scene), tree (its elements,
                                  from the accessibility bus), text (a terminal), look (pictures);
                                  `nothing (private)`: ask the user. A terminal with several shells
                                  says them: name one as PID:SHELL
  can PID                         what it does, its verbs; a terminal's shells, what runs in each,
                                  where, its size and how long it has been quiet
  monitors, layers                the monitors; the programs' own surfaces (bars, docks, panels)
  hide N, show N                  one of those layers off the screen for now, or back
  open [--monitor N] COMMAND...   start a program for the agent, on the monitor it works on,
                                  without taking your keyboard
  send PID MONITOR                that window to another monitor

Names: a window PID, PID.N, PID:SHELL; an element @24 (a tree's number) or @save (a scene's
name; one of digits @=3); a text to look for --text \"Save as\"; a file -o FILE; durations
500ms, 5s, 2m, or seconds; -- ends the options (what follows is text, even @alice or --set)

Read
  tree PID [--hidden] [--json] [--long]
                                  the whole window and its version (`tree V`): what can be acted on,
                                  headings and texts that say something, one a line, under what they
                                  are in: @ID role \"name\" text \"...\" X,Y WxH states (look coordinates;
                                  names cut at 120, --long 400); more than 8 alike rows without a name
                                  fold to `... N more ROLE without a name`; --hidden: what is not
                                  shown too
  tree PID --within @X            a part: that element and what it holds
  tree PID --role ROLE            the elements of that role, under what they are in
  tree PID --max N                at most N lines, the rest counted (with any of the above)
  tree PID --since [V]            what changed since your last read of it (`since V, read at HH:MM`),
                                  or since read V: + came, - went, ~ changed; `full` and the whole
                                  window if that read is not kept or it shows another page or dialog
  tree PID --focused              the focused element and what it is in (a window with a tree)
  tree PID[:SHELL] [--history]    a terminal: `mark M` (its newest line) and its screen's lines;
                                  --history all it keeps; --since [M] what came after your last read
                                  of it, or after line M
  find PID --text T               what says T in its name or text, under what it is in; a terminal's
                                  lines that say it, by number
  at PID X Y                      the element at that point of the picture, under what it is in
  listen PID[:SHELL] [DURATION]   a line for each thing as it happens (30 s, up to 10 m): focus,
                                  text, live, announce, dialog; a scene's events; a terminal's new
                                  lines, after `mark M`

Act: each answers what it did (`pressed @24`, `typed into @12`, `clicked 120,40`), then what
changed since just before it, as `tree PID --since V` gives it (at most 60 lines, --max N; a
page that navigated: `full` and its first lines; a terminal: `mark M` and its new lines; pictures
only: `done`), once the window drew and stopped (at most 1 s; a window that never stops drawing,
a spinner: `still drawing after 0.3 s`); --no-wait: what it did alone
  ... --until COND [DURATION]    then waits as `wait` does: `ok`, what met it, and the changes;
                                  a timeout (5): the action happened, the condition did not hold
  press PID @X [right|middle] [COUNT] [--action N]
                                  the cursor goes to it and presses it (scrolled into view first);
                                  --action N: its action N (0 its default) without the pointer
  press PID --text T [--first]    the element whose whole name is T (several: they are listed, or
                                  --first takes the first)
  focus PID @X|--text T           the focus in its window to it (a field, to type in)
  type PID [@X] [--set] [--] TEXT|-
                                  typed into @X (focused first) or what has the keyboard; - reads it
                                  from the input; --set: its whole text at once. A terminal: into
                                  its shell, a newline is Enter
  key PID NAME                    enter tab escape backspace space up down left right delete home end
                                  pageup pagedown f1..f12
  hotkey PID MODS+KEY             ctrl+l, ctrl+shift+t, alt+f4
  run PID[:SHELL] \"COMMAND\"|- [DURATION] [--full]
                                  a terminal: typed, Enter, and once the shell waits again (30 s,
                                  up to 10 m) `mark M`, how many lines came before the last 200,
                                  and those (--full: all); - reads the command from the input
                                  (<<'EOF': no quoting twice). A timeout (5): what came so far;
                                  `wait PID prompt` or `tree PID --since M` goes on from there
  say PID ORDER...                any order to a pleamar scene: type query words, drag knob 0 -40
  click PID X Y [left|right|middle] [COUNT], drag PID X1 Y1 X2 Y2,
  scroll PID X Y up|down|left|right [STEPS]
                                  the agent's pointer at points of the window's picture
  move PID X Y                    only the pointer there: nothing answered

Look
  look [PID] [-o FILE]            a picture of the window (or of the desktop); prints the file. The
                                  window's own: covered or on another workspace, with its menus
  look PID X Y W H                a part of it, in the picture's coordinates
  look PID @X|--text T [--pad N]  one element only, N units around it; prints its corner
  look PID --words T [--pad N]    just the words T, where an element's text says them
  look PID --marks [--all]        what can be acted on outlined with its @ID, and their records
                                  (--all: texts and headings too)
  look ... --hidpi                at the window's own pixels; prints the scale

Wait
  wait PID COND [DURATION] [--json]
                                  `ok` and what met it, as soon as it holds (30 s, up to 60 s);
                                  what a window does not wait for is refused (exit 3):
    has --text T, gone --text T   something shown says T (case folded), or nothing does; a
                                  terminal: a line that came after the wait began (--from M: after
                                  its mark M), or no line of its screen
    @X gone, @X STATE, @X not STATE
                                  that element went, has or lost a state (a tree's: focused,
                                  checked, selected, expanded, editable, disabled, enabled, busy...;
                                  a scene's: disabled, enabled, checked, selected, offscreen)
    focus @X                      the focus is there (a tree)
    changed                       anything shown changed; a terminal: any output
    settled DURATION              nothing changed, nor the picture, for that long; a terminal: quiet
    prompt                        a terminal's shell waits for a command again (up to 10 m)
    dialog                        the window opened a dialog
    expr 'EXPR'                   a scene's expression holds: expr 'status == \"Saved\"'

The hands
  done                            finished: the light on the monitor goes out now
  stop                            the user's: what the agent tries next is refused until `done`
  pace [default|fast|instant]     how fast the hands go, for every agent of the session
  raw LINE...                     protocol lines, as they are (cua-inject v1)

Exit codes: 0 done, 1 another error (read the message), 2 private window or locked screen (ask
the user), 3 not for this window (the message says what it does), 4 the element is gone (read
again), 5 timeout, 6 ambiguous (name one of those listed), 7 the window is gone, 8 the user
stopped the agent, 64 bad arguments, 69 no session.

Read before you look; look before each click by coordinates: a page moves under you.";
