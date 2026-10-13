@echo off
setlocal EnableExtensions DisableDelayedExpansion
if /i "%~1"=="--help" goto usage
if /i "%~1"=="-h" goto usage
pushd "%~dp0" || exit /b 1

rem Pick up tools installed by install.bat, even in an already-open terminal.
for /f "delims=" %%P in ('powershell.exe -NoProfile -Command "[Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')"') do set "PATH=%PATH%;%%P"
if not defined CARGO_HOME set "CARGO_HOME=%USERPROFILE%\.cargo"
set "PATH=%CARGO_HOME%\bin;%PATH%"
if defined JAVA_HOME set "PATH=%JAVA_HOME%\bin;%PATH%"

if /i "%~1"=="clean" goto clean
set "TARGETS=%*"
if defined TARGETS goto validate
for /f "tokens=2" %%T in ('rustc -vV ^| findstr /b "host:"') do set "TARGETS=%%T"
if not defined TARGETS (
    echo Error: Cannot determine the Rust host. Run install.bat first. 1>&2
    goto failed
)

:validate
for %%T in (%TARGETS%) do (
    if not "%%~T"=="x86_64-pc-windows-msvc" if not "%%~T"=="aarch64-pc-windows-msvc" (
        echo Unsupported target: %%T. Use build.sh for Linux and macOS. 1>&2
        goto failed
    )
)
rem Keep recursive removal in PowerShell and check the resolved project path.
powershell.exe -NoProfile -Command "$ErrorActionPreference = 'Stop'; $root = (Get-Location).ProviderPath; $path = [IO.Path]::GetFullPath((Join-Path $root 'build/java')); if (-not $path.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Invalid build path' }; if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Recurse -Force }; New-Item -ItemType Directory -Force 'build/java', 'dist/boot' | Out-Null; $sources = Get-ChildItem -LiteralPath java -Recurse -Filter *.java | ForEach-Object { [char]34 + $_.FullName.Replace('\', '/') + [char]34 }; [IO.File]::WriteAllLines((Join-Path $root 'build/java-sources.txt'), [string[]]$sources, [Text.UTF8Encoding]::new($false))"
if errorlevel 1 goto failed
javac -J-Dfile.encoding=UTF-8 --release 17 -d build/java @build/java-sources.txt
if errorlevel 1 goto failed
jar --create --file dist/boot/wrapper.jar --date=2026-01-01T00:00:00Z -C build/java . -C . LICENSE
if errorlevel 1 goto failed

set "RUSTFLAGS=-C target-feature=+crt-static"
rem Cargo gives encoded flags precedence over RUSTFLAGS.
set "CARGO_ENCODED_RUSTFLAGS="
if not defined CARGO_TARGET_DIR set "CARGO_TARGET_DIR=target"
for %%T in (%TARGETS%) do (
    call :build "%%~T"
    if errorlevel 1 goto failed
)
popd
exit /b 0

:build
cargo build --locked --release --target "%~1"
if errorlevel 1 exit /b 1
if "%~1"=="x86_64-pc-windows-msvc" set "BINARY=wrapper-windows-x86-64.exe"
if "%~1"=="aarch64-pc-windows-msvc" set "BINARY=wrapper-windows-arm-64.exe"
copy /y "%CARGO_TARGET_DIR%\%~1\release\wrapper.exe" "dist\boot\%BINARY%" >nul
exit /b %ERRORLEVEL%

:clean
if not "%~2"=="" (
    echo Usage: build.bat clean 1>&2
    goto failed
)
echo Cleaning build...
cargo clean
if errorlevel 1 goto failed
powershell.exe -NoProfile -Command "$ErrorActionPreference = 'Stop'; $root = (Get-Location).ProviderPath; foreach ($name in @('build', 'dist')) { $path = [IO.Path]::GetFullPath((Join-Path $root $name)); if (-not $path.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Invalid clean path' }; if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Recurse -Force } }"
if errorlevel 1 goto failed
echo Build cleaned
popd
exit /b 0

:failed
popd
exit /b 1

:usage
echo Usage: build.bat [x86_64-pc-windows-msvc] [aarch64-pc-windows-msvc]
echo        build.bat clean
echo Without arguments, builds for the Rust host. Outputs go to dist\boot.
exit /b 0
