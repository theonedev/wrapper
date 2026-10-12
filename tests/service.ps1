# Run in an elevated PowerShell on a disposable Windows test host with JDK 17+.
# Uses a unique service name and always attempts removal in finally.
$ErrorActionPreference = 'Stop'
$project = Split-Path $PSScriptRoot -Parent
$root = Join-Path ([System.IO.Path]::GetTempPath()) "ServiceWrapper service test $PID"
$name = "servicewrapper-test-$PID"
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
wrapper.displayname=ServiceWrapper acceptance test $PID
wrapper.java.command=$java
wrapper.java.classpath.1=.
wrapper.app.parameter.1=Fixture
wrapper.java.additional.1=-Dfixture.root=$root
wrapper.java.additional.2=-Dfixture.value=before
wrapper.shutdown.timeout=10
wrapper.restart.delay=0
wrapper.ntservice.starttype=DEMAND_START
"@
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
    Write-Host 'Windows service acceptance test passed'
} finally {
    & $binary remove $config
    if ($LASTEXITCODE -eq 0) { Remove-Item -Recurse -Force $root }
    else { Write-Warning "Service cleanup failed. Remove service $name and inspect $root." }
}
