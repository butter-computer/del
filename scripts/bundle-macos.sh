#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
profile="${1:-debug}"
case "$profile" in
  debug) cargo build --locked -p del-app ;;
  release) cargo build --locked -p del-app --release ;;
  *) echo "Usage: $0 [debug|release]" >&2; exit 1 ;;
esac
# Build and sign away from the installed path. A failed signing operation must
# leave the last usable app intact, and never silently fall back to ad-hoc signing.
staging="$(mktemp -d "target/$profile/.del-bundle.XXXXXX")"
trap 'if [ -d "$staging" ]; then /usr/bin/python3 -c "import shutil,sys; shutil.rmtree(sys.argv[1])" "$staging"; fi' EXIT
bundle="$staging/del.app/Contents"
mkdir -p "$bundle/MacOS" "$bundle/Resources"
cp "target/$profile/del" "$bundle/MacOS/del.new"
mv "$bundle/MacOS/del.new" "$bundle/MacOS/del"
cp crates/app/Info.plist "$bundle/Info.plist"
cp crates/app/assets/Del.icns "$bundle/Resources/Del.icns"
python3 scripts/sign-macos.py "$staging/del.app"
# Keep both the launch location and any running process's signed bundle valid.
# Retired bundles are removed only after no process holds their files open.
python3 scripts/install-macos.py "$staging/del.app" "target/$profile/del.app"
touch "target/$profile/del.app"
