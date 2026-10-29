#!/usr/bin/env bash
# The iPhone app, built on Linux with no Mac and no Xcode download:
#   - the app is the Rust library (src/lib.rs) for aarch64-apple-ios;
#   - main.c starts it, tauri-glue.c stands in for Tauri's Swift layer;
#   - Apple's frameworks are linked against stubs (names only): the ones
#     cut from Apple's public iOS update by omarchy-apple-dev's sdk-free
#     mode, plus small ones written here for the frameworks it lacks;
#   - the result is dist/kagaz-ios.ipa, signed ad hoc. `xtool install`
#     signs it with your Apple ID for your iPhone (see README.md).
#
# Needs: source /data/installations/apple-dev/env.sh (rcodesign, sdk-free).
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
sdk=${SDKFREE_HOME:-/data/installations/apple-dev/sdk-free}/iPhoneOS.sdk
work=${KAGAZ_IOS_WORK:-/data/build/kagaz-ios}
target=aarch64-apple-ios
min=15.0
# The system's LLVM, first on the PATH for this script: the Swift toolchain
# that env.sh puts there brings an older lld that cannot link for iOS.
llvm=${KAGAZ_IOS_LLVM:-/usr/lib/llvm-21/bin}
export PATH="$llvm:$PATH"
clang=$llvm/clang
version=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$root/apps/desktop/tauri.conf.json" | head -n1)
build=$(git -C "$root" rev-list --count HEAD)
cargo_target=${CARGO_TARGET_DIR:-$(cd "$root" && cargo metadata --format-version 1 --no-deps | sed 's/.*"target_directory":"\([^"]*\)".*/\1/')}

[ -d "$sdk" ] || { echo "no iPhone sysroot at $sdk: run omarchy-apple-dev's sdk-free/setup.sh --ipsw" >&2; exit 1; }
mkdir -p "$work"

echo "== 1. the Rust library"
(cd "$root" && SDKROOT=$sdk IPHONEOS_DEPLOYMENT_TARGET=$min CC_aarch64_apple_ios=$clang \
  CFLAGS_aarch64_apple_ios="-isysroot $sdk -target arm64-apple-ios$min" \
  cargo rustc -p kagaz-desktop --lib --release --target $target --crate-type staticlib --features tauri/custom-protocol)
lib=$cargo_target/$target/release/libkagaz_desktop_lib.a

echo "== 2. stubs for frameworks the sysroot lacks"
# A stub only names what the app takes from a framework; the iPhone has
# the framework itself. Frameworks used only through the Objective-C
# runtime need no names at all.
stubs=$work/stubs
rm -rf "$stubs"
framework() {
  local name=$1 symbols=${2:-}
  mkdir -p "$stubs/$name.framework"
  {
    echo "--- !tapi-tbd"
    echo "tbd-version: 4"
    echo "targets: [ arm64-ios, arm64e-ios ]"
    echo "install-name: '/System/Library/Frameworks/$name.framework/$name'"
    if [ -n "$symbols" ]; then
      echo "exports:"
      echo "  - targets: [ arm64-ios, arm64e-ios ]"
      echo "    symbols: [ $symbols ]"
    fi
    echo "..."
  } >"$stubs/$name.framework/$name.tbd"
}
framework WebKit
framework CoreData
framework CoreText
framework CloudKit
framework UserNotifications
framework CoreBluetooth "_CBAdvertisementDataLocalNameKey, _CBAdvertisementDataManufacturerDataKey, _CBAdvertisementDataServiceDataKey, _CBAdvertisementDataServiceUUIDsKey, _CBAdvertisementDataTxPowerLevelKey, _CBCentralManagerScanOptionAllowDuplicatesKey"
mkdir -p "$stubs/lib"
cat >"$stubs/lib/libiconv.tbd" <<'EOF'
--- !tapi-tbd
tbd-version: 4
targets: [ arm64-ios, arm64e-ios ]
install-name: '/usr/lib/libiconv.2.dylib'
exports:
  - targets: [ arm64-ios, arm64e-ios ]
    symbols: [ _iconv, _iconv_open, _iconv_close ]
...
EOF

echo "== 3. link"
cc() { "$clang" -target arm64-apple-ios$min -isysroot "$sdk" "$@"; }
cc -O2 -c "$here/main.c" -o "$work/main.o"
cc -O2 -c "$here/tauri-glue.c" -o "$work/tauri-glue.o"
app=$work/Payload/Kagaz.app
rm -rf "$work/Payload"
mkdir -p "$app"
cc -fuse-ld=lld -F "$stubs" -L "$stubs/lib" -o "$app/Kagaz" "$work/main.o" "$work/tauri-glue.o" "$lib" \
  -framework WebKit -framework UIKit -framework UserNotifications -framework CoreData -framework CoreLocation \
  -framework CoreText -framework CoreImage -framework CloudKit -framework QuartzCore -framework CoreGraphics \
  -framework CoreBluetooth -framework Foundation -framework CoreFoundation -framework Security \
  -lobjc -liconv -lc++ -Wl,-dead_strip

echo "== 4. the app bundle"
sed -e "s/@VERSION@/$version/" -e "s/@BUILD@/$build/" -e "s/@MIN@/$min/" "$here/Info.plist" >"$app/Info.plist"
printf 'APPL????' >"$app/PkgInfo"
python3 - "$root/apps/desktop/icons/icon.png" "$app" <<'EOF'
import sys
from PIL import Image
src, app = sys.argv[1], sys.argv[2]
img = Image.open(src).convert("RGBA")
# iPhone icons have no transparency: the icon sits on white.
flat = Image.new("RGB", img.size, (255, 255, 255))
flat.paste(img, mask=img.split()[3])
for name, size in [("AppIcon60x60@2x.png", 120), ("AppIcon60x60@3x.png", 180), ("AppIcon76x76@2x~ipad.png", 152)]:
    flat.resize((size, size), Image.LANCZOS).save(f"{app}/{name}")
EOF

echo "== 5. sign (ad hoc) and pack"
rcodesign sign "$app" >/dev/null
mkdir -p "$root/dist"
(cd "$work" && rm -f Kagaz.ipa && python3 -c "
import os, zipfile
with zipfile.ZipFile('Kagaz.ipa', 'w', zipfile.ZIP_DEFLATED) as z:
    for d, _, files in os.walk('Payload'):
        for f in files:
            p = os.path.join(d, f)
            info = zipfile.ZipInfo.from_file(p, p)
            info.compress_type = zipfile.ZIP_DEFLATED
            with open(p, 'rb') as fh:
                z.writestr(info, fh.read())
")
cp "$work/Kagaz.ipa" "$root/dist/kagaz-ios.ipa"
echo "dist/kagaz-ios.ipa ($(du -h "$root/dist/kagaz-ios.ipa" | cut -f1)), version $version ($build)"
