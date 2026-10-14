# Run in an elevated PowerShell on a disposable Windows test host with JDK 17+.
# Uses a unique service name and always attempts removal in finally.
$ErrorActionPreference = 'Stop'
$project = Split-Path $PSScriptRoot -Parent
$root = Join-Path ([System.IO.Path]::GetTempPath()) "Wrapper service test $PID"
$name = "wrapper-test-$PID"
$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
$binaryName = if ($arch -eq 'ARM64') { 'wrapper-windows-arm-64.exe' } else { 'wrapper-windows-x86-64.exe' }
$java = (Get-Command java).Source
$javac = (Get-Command javac).Source
New-Item -ItemType Directory -Force "$root/boot", "$root/bin", "$root/conf", "$root/logs" | Out-Null
Copy-Item "$project/dist/boot/$binaryName" "$root/boot/"
Copy-Item "$project/dist/boot/wrapper.jar" "$root/boot/"
Copy-Item "$project/resources/AppCommand.bat.in" "$root/bin/service.bat"
$binary = "$root/boot/$binaryName"
$config = "$root/conf/wrapper.conf"
& $javac --release 17 -cp "$root/boot/wrapper.jar" -d "$root/boot" "$PSScriptRoot/Fixture.java"
if ($LASTEXITCODE -ne 0) { throw 'Fixture compilation failed' }
$source = @"
#encoding=UTF-8
wrapper.name=$name
wrapper.displayname=Wrapper acceptance test $PID
wrapper.java.command=$java
wrapper.java.classpath.1=.
wrapper.app.parameter.1=Fixture
wrapper.java.additional.1=-Dfixture.root=$root
wrapper.java.additional.2=-Dfixture.value=before
wrapper.shutdown.timeout=10
wrapper.restart.delay=0
wrapper.ntservice.starttype=DEMAND_START
"@
if ($env:WRAPPER_TEST_SHARE) {
    # Optional SMB acceptance in SCM's logon session. Use a disposable writable
    # share accessible to LocalSystem, and a drive not used by another service.
    $share = $env:WRAPPER_TEST_SHARE.Replace('#', '##')
    $drive = if ($env:WRAPPER_TEST_SHARE_DRIVE) { $env:WRAPPER_TEST_SHARE_DRIVE } else { 'W:' }
    if ($drive -notmatch '^[A-Za-z]:$') { throw 'WRAPPER_TEST_SHARE_DRIVE must be a drive letter such as W:' }
    $source += @"

wrapper.share.1.location=$share
wrapper.share.1.target=$drive
wrapper.share.1.startup.failure=SHUTDOWN
wrapper.share.1.startup.premapped=SHUTDOWN
wrapper.share.1.startup.max_retries=1
wrapper.share.1.shutdown.unmap=TRUE
wrapper.java.additional.3=-Dfixture.share=$drive/
"@
}
[System.IO.File]::WriteAllText($config, $source, [System.Text.UTF8Encoding]::new($false))
function Invoke-Wrapper([string] $command) {
    & "$root/bin/service.bat" $command
    if ($LASTEXITCODE -ne 0) { throw "Wrapper $command failed: $LASTEXITCODE" }
}
function Wait-Event([string] $text) {
    $until = (Get-Date).AddSeconds(30)
    do {
        if ((Test-Path "$root/events") -and (Get-Content "$root/events" | Select-String -SimpleMatch $text)) { return }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $until)
    throw "Timed out waiting for $text"
}
function Get-StartResult {
    # Windows PowerShell can turn redirected native stderr into ErrorRecords.
    $ErrorActionPreference = 'Continue'
    $output = (& "$root/bin/service.bat" start 2>&1 | Out-String)
    return @{ Output = $output; ExitCode = $LASTEXITCODE }
}
try {
    Invoke-Wrapper install
    foreach ($timeout in @('invalid', '-1')) {
        & $binary start $config "wrapper.startup.wait.timeout=$timeout"
        if ($LASTEXITCODE -eq 0) { throw "Invalid startup wait timeout was accepted: $timeout" }
        if ((Get-Service $name).Status -ne 'Stopped') { throw 'Invalid startup wait timeout started the service' }
    }
    Invoke-Wrapper start
    Invoke-Wrapper status
    Wait-Event before
    [System.IO.File]::WriteAllText($config, $source.Replace('value=before', 'value=after'), [System.Text.UTF8Encoding]::new($false))
    New-Item "$root/restart" -ItemType File | Out-Null
    Wait-Event after
    if ((Get-Service $name).Status -ne 'Running') { throw 'SCM lost Running state during JVM restart' }
    Invoke-Wrapper stop
    if ((Get-Service $name).Status -ne 'Stopped') { throw 'SCM did not report Stopped' }
    if (@(Get-Content "$root/events" | Select-String '^stop ').Count -ne 2) { throw 'Shutdown hooks were not called' }
    Invoke-Wrapper start
    Invoke-Wrapper stop
    foreach ($logging in @('INFO', 'NONE')) {
        foreach ($failure in @(
            @{ Setting = "wrapper.java.command=$root/missing-java.exe"; Diagnostic = 'missing-java' },
            @{ Setting = 'wrapper.java.additional.20=-Xmxinvalid-fixture-size'; Diagnostic = 'invalid-fixture-size' },
            @{ Setting = 'wrapper.java.version.min=999'; Diagnostic = 'wrapper.java.version.min' }
        )) {
            $content = $source + "`nwrapper.logfile.loglevel=$logging`n" + $failure.Setting + "`n"
            [System.IO.File]::WriteAllText($config, $content, [System.Text.UTF8Encoding]::new($false))
            $result = Get-StartResult
            $output = $result.Output
            if ($result.ExitCode -eq 0) { throw "Startup failure reported success: $output" }
            if (!$output.Contains('Starting service...') -or !$output.Contains($failure.Diagnostic) -or $output.Contains('Service started')) {
                throw "Missing terminal startup diagnostic: $output"
            }
            $until = (Get-Date).AddSeconds(15)
            while ((Get-Service $name).Status -ne 'Stopped') {
                if ((Get-Date) -gt $until) { throw 'Failed service did not stop' }
                Start-Sleep -Milliseconds 100
            }
        }
    }
    [System.IO.File]::WriteAllText($config, $source, [System.Text.UTF8Encoding]::new($false))
    $result = Get-StartResult
    $output = $result.Output
    if ($result.ExitCode -ne 0 -or !$output.Contains('Starting service...') -or !$output.Contains('Service started')) {
        throw "Missing startup confirmation: $output"
    }
    Invoke-Wrapper stop
    Write-Host 'Windows service acceptance test passed'
} finally {
    & $binary remove $config
    if ($LASTEXITCODE -eq 0) { Remove-Item -Recurse -Force $root }
    else { Write-Warning "Service cleanup failed. Remove service $name and inspect $root." }
}
