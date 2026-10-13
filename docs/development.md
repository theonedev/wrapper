# Development guide

These instructions are for developers working on a **Wrapper source checkout**.
For normal use, [download the prebuilt package](integration.md#download-wrapper)
and follow the manual integration guide.

## Build a local release

On Apple Silicon macOS (Homebrew and Xcode Command Line Tools) or amd64/arm64
Linux (Debian/Ubuntu or Fedora):

```sh
./install.sh
./build.sh
python3 package.py --host-only --destination dist/local-wrapper
```

On Windows amd64 or arm64, from Command Prompt or PowerShell:

```bat
.\install.bat
.\build.bat
python package.py --host-only --destination dist/local-wrapper
```

`install.sh` / `install.bat` install **development tools**, not an application
service. Run them as your normal user; system package installation requests
elevation when needed. Skip this step if the pinned toolchain is already installed.
Unix setup supports Homebrew, apt-get, and dnf; Windows setup uses WinGet.

The build produces the Java bridge and host executable in `dist/boot/`.
`package.py` is a maintainer tool that assembles a Wrapper release from those
outputs. The destination must be new; choose another `--destination` for subsequent
runs. From `dist/local-wrapper/`, follow the
[Hello service instructions](../examples/hello-service/README.md) to compile and
run the example manually. A host-only release contains one native executable.

Both build scripts accept `clean` to remove Rust artifacts, `build/`, and `dist/`
(honoring `CARGO_TARGET_DIR`). The Unix build automatically loads the environment
saved by `install.sh`; rerunning setup preserves previously installed cross-compilers.

## Toolchain and cross-compilation

Use Rust 1.96.1, JDK 17+, and Python 3.11+. `rust-toolchain.toml` selects Rust;
`Cargo.lock` fixes dependency versions. Linux builds use a static musl target.

Cross-compilation uses Zig 0.16.0, `cargo-zigbuild` 0.23.4, and `cargo-xwin` 0.23.1.
Windows targets also need LLVM `clang-cl` and `lld-link`; cargo-xwin downloads
the Microsoft SDK/CRT. Native Windows builds use Visual Studio 2022 C++ Build Tools.
macOS targets build on Apple Silicon or on Linux with a separately supplied macOS
SDK (`SDKROOT`, default `/opt/MacOSX11.3.sdk`).

On Unix, `./install.sh --all` installs cross-compilers and all five Rust targets,
except for the separately supplied macOS SDK. Then build all targets:

```sh
./build.sh aarch64-apple-darwin \
  x86_64-unknown-linux-musl aarch64-unknown-linux-musl \
  x86_64-pc-windows-msvc aarch64-pc-windows-msvc
```

On Windows, `.\install.bat --all` installs both Windows targets and ARM64 MSVC
tools. Build both with
`.\build.bat x86_64-pc-windows-msvc aarch64-pc-windows-msvc`.
Use `build.sh` for Linux and macOS targets.

## Package a release

After building all five executables:

```sh
python3 package.py --archive dist/artifacts
```

This creates `dist/release/wrapper`, a versioned archive, and its SHA-256 file.
Use `python` on Windows. The destination must not already exist; pass
`--destination PATH` for a fresh location. `--host-only` packages just the current
platform and includes its Rust target in the archive filename.

Packaging validates executable architectures, collects dependency license texts,
embeds notices in `wrapper.jar`, and writes `SHA256SUMS`. Releases include runtime
files, documentation, the example, MIT license, and third-party notices. Vendored
license provenance is recorded in `licenses/sources.json`.

Packaging uses Cargo's cached metadata and licenses; `cargo fetch --locked`
prepares the cache. Integrating a downloaded release requires no Cargo, Python,
or network access.

## Publish releases

The OneDev **Release** job runs `ci/release.sh` in `1dev/buildenv`, validates and
builds all five targets, then publishes two outputs:

| Destination | Files | Purpose |
| --- | --- | --- |
| Build artifacts (`PublishArtifactStep`) | `wrapper-VERSION.tar.gz` and `wrapper-VERSION.tar.gz.sha256` from `dist/artifacts/` | Versioned downloads attached to the build |
| Public site (`PublishSiteStep`) | `wrapper-latest.tar.gz` and `wrapper-latest.tar.gz.sha256` from `dist/site/` | Stable links for application integration |

The version comes from `Cargo.toml`. Each latest archive is an identical copy of
the versioned artifact and keeps its `wrapper-VERSION/` top-level directory.
Its checksum file names `wrapper-latest.tar.gz` so checksum verification works
with the downloaded filename. The public site is updated after versioned artifacts
are published successfully.

The stable download is
[wrapper-latest.tar.gz](https://code.onedev.io/onedev/wrapper/~sites/public/wrapper-latest.tar.gz).
For a specific release, open its Release build in OneDev and download the versioned
package from the Artifacts tab.

The build image must already contain the toolchain and macOS SDK; initial Windows
SDK/CRT downloads require network access. Running `package.py` locally creates
the versioned output; `ci/release.sh` also prepares the stable site files.

## Verification

After building the host runtime:

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
python3 tests/integration.py
```

Use `python` on Windows. Integration tests launch temporary JVM fixtures and need
localhost sockets. They cover configuration, quoting, shutdown, timeouts, restarts,
parent death, control authentication, logs, and launcher arguments. They do not
register machine services. On Linux, also run `bash tests/linux-smoke.sh`; on other
hosts it uses a disposable Java 17 container with the project mounted read-only.

`tests/service-unix.sh` and `tests/service.ps1` exercise real service installation
and removal. Run them as administrator/root on disposable hosts. On a disposable
Unix host, run `python3 tests/native-startup.py` as root for startup diagnostics,
saved installation arguments, and service-account execution with mocked managers.

Before releasing runtime changes, run native SCM, launchd, and systemd acceptance
on the corresponding operating systems. Cross-compilation alone does not validate
service-manager behavior. For documentation changes, follow the example's manual
steps and check links in both the source tree and extracted release.
