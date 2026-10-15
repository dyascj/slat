#!/bin/sh
# Builds dist/Slat.app, dist/Slat.zip (first install) and dist/Slat-macos.app.tar.gz
# (what the in-app updater installs). SIGN_IDENTITY picks the signing certificate
# (ad-hoc by default). UNIVERSAL=1 builds arm64 + x86_64 (needs both rustup
# targets); otherwise builds for the host only.
set -eu
cd "$(dirname "$0")/.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' app/Cargo.toml | head -1)

build() { # crate, binary -> prints path of the (possibly lipo'd) binary
  if [ "${UNIVERSAL:-0}" = 1 ]; then
    for t in aarch64-apple-darwin x86_64-apple-darwin; do
      (cd "$1" && cargo build --release --target "$t" --bin "$2" >&2)
    done
    lipo -create -output "$1/target/$2-universal" \
      "$1/target/aarch64-apple-darwin/release/$2" "$1/target/x86_64-apple-darwin/release/$2"
    echo "$1/target/$2-universal"
  else
    (cd "$1" && cargo build --release --bin "$2" >&2)
    echo "$1/target/release/$2"
  fi
}

APP=dist/Slat.app
rm -rf dist && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$(build app slat)" "$APP/Contents/MacOS/Slat"
cp "$(build engine slat-engine)" "$APP/Contents/MacOS/slat-engine"
sed "s/__VERSION__/$VERSION/g" assets/Info.plist > "$APP/Contents/Info.plist"
cp assets/AppIcon.icns "$APP/Contents/Resources/"
printf 'APPL????' > "$APP/Contents/PkgInfo"
codesign --force --deep --sign "${SIGN_IDENTITY:--}" "$APP"
(cd dist && ditto -c -k --keepParent Slat.app Slat.zip && tar -czf Slat-macos.app.tar.gz Slat.app)
echo "built $APP ($VERSION)"
