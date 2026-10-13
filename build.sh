#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"

if [ "${1:-}" = clean ]; then
    if [ "$#" -ne 1 ]; then
        echo "Usage: $0 clean" >&2
        exit 1
    fi
    echo "Cleaning build..."
    cargo clean
    rm -rf build dist
    echo "Build cleaned"
    exit 0
fi

rm -rf build/java
mkdir -p build/java dist/boot
find java -name "*.java" -print > build/java-sources.txt
javac --release 17 -d build/java @build/java-sources.txt
jar --create --file dist/boot/wrapper.jar --date=2026-01-01T00:00:00Z -C build/java . -C . LICENSE

if [ "$#" = 0 ]; then
    BUILD_HOST=$(rustc -vV | sed -n 's/^host: //p')
    case "$BUILD_HOST" in
        *-unknown-linux-gnu) BUILD_HOST=${BUILD_HOST%-gnu}-musl ;;
    esac
    set -- "$BUILD_HOST"
fi
for TARGET in "$@"; do
    case "$TARGET" in
        aarch64-apple-darwin) BINARY=wrapper-macosx-arm-64; EXTENSION= ;;
        x86_64-unknown-linux-musl) BINARY=wrapper-linux-x86-64; EXTENSION= ;;
        aarch64-unknown-linux-musl) BINARY=wrapper-linux-arm-64; EXTENSION= ;;
        x86_64-pc-windows-msvc) BINARY=wrapper-windows-x86-64.exe; EXTENSION=.exe ;;
        aarch64-pc-windows-msvc) BINARY=wrapper-windows-arm-64.exe; EXTENSION=.exe ;;
        *) echo "Unsupported target: $TARGET" >&2; exit 1 ;;
    esac
    case "$TARGET" in
        *-linux-musl) cargo zigbuild --locked --release --target "$TARGET" ;;
        *-windows-msvc)
            case "$(uname -s)" in
                MINGW*|MSYS*|CYGWIN*) RUSTFLAGS='-C target-feature=+crt-static' cargo build --locked --release --target "$TARGET" ;;
                *) RUSTFLAGS='-C target-feature=+crt-static' cargo xwin build --locked --release --target "$TARGET" ;;
            esac ;;
        *-apple-darwin)
            case "$(uname -s)" in
                Darwin) cargo build --locked --release --target "$TARGET" ;;
                *) SDKROOT="${SDKROOT:-/opt/MacOSX11.3.sdk}" MACOSX_DEPLOYMENT_TARGET=11.0 cargo zigbuild --locked --release --target "$TARGET" ;;
            esac ;;
        *) cargo build --locked --release --target "$TARGET" ;;
    esac
    cp "${CARGO_TARGET_DIR:-target}/$TARGET/release/wrapper$EXTENSION" "dist/boot/$BINARY"
done
