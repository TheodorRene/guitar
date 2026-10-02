#!/bin/sh
# Runs guitar off screen, for screenshots: in its own headless sway, on a synthetic input
# (GUITAR_TEST_SIGNAL) with scratch settings. Nothing appears on the user's screen, the
# microphone isn't opened, and their settings aren't touched.
#
#   tools/gui-headless.sh [-m tune|sing] [-g guitar|voice] [-r WxH] [-t seconds]
#                         [-s shot-at-seconds] [-e 'json'] [-o dir] [binary]
#
# -m picks the view, -g the test signal (or `mic` for the real default input), -r the output size (the window tiles to fill it;
# default 1920x1170 like the user's panel), -e adds settings fields, e.g. -e '"span": 36'.
# The binary defaults to ./target/release/guitar, which needs the dev shell: run this under
# `nix develop -c`.
set -eu
mode=tune signal=guitar res=1920x1170 secs=8 shot=6 extra= out=
while getopts m:g:r:t:s:e:o: opt; do
  case $opt in
    m) mode=$OPTARG ;; g) signal=$OPTARG ;; r) res=$OPTARG ;; t) secs=$OPTARG ;; s) shot=$OPTARG ;;
    e) extra=", $OPTARG" ;; o) out=$OPTARG ;;
    *) exit 2 ;;
  esac
done
shift $((OPTIND - 1))
[ "$signal" = mic ] && signal=
bin=$(realpath "${1:-./target/release/guitar}")
out=${out:-$(mktemp -d)}
mkdir -p "$out/state/guitar"
printf '{ "mode": "%s" %s }\n' "$mode" "$extra" > "$out/state/guitar/settings.json"

# Its own config, so none of the user's (or /etc/sway's) exec lines run: that one runs
# `systemctl --user import-environment`, which would point the user's session at this
# display. It reports the display it made; sway sets WAYLAND_DISPLAY for its children.
rm -f "$out/display"
cat > "$out/sway.cfg" <<EOF
output HEADLESS-1 mode --custom ${res}@120Hz
default_border none
exec echo \$WAYLAND_DISPLAY > $out/display
EOF
env -u WAYLAND_DISPLAY -u SWAYSOCK -u I3SOCK -u DISPLAY \
  WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1 \
  sway -c "$out/sway.cfg" > "$out/sway.log" 2>&1 &
sway_pid=$!
trap 'kill $sway_pid 2>/dev/null || true' EXIT
for _ in $(seq 50); do [ -s "$out/display" ] && break; sleep 0.1; done
display=$(cat "$out/display")

(sleep "$shot"; WAYLAND_DISPLAY=$display grim "$out/shot.png") &
shot_pid=$!
WAYLAND_DISPLAY=$display GUITAR_TEST_SIGNAL=$signal XDG_STATE_HOME=$out/state \
  timeout "$secs" "$bin" > "$out/app.log" 2>&1 || true
wait $shot_pid
grep -E "panic|rror" "$out/app.log" || true
echo "screenshot: $out/shot.png"
