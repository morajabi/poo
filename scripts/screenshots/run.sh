#!/usr/bin/env zsh
# Regenerates docs/images/*.png from neutral fixture data (no real paths or projects).
# Run it with the screen unlocked: a locked screen renders every window as inactive.
# Nothing is scanned or cleaned: a stub CLI serves the fixture, and cleaning is the app's simulation.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$ROOT/scripts/screenshots"
OUT="$ROOT/docs/images"
W="$(mktemp -d)"
trap 'rm -rf "$W"' EXIT
cd "$ROOT"

cargo build --release -q
swift build --build-system native --package-path app -q
BIN="$ROOT/app/.build/arm64-apple-macosx/debug"
POO="$ROOT/target/release/poo"

# Bucket and plan definitions from the built-in rules, then the fixture on top.
mkdir -p "$W/tpl" "$W/empty" "$W/home/state" "$W/home/config"
POO_HOME="$W/tpl" "$POO" scan --no-providers --root "$W/empty" --json > /dev/null
python3 "$HERE/fixture.py" "$W/tpl/state/report.json" "$W/home/state/report.json"

# Stub CLI: scans and plans come from the fixture, everything else from the real CLI on the fixture home.
cat > "$W/poo" <<STUB
#!/bin/zsh
case "\$1" in
  scan) python3 -c "import json,time;p='$W/home/state/report.json';r=json.load(open(p));r['scanned_at']=int(time.time());json.dump(r,open(p,'w'));print(json.dumps(r))";;
  plan) cat "$W/home/state/plan-stale.json";;
  watch|clean) exit 2;;
  history) python3 -c "import json,time;n=int(time.time());print(json.dumps({'history':[{'at':n-172800,'bytes':4200000000},{'at':n-432000,'bytes':1900000000}]}))";;
  schedule) if [ "\$2" = ls ]; then python3 -c "import json,time;n=int(time.time());print(json.dumps({'schedules':[{'schedule':{'id':'nightly','plan':'stale','every':'1d','at':'03:30','enabled':True},'last_run':{'at':n-40000,'status':'ok','freed_bytes':3100000000,'cleaned':4,'skipped':0},'next_run':n+30000}]}))"; else POO_HOME="$W/home" exec "$POO" "\$@"; fi;;
  *) POO_HOME="$W/home" exec "$POO" "\$@";;
esac
STUB
chmod +x "$W/poo"

# A throwaway bundle with its own id and defaults, so `open` activates it like a real app.
APP="$W/PooShots.app"
mkdir -p "$APP/Contents/MacOS"
cp "$BIN/Poo" "$APP/Contents/MacOS/Poo"
[ -d "$BIN/Sparkle.framework" ] && ditto "$BIN/Sparkle.framework" "$APP/Contents/MacOS/Sparkle.framework"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.poo.screenshots</string>
<key>CFBundleName</key><string>Poo</string>
<key>CFBundleExecutable</key><string>Poo</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSMinimumSystemVersion</key><string>15.0</string>
</dict></plist>
PLIST
codesign -s - --force --deep "$APP" 2>/dev/null
D=dev.poo.screenshots
defaults write $D showInMenuBar -bool false
defaults write $D launchedBefore -bool true

swiftc -O "$HERE/compose.swift" -o "$W/compose"
cat > "$W/wins.swift" <<'SWIFT'
import CoreGraphics
let pid = Int32(CommandLine.arguments[1])!, title = CommandLine.arguments[2]
for w in CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
where (w[kCGWindowOwnerPID as String] as? Int32) == pid && (w[kCGWindowName as String] as? String) == title {
    print(w[kCGWindowNumber as String]!); break
}
SWIFT
swiftc -O "$W/wins.swift" -o "$W/wins"

# shot <name> <window title> <frame "x y w h"|-> [speed] -- app args...
shot() {
  local name=$1 title=$2 frame=$3 speed=$4; shift 5
  [ "$frame" != - ] && defaults write $D "NSWindow Frame main" "$frame 0 0 1470 923 "
  open -n "$APP" --env POO_BIN="$W/poo" --env POO_HOME="$W/home" --env POO_NO_GUARD=1 --env POO_NO_WATCH=1 \
    --env POO_PREVIEW_SPEED="$speed" --args "$@"
  sleep 6
  osascript -e "tell application id \"$D\" to activate" >/dev/null 2>&1 || true
  sleep 1.5
  local pid id
  pid=$(pgrep -nf "PooShots.app/Contents/MacOS/Poo")
  id=$("$W/wins" "$pid" "$title")
  screencapture -o -l "$id" "$W/raw-$name.png"
  kill "$pid"; sleep 1
}
caffeinate -u -d -t 600 &
CAF=$!
shot menubar      "Poo Menu"     -                       1    -- --preview-menu
shot menubar-dark "Poo Menu"     -                       1    -- --preview-menu --dark
shot overview     "Overview"     "175 136 960 640"       1    -- --window
shot browse       "Browse"       "120 136 1240 720"      1    -- --window --preview-section=browse --preview-inspect
shot elsewhere    "Elsewhere on This Mac" "175 136 960 560" 1 -- --window --preview-section=elsewhere
shot cleaning     "Poo Cleaning" -                       0.06 -- --preview-cleaning=freed
shot freed        "Poo Cleaning" -                       3    -- --preview-cleaning=freed
kill $CAF 2>/dev/null || true

mkdir -p "$OUT"
"$W/compose" "$W/raw-menubar.png" "$OUT/menubar.png" light menubar 56 "86 GB"
"$W/compose" "$W/raw-menubar-dark.png" "$OUT/menubar-dark.png" dark menubar 56 "86 GB"
for n in overview browse elsewhere cleaning freed; do "$W/compose" "$W/raw-$n.png" "$OUT/$n.png" light; done
ls -l "$OUT"
