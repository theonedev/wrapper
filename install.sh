#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"

usage() {
    cat <<'EOF'
Usage: ./install.sh [--all]

Install tools for ./build.sh on Apple Silicon macOS or amd64/arm64 Linux.
Supports Homebrew, apt-get (Debian/Ubuntu), and dnf (Fedora).
--all also installs Linux/Windows cross-compilers and all release Rust targets.
Cross-compiling macOS on Linux additionally requires a separately supplied SDKROOT.
Run as your normal user; system package installation uses sudo when needed.
EOF
}

ALL=false
case "${1:-}" in
    '') ;;
    --all) ALL=true ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 1 ;;
esac
if [ "$#" -gt 1 ]; then usage >&2; exit 1; fi

fail() { echo "Error: $*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }
as_root() {
    if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi
}
java_ok() {
    have jar && javac -version 2>&1 | awk '
        /^javac / { split($2, v, "."); if (v[1] >= 17) ok = 1 }
        END { exit !ok }'
}
python_ok() {
    have python3 && python3 -c 'import sys; sys.exit(sys.version_info < (3, 11))'
}

OS=$(uname -s)
case "$(uname -m)" in
    arm64|aarch64) ARCH=aarch64 ;;
    x86_64) ARCH=x86_64 ;;
    *) fail "Only amd64 and arm64 hosts are supported." ;;
esac
case "$OS" in
    Darwin)
        [ "$ARCH" = aarch64 ] || fail "build.sh supports Apple Silicon Macs only."
        if ! xcode-select -p >/dev/null 2>&1; then
            xcode-select --install
            fail "Complete the Command Line Tools installation, then rerun ./install.sh."
        fi
        have brew || fail "Install Homebrew from https://brew.sh, then rerun ./install.sh."
        HOST_TARGET=aarch64-apple-darwin
        ZIG_PLATFORM=aarch64-macos
        ;;
    Linux)
        HOST_TARGET=$ARCH-unknown-linux-musl
        ZIG_PLATFORM=$ARCH-linux
        ;;
    *) fail "Only macOS and Linux are supported." ;;
esac

INSTALL_DIR=${XDG_DATA_HOME:-$HOME/.local/share}/onedev-wrapper
CARGO_DIR=${CARGO_HOME:-$HOME/.cargo}
ENV_FILE=$INSTALL_DIR/env.sh
if [ -f "$ENV_FILE" ]; then
    source "$ENV_FILE"
fi

PACKAGES=()
EXTRA_PATH=()
if [ "$OS" = Darwin ]; then
    java_ok || PACKAGES+=(openjdk@17)
    python_ok || PACKAGES+=(python@3.13)
    if $ALL; then
        have xz || PACKAGES+=(xz)
        if ! have clang-cl || ! have lld-link; then PACKAGES+=(llvm lld); fi
    fi
    if [ "${#PACKAGES[@]}" -gt 0 ]; then brew install "${PACKAGES[@]}"; fi
    if ! java_ok; then EXTRA_PATH+=("$(brew --prefix openjdk@17)/bin"); fi
    if ! python_ok; then EXTRA_PATH+=("$(brew --prefix python@3.13)/libexec/bin"); fi
    if $ALL && { ! have clang-cl || ! have lld-link; }; then
        EXTRA_PATH+=("$(brew --prefix llvm)/bin" "$(brew --prefix lld)/bin")
    fi
else
    if have apt-get; then
        MANAGER=apt
        if ! have cc || ! have c++ || ! have make; then PACKAGES+=(build-essential); fi
        have curl || PACKAGES+=(curl ca-certificates)
        have xz || PACKAGES+=(xz-utils)
        java_ok || PACKAGES+=(default-jdk)
        python_ok || PACKAGES+=(python3)
        if $ALL; then
            have clang-cl || PACKAGES+=(clang)
            have lld-link || PACKAGES+=(lld)
        fi
    elif have dnf; then
        MANAGER=dnf
        if ! have cc || ! have c++ || ! have make; then PACKAGES+=(gcc gcc-c++ make); fi
        have curl || PACKAGES+=(curl ca-certificates)
        have xz || PACKAGES+=(xz)
        java_ok || PACKAGES+=(java-17-openjdk-devel)
        python_ok || PACKAGES+=(python3)
        if $ALL; then
            have clang-cl || PACKAGES+=(clang)
            have lld-link || PACKAGES+=(lld)
        fi
    else
        fail "Supported Linux package managers: apt-get and dnf."
    fi
    if [ "${#PACKAGES[@]}" -gt 0 ]; then
        if [ "$MANAGER" = apt ]; then
            as_root apt-get update
            as_root apt-get install -y "${PACKAGES[@]}"
        else
            as_root dnf install -y "${PACKAGES[@]}"
        fi
    fi
