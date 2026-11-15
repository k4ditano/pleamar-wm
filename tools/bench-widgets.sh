#!/bin/bash
# bench-widgets.sh BIN BAHIA WIDGETS [SECONDS] [LABEL]
#
# What the desktop's widgets cost (experiment scene-surfaces): pleamar-wm headless, one
# monitor of 1920 × 1080, software Vulkan (lavapipe), Bahía's scene, and WIDGETS user widgets:
#   0  none
#   1  the clock (it changes once a second)
#   4  the clock, the counter, a second clock and one that moves at every frame (anim.plm)
# BIN holds pleamar-wm and pleamar (the same build); BAHIA is Bahía's folder. After 20 s to
# settle, SECONDS (60) are measured: CPU of pleamar-wm (which runs the scene) and of the
# widgets' processes, RSS and PSS at the end, and each round's own work (`PLEAMAR_BENCH_ROUNDS`:
# mean, p95, max) and rounds a second. One CSV line on stdout:
#   label,widgets,wm_cpu%,widgets_cpu%,wm_rss_MB,wm_pss_MB,widgets_pss_MB,rounds/s,mean_ms,p95_ms,max_ms
# Software rendering: the absolute times are not a graphics card's; compare one build with another.
set -u
BIN=$1; BAHIA=$2; N=$3; SECS=${4:-60}; LABEL=${5:-run}
HERE=$(cd "$(dirname "$0")" && pwd)
d=$(mktemp -d /tmp/bw-XXXXXX); chmod 700 "$d"
home=$d/home; mkdir -p "$home/.config/bahia/widgets" "$home/.local/share/pleamar/bahia" "$home/.cache" "$home/.local/state"
cp "$BAHIA/config/keys.conf" "$home/.config/bahia/"
W=$home/.config/bahia/widgets
cp "$BAHIA/widgets/ejemplos/reloj.plm" "$W/reloj.plm"
sed 's/^scene Reloj/scene Reloj2/' "$BAHIA/widgets/ejemplos/reloj.plm" > "$W/reloj2.plm"
[ -f "$BAHIA/widgets/ejemplos/contador.plm" ] && cp "$BAHIA/widgets/ejemplos/contador.plm" "$W/" || cp "$W/reloj.plm" "$W/contador.plm"
cp "$HERE/bench-widgets/anim.plm" "$W/"
case $N in
    0) list="" ;;
    1) list="reloj:4" ;;
    4) list="reloj:4 contador:6 reloj2:8 anim:10" ;;
esac
json='{"version":1,"widgets":['; sep=""
for w in $list; do json+="$sep{\"path\":\"$W/${w%%:*}.plm\",\"on\":true,\"x\":${w##*:},\"y\":0,\"mon\":0,\"size\":0}"; sep=","; done
echo "$json]}" > "$home/.local/share/pleamar/bahia/widgets-user.json"
run=$d/run; mkdir -p "$run"; chmod 700 "$run"
log=$d/log.txt
env -u WAYLAND_DISPLAY -u DISPLAY $(env | grep -o "^PLEAMAR_[A-Z_]*" | sed "s/^/-u /") PATH="$BIN:$PATH" HOME="$home" XDG_CONFIG_HOME="$home/.config" XDG_CACHE_HOME="$home/.cache" XDG_DATA_HOME="$home/.local/share" \
    XDG_STATE_HOME="$home/.local/state" XDG_RUNTIME_DIR="$run" PLEAMAR_SOCKETS="$run/pleamar" PLEAMAR_CONFIG="$home/.config/bahia" \
    PLEAMAR_NO_SKILL_REFRESH=1 PLEAMAR_BENCH_ROUNDS=1 PLEAMAR_HEADLESS_AT=${SHOT_AT:-999999} PLEAMAR_HEADLESS_PNG="$d/shot.png" \
    ${LD_LIBRARY_PATH:+LD_LIBRARY_PATH=$LD_LIBRARY_PATH} ${VK_ICD_FILENAMES:+VK_ICD_FILENAMES=$VK_ICD_FILENAMES} \
    timeout $((SECS + 60)) "$BIN/pleamar-wm" headless "$BAHIA/bahia.plm" --seconds $((SECS + 40)) > "$log" 2>&1 &
sleep 1
wm=$(pgrep -f "^$BIN/pleamar-wm headless $BAHIA/bahia.plm" | head -1)
ticks() { local t=0; for p in "$@"; do [ -r /proc/$p/stat ] && t=$((t + $(awk '{print $14 + $15}' /proc/$p/stat))); done; echo $t; }
widgets() { pgrep -f "pleamar --scene $W/" | tr '\n' ' '; }
pss() { local t=0; for p in "$@"; do [ -r /proc/$p/smaps_rollup ] && t=$((t + $(awk '/^Pss:/{print $2}' /proc/$p/smaps_rollup))); done; echo $((t / 1024)); }
for i in $(seq 120); do grep -q "first frame" "$log" && break; sleep 0.5; done
sleep 20
hz=$(getconf CLK_TCK)
ws=$(widgets); n0=$(grep -c "^bench · round" "$log")
t0=$(ticks $wm); w0=$(ticks $ws); s0=$(date +%s.%N)
sleep "$SECS"
t1=$(ticks $wm); w1=$(ticks $ws); s1=$(date +%s.%N)
rss=$(( $(awk '/^Rss:/{print $2}' /proc/$wm/smaps_rollup) / 1024 )); wpss=$(pss $wm); wgpss=$(pss $ws)
el=$(awk -v a="$s0" -v b="$s1" 'BEGIN{print b - a}')
rounds=$(grep "^bench · round" "$log" | tail -n +$((n0 + 1)) | awk '{print $4}')
stats=$(echo "$rounds" | sort -n | awk -v el="$el" 'NF{a[n++]=$1; s+=$1} END{if(!n){print "0,0,0,0"; exit} p=a[int(0.95*(n-1))]; printf "%.1f,%.2f,%.2f,%.2f", n/el, s/n, p, a[n-1]}')
nw=$(echo $ws | wc -w)
pct() { awk -v d="$1" -v hz="$hz" -v el="$el" 'BEGIN{printf "%.1f", d * 100 / hz / el}'; }
echo "$LABEL,$N/$nw,$(pct $((t1 - t0))),$(pct $((w1 - w0))),$rss,$wpss,$wgpss,$stats"
kill $wm 2>/dev/null; for p in $(widgets); do kill $p 2>/dev/null; done
wait 2>/dev/null
grep -E "panic|thrown out|desk|the scene draws" "$log" | head -3 >&2
[ -n "${SHOT:-}" ] && cp "$d/shot.png" "$SHOT" 2>/dev/null
[ -n "${KEEP:-}" ] && echo "kept $d" >&2 || rm -rf "$d"
