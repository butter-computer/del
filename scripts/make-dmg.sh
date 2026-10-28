#!/bin/sh
# Alpha-style public release: ad-hoc signed app, drag-to-Applications DMG,
# a SHA-256 sidecar, and a latest.json manifest for the in-app updater.
# Local development bundles and identities are untouched.
set -eu
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version *= *"\([^"]*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m)
[ "$arch" = arm64 ] || { echo "This release script targets Apple Silicon (arm64)." >&2; exit 1; }
export MACOSX_DEPLOYMENT_TARGET=11.0
cargo build --locked -p del-app --release
stage=$(mktemp -d "${TMPDIR:-/tmp}/del-dmg.XXXXXX")
cleanup() { /usr/bin/python3 -c 'import shutil,sys; shutil.rmtree(sys.argv[1])' "$stage"; }
trap cleanup EXIT
app="$stage/src/Del.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/del "$app/Contents/MacOS/del"
cp crates/app/Info.plist "$app/Contents/Info.plist"
cp crates/app/assets/Del.icns "$app/Contents/Resources/Del.icns"
/usr/bin/python3 - "$app/Contents/Info.plist" "$version" <<'PY'
import plistlib, sys
from pathlib import Path
path = Path(sys.argv[1])
info = plistlib.loads(path.read_bytes())
info.update(CFBundleIdentifier='computer.butter.del',
            CFBundleVersion=sys.argv[2], CFBundleShortVersionString=sys.argv[2],
            CFBundleInfoDictionaryVersion='6.0', LSMinimumSystemVersion='11.0',
            LSApplicationCategoryType='public.app-category.developer-tools',
            NSRequiresAquaSystemAppearance=False)
path.write_bytes(plistlib.dumps(info))
PY
plutil -lint "$app/Contents/Info.plist"
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"
ln -s /Applications "$stage/src/Applications"
cp crates/app/assets/Del.icns "$stage/src/.VolumeIcon.icns"
xcrun SetFile -a C "$stage/src"
dmg="target/release/Del-$version-$arch.dmg"
# Build at a temporary destination; only replace the output once verified.
hdiutil create -quiet -srcfolder "$stage/src" -volname Del -fs HFS+ -format UDZO \
    -imagekey zlib-level=9 "$stage/Del.dmg"
hdiutil verify -quiet "$stage/Del.dmg"
mv "$stage/Del.dmg" "$dmg"
(cd target/release && shasum -a 256 "Del-$version-$arch.dmg" > "Del-$version-$arch.dmg.sha256")
printf 'Built %s\n' "$dmg"

# Public updater discovery must not depend on GitHub's shared unauthenticated
# API quota. Upload this manifest with the DMG and checksum before publishing.
/usr/bin/python3 - "$version" "$dmg" <<'MANIFEST'
import json, pathlib, sys
version, dmg = sys.argv[1], pathlib.Path(sys.argv[2])
assets = []
for path in (dmg, pathlib.Path(str(dmg) + '.sha256')):
    assets.append(dict(name=path.name, size=path.stat().st_size,
        browser_download_url=f'https://github.com/butter-computer/del/releases/download/v{version}/{path.name}'))
manifest = dict(tag_name=f'v{version}', draft=False, prerelease=False, assets=assets)
(dmg.parent / 'latest.json').write_text(json.dumps(manifest, indent=2) + '\n')
MANIFEST
printf 'Upload %s, its .sha256 sidecar, and target/release/latest.json together.\n' "$dmg"