fi

mkdir -p "$INSTALL_DIR/bin"
# Bash 3.2 (macOS) treats an empty array as unset with nounset enabled.
if [ "${#EXTRA_PATH[@]}" -gt 0 ]; then
    for DIRECTORY in "${EXTRA_PATH[@]}"; do export PATH="$DIRECTORY:$PATH"; done
fi
export PATH="$INSTALL_DIR/bin:$CARGO_DIR/bin:$PATH"
java_ok || fail "JDK 17+ is required; install a newer JDK and rerun."
python_ok || fail "Python 3.11+ is required; upgrade Python and rerun."

WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT
RUST_VERSION=$(sed -n 's/^channel = "\([^"]*\)"/\1/p' rust-toolchain.toml)
[ -n "$RUST_VERSION" ] || fail "Cannot read Rust version from rust-toolchain.toml."
if ! have rustup; then
    curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o "$WORK_DIR/rustup.sh"
    sh "$WORK_DIR/rustup.sh" -y --no-modify-path --default-toolchain none
fi
rustup toolchain install "$RUST_VERSION" --profile minimal --component rustfmt --component clippy
TARGETS=("$HOST_TARGET")
if $ALL; then
    TARGETS=(aarch64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl
        x86_64-pc-windows-msvc aarch64-pc-windows-msvc)
fi
rustup target add --toolchain "$RUST_VERSION" "${TARGETS[@]}"

if [ "$OS" = Linux ] || $ALL; then
    ZIG_VERSION=0.16.0
    if ! have zig || [ "$(zig version)" != "$ZIG_VERSION" ]; then
        # SHA-256 values from https://ziglang.org/download/index.json (0.16.0).
        case "$ZIG_PLATFORM" in
            aarch64-macos) ZIG_SHA=b23d70deaa879b5c2d486ed3316f7eaa53e84acf6fc9cc747de152450d401489 ;;
            aarch64-linux) ZIG_SHA=ea4b09bfb22ec6f6c6ceac57ab63efb6b46e17ab08d21f69f3a48b38e1534f17 ;;
            x86_64-linux) ZIG_SHA=70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00 ;;
        esac
        ZIG_NAME=zig-$ZIG_PLATFORM-$ZIG_VERSION
        curl --proto '=https' --tlsv1.2 -fSL "https://ziglang.org/download/$ZIG_VERSION/$ZIG_NAME.tar.xz" -o "$WORK_DIR/zig.tar.xz"
        python3 - "$WORK_DIR/zig.tar.xz" "$ZIG_SHA" <<'PY'
import hashlib
import sys
with open(sys.argv[1], "rb") as archive:
    actual = hashlib.file_digest(archive, "sha256").hexdigest()
if actual != sys.argv[2]:
    sys.exit("Zig download checksum mismatch")
PY
        tar -xJf "$WORK_DIR/zig.tar.xz" -C "$INSTALL_DIR"
        ln -sfn "$INSTALL_DIR/$ZIG_NAME/zig" "$INSTALL_DIR/bin/zig"
    fi
    cargo install cargo-zigbuild --version 0.23.4 --locked
fi
if $ALL; then
    have clang-cl && have lld-link || fail "LLVM clang-cl and lld-link must be on PATH."
    cargo install cargo-xwin --version 0.23.1 --locked
fi

# Preserve existing tools, including cross-compilers installed by an earlier --all.
EXTRA_PATH=("$(dirname "$(command -v javac)")" "$(dirname "$(command -v jar)")"
    "$(dirname "$(command -v python3)")")
for TOOL in clang-cl lld-link; do
    if have "$TOOL"; then EXTRA_PATH+=("$(dirname "$(command -v "$TOOL")")"); fi
done
{
    echo '# Generated by wrapper/install.sh; loaded automatically by build.sh.'
    printf 'export PATH='
    printf '%q:' "$INSTALL_DIR/bin" "$CARGO_DIR/bin" "${EXTRA_PATH[@]}"
    printf '"$PATH"\n'
} > "$ENV_FILE"
echo "Build tools installed. Run in your shell:"
echo '  ./build.sh'
if $ALL && [ "$OS" = Linux ]; then
    echo 'For macOS cross-compilation, provide a macOS SDK via SDKROOT (default /opt/MacOSX11.3.sdk).'
fi
