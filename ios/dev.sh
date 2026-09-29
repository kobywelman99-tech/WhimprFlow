#!/usr/bin/env bash
# iOS dev loop.
#
#   ios/dev.sh test    regenerate the project and run the tests on the simulator
#   ios/dev.sh phone   run the tests, then build, install and launch on the
#                      connected iPhone. Nothing is installed if a test fails.
#
# Needs Config/Local.xcconfig with your DEVELOPMENT_TEAM for `phone`.
# WHIMPR_SIM picks the simulator (default "iPhone 17").
set -euo pipefail
cd "$(dirname "$0")"

SIM="${WHIMPR_SIM:-iPhone 17}"
DERIVED=build/DerivedData
BUNDLE_ID=com.whimpr.whimprflow.ios

run_tests() {
  ../scripts/build-ios.sh
  xcodegen generate --quiet
  echo "==> Running tests on $SIM"
  # Diagnostics collection on failure takes ~10 minutes; the log is enough.
  xcodebuild test -project WhimprFlow.xcodeproj -scheme WhimprFlow \
    -destination "platform=iOS Simulator,name=$SIM" -derivedDataPath "$DERIVED" \
    -collect-test-diagnostics never -quiet
  echo "==> Tests passed"
}

phone_udid() {
  local json
  json=$(mktemp)
  xcrun devicectl list devices --json-output "$json" >/dev/null
  python3 - "$json" <<'EOF'
import json, sys
for d in json.load(open(sys.argv[1]))["result"]["devices"]:
    hw = d["hardwareProperties"]
    if hw.get("reality") == "physical" and hw.get("platform") == "iOS" \
            and d["connectionProperties"].get("tunnelState") == "connected":
        print(hw["udid"])
        break
EOF
  rm -f "$json"
}

install_on_phone() {
  if [[ ! -f Config/Local.xcconfig ]]; then
    echo "Missing ios/Config/Local.xcconfig (copy Local.xcconfig.example and set DEVELOPMENT_TEAM)." >&2
    exit 1
  fi
  local udid
  udid=$(phone_udid)
  if [[ -z "$udid" ]]; then
    echo "No iPhone connected. Plug it in, unlock it, and try again." >&2
    exit 1
  fi
  echo "==> Building for iPhone $udid"
  xcodebuild build -project WhimprFlow.xcodeproj -scheme WhimprFlow \
    -destination "id=$udid" -derivedDataPath "$DERIVED" \
    -allowProvisioningUpdates -quiet
  echo "==> Installing"
  xcrun devicectl device install app --device "$udid" \
    "$DERIVED/Build/Products/Debug-iphoneos/WhimprFlow.app" >/dev/null
  if xcrun devicectl device process launch --device "$udid" "$BUNDLE_ID" >/dev/null 2>&1; then
    echo "==> Installed and launched on the iPhone"
  else
    echo "==> Installed. Couldn't launch it (is the phone locked?); open WhimprFlow on the phone."
  fi
}

case "${1:-}" in
  test) run_tests ;;
  phone) run_tests && install_on_phone ;;
  *) sed -n '2,10p' "$0"; exit 1 ;;
esac
