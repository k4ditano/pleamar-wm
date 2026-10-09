#!/bin/sh
# pleamar-wm as a session of its own. From a TTY of its own (Ctrl+Alt+F3, log
# in there), not from inside another desktop:
#
#   pleamar-session                     the window manager: yours if you made one
#                                       (~/.config/pleamar/wm/session.plm), or its own
#   pleamar-session --seconds 45        ...and it leaves by itself after 45 s
#   pleamar-session other.plm [opts]    another scene
#
# (Installed as pleamar-session; from the source, ./session.sh uses this
# folder's build.) Everything of yours is in ~/.config/pleamar: session.conf,
# keys.conf, autostart. Ctrl+Alt+Backspace leaves; Ctrl+Alt+F1…F12 go to
# another TTY. What it does goes to its log, which is shown when it ends.
here=$(dirname "$(readlink -f "$0")")
wm="$here/target/release/pleamar-wm"
[ -x "$wm" ] || wm=$(command -v pleamar-wm)
[ -n "$wm" ] || { echo "pleamar-wm is not installed"; exit 1; }
dir_state="${XDG_STATE_HOME:-$HOME/.local/state}/pleamar-wm"
log="$dir_state/session.log"
mkdir -p "$dir_state"
[ -f "$log" ] && mv -f "$log" "$log.1"
# The monitors left to right, as Hyprland had them the last time it said so
# (or as PLEAMAR_MONITORS already says).
order="$dir_state/monitors"
if [ -z "$PLEAMAR_MONITORS" ] && [ -f "$order" ]; then
    PLEAMAR_MONITORS=$(cat "$order")
    export PLEAMAR_MONITORS
fi
# The wallpaper Marea has saved, for the autostart that comes with it.
if [ -z "$PLEAMAR_WALLPAPER" ]; then
    w=$(grep -o '"wallpaper" *: *"[^"]*"' "$HOME/.local/share/pleamar/marea/settings.json" 2> /dev/null | sed 's/.*: *"\(.*\)"/\1/')
    [ -n "$w" ] && PLEAMAR_WALLPAPER="$w" && export PLEAMAR_WALLPAPER
fi
# The environment the user keeps for their Wayland sessions, in UWSM's files
# (what Hyprland reads through UWSM): the Qt theme (QT_QPA_PLATFORMTHEME=qt6ct),
# the cursor, the browser. Without it a KDE program (Dolphin) made its palette
# from two themes and drew every other row of a list black, its words unread.
for f in "${XDG_CONFIG_HOME:-$HOME/.config}/uwsm/env" "${XDG_CONFIG_HOME:-$HOME/.config}/uwsm/env-pleamar"; do
    # shellcheck disable=SC1090
    [ -f "$f" ] && . "$f"
done
# Where each frame's time goes, every 300 frames, and in each slow one: cheap,
# and it is what answers «it feels slow» from the log alone.
: "${PLEAMAR_TIMING:=1}"
export PLEAMAR_TIMING
echo "pleamar-wm · its log: $log"
"$wm" session "$@" > "$log" 2>&1
status=$?
echo "pleamar-wm · left (exit $status). The end of its log:"
tail -n 20 "$log"
