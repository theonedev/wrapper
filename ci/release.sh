#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
rustc --version
zig version
# The version flag belongs to the executable, not its zigbuild subcommand.
cargo-zigbuild --version
cargo xwin --version
command -v clang-cl
command -v lld-link
test -d "${SDKROOT:-/opt/MacOSX11.3.sdk}"
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
./build.sh x86_64-unknown-linux-musl aarch64-unknown-linux-musl \
    aarch64-apple-darwin x86_64-pc-windows-msvc aarch64-pc-windows-msvc
python3 tests/integration.py
bash tests/linux-smoke.sh
python3 tests/native-startup.py
rm -rf dist/artifacts dist/site dist/release/wrapper
python3 package.py --archive dist/artifacts

# Keep versioned build artifacts and publish identical bytes at the stable site URL.
WRAPPER_VERSION=$(cat dist/release/wrapper/VERSION)
mkdir -p dist/site
cp "dist/artifacts/wrapper-$WRAPPER_VERSION.tar.gz" dist/site/wrapper-latest.tar.gz
(
    cd dist/site
    sha256sum wrapper-latest.tar.gz > wrapper-latest.tar.gz.sha256
)
