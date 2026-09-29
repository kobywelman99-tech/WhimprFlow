#!/usr/bin/env bash
# Builds the Rust core for iOS: WhimprCore.xcframework (device + simulator)
# plus its Swift bindings, both generated into ios/ (gitignored).
# ios/dev.sh runs this before every build; cargo makes repeat runs quick.
set -euo pipefail
cd "$(dirname "$0")/.."

CRATE=whimpr-ffi
LIB=libwhimpr_ffi
OUT_GEN=ios/Generated
OUT_FW=ios/Frameworks/WhimprCore.xcframework
STAGE=target/ios-ffi

# Xcode's environment can point cargo at the wrong SDK; build with the defaults.
unset SDKROOT IPHONEOS_DEPLOYMENT_TARGET
export IPHONEOS_DEPLOYMENT_TARGET=26.0

for target in aarch64-apple-ios aarch64-apple-ios-sim; do
  cargo build -p "$CRATE" --lib --release --target "$target" --quiet
done
# Bindings come from the host build, which carries the same UniFFI metadata.
cargo build -p "$CRATE" --lib --release --quiet

rm -rf "$STAGE" && mkdir -p "$STAGE/headers"
cargo run -p "$CRATE" --bin uniffi-bindgen --release --quiet -- \
  generate --library "target/release/$LIB.dylib" --language swift --out-dir "$STAGE/bindings" >/dev/null

# The C half becomes the xcframework's module; the Swift half is app source.
cp "$STAGE/bindings/"*FFI.h "$STAGE/headers/"
cp "$STAGE/bindings/"*FFI.modulemap "$STAGE/headers/module.modulemap"
mkdir -p "$OUT_GEN"
cp "$STAGE/bindings/"*.swift "$OUT_GEN/"

rm -rf "$OUT_FW"
xcodebuild -create-xcframework \
  -library "target/aarch64-apple-ios/release/$LIB.a" -headers "$STAGE/headers" \
  -library "target/aarch64-apple-ios-sim/release/$LIB.a" -headers "$STAGE/headers" \
  -output "$OUT_FW" >/dev/null
echo "==> Built $OUT_FW and $OUT_GEN/"
