@echo off
setlocal EnableExtensions DisableDelayedExpansion
if not "%~2"=="" goto invalid
if /i "%~1"=="--help" goto usage
if /i "%~1"=="-h" goto usage
set "WRAPPER_INSTALL_ALL="
if /i "%~1"=="--all" set "WRAPPER_INSTALL_ALL=1"
if not "%~1"=="" if not defined WRAPPER_INSTALL_ALL goto invalid
set "WRAPPER_INSTALL_SCRIPT=%~f0"
rem Keep the installer in this file; PowerShell handles discovery and native errors.
powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "$source = Get-Content -LiteralPath $env:WRAPPER_INSTALL_SCRIPT -Raw; & ([scriptblock]::Create(($source -split '(?m)^# POWERSHELL INSTALLER\r?$', 2)[1]))"
exit /b %ERRORLEVEL%

:invalid
call :usage
exit /b 1

:usage
echo Usage: install.bat [--all]
echo Install tools for build.bat on Windows amd64 or arm64 using WinGet.
echo --all installs tools and Rust targets for both Windows architectures.
echo Run as your normal user; system installers request elevation when needed.
exit /b 0

# POWERSHELL INSTALLER
$ErrorActionPreference = 'Stop'

function Refresh-Path([switch] $AfterInstall) {
    $savedPath = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')
    $env:Path = if ($AfterInstall) { $savedPath + ';' + $env:Path } else { $env:Path + ';' + $savedPath }
    $cargoDir = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    $env:Path = (Join-Path $cargoDir 'bin') + ';' + $env:Path
}

function Invoke-Checked([string] $program, [string[]] $arguments) {
    & $program @arguments
    if ($LASTEXITCODE -ne 0) { throw "$program failed with exit code $LASTEXITCODE" }
}

function Install-Package([string] $id, [string[]] $extra = @()) {
    if (-not (Get-Command winget.exe -ErrorAction SilentlyContinue)) {
        throw 'Install App Installer (WinGet), then rerun install.bat.'
    }
    Invoke-Checked winget.exe (@('install', '--exact', '--id', $id, '--source', 'winget', '--accept-package-agreements', '--accept-source-agreements') + $extra)
    Refresh-Path -AfterInstall
}

function Test-Java {
    $ErrorActionPreference = 'Continue'
    if (-not (Get-Command javac.exe -ErrorAction SilentlyContinue) -or -not (Get-Command jar.exe -ErrorAction SilentlyContinue)) { return $false }
    $version = & javac.exe -version 2>&1 | Out-String
    return $LASTEXITCODE -eq 0 -and $version -match 'javac (\d+)' -and [int]$Matches[1] -ge 17
}

function Test-Python {
    $ErrorActionPreference = 'Continue'
    $python = Get-Command python.exe -ErrorAction SilentlyContinue
    if (-not $python -or $python.Source -like '*\Microsoft\WindowsApps\python.exe') { return $false }
    & python.exe -c 'import sys; sys.exit(sys.version_info < (3, 11))' 2>$null
    return $LASTEXITCODE -eq 0
}

try {
    Set-Location -LiteralPath (Split-Path -Parent $env:WRAPPER_INSTALL_SCRIPT)
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    $target = switch ($arch) {
        'AMD64' { 'x86_64-pc-windows-msvc' }
        'ARM64' { 'aarch64-pc-windows-msvc' }
        default { throw 'Only Windows amd64 and arm64 hosts are supported.' }
    }
    Refresh-Path
    if ($env:JAVA_HOME) { $env:Path = (Join-Path $env:JAVA_HOME 'bin') + ';' + $env:Path }
    if (-not (Test-Java)) { Install-Package Microsoft.OpenJDK.17 }
    if (-not (Test-Python)) { Install-Package Python.Python.3.13 }
    if (-not (Test-Java)) { throw 'JDK 17+ must be on PATH. Open a new terminal after installation and rerun install.bat.' }
    if (-not (Test-Python)) { throw 'Python 3.11+ must be on PATH. Open a new terminal after installation and rerun install.bat.' }

    $components = @('Microsoft.VisualStudio.Component.VC.Tools.x86.x64')
    if ($arch -eq 'ARM64' -or $env:WRAPPER_INSTALL_ALL) {
        $components += 'Microsoft.VisualStudio.Component.VC.Tools.ARM64'
    }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $vsArgs = @('-latest', '-products', '*', '-version', '[17.0,)', '-property', 'installationPath')
    $ready = if (Test-Path -LiteralPath $vswhere) { & $vswhere @vsArgs -requires $components }
    if (-not $ready) {
        $options = '--passive --norestart --includeRecommended --add Microsoft.VisualStudio.Workload.VCTools'
        foreach ($component in $components) { $options += " --add $component" }
        $existing = if (Test-Path -LiteralPath $vswhere) { & $vswhere @vsArgs }
        if ($existing) {
            $setup = Join-Path (Split-Path -Parent $vswhere) 'setup.exe'
            $process = Start-Process -FilePath $setup -ArgumentList "modify --installPath `"$existing`" $options" -Verb RunAs -WindowStyle Hidden -Wait -PassThru
            if ($process.ExitCode -notin @(0, 3010)) { throw "Visual Studio installation failed with exit code $($process.ExitCode)" }
        } else {
            Install-Package Microsoft.VisualStudio.2022.BuildTools @('--override', "--wait $options")
        }
        $ready = & $vswhere @vsArgs -requires $components
        if (-not $ready) { throw 'MSVC tools are unavailable. Complete installation or reboot, then rerun install.bat.' }
    }

    if (-not (Get-Command rustup.exe -ErrorAction SilentlyContinue)) {
        Install-Package Rustlang.Rustup @('--override', '-y --default-toolchain none')
    }
    $toolchain = Get-Content -LiteralPath rust-toolchain.toml -Raw
    if ($toolchain -notmatch '(?m)^channel\s*=\s*"([^"]+)"') { throw 'Cannot read Rust version from rust-toolchain.toml.' }
    $version = $Matches[1]
    Invoke-Checked rustup.exe @('toolchain', 'install', $version, '--profile', 'minimal', '--component', 'rustfmt', '--component', 'clippy')
    $targets = @($target)
    if ($env:WRAPPER_INSTALL_ALL) { $targets = @('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc') }
    Invoke-Checked rustup.exe (@('target', 'add', '--toolchain', $version) + $targets)
    Write-Host 'Build tools installed. Run .\build.bat'
} catch {
    [Console]::Error.WriteLine("Error: $($_.Exception.Message)")
    exit 1
}
