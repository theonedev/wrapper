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
rm -rf dist/artifacts
python3 package.py --archive dist/artifacts
