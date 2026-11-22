param([Parameter(Mandatory)][string]$Destination)

$ErrorActionPreference = 'Stop'
$version = '1.28.9'
$name = "gstreamer-1.0-msvc-x86_64-$version.exe"
$expected = 'ce4008705e8c6fe92803535b82fb3d6551e183f438e6ef26682ac4d0aca61076'
$installer = Join-Path ([System.IO.Path]::GetTempPath()) $name
if (!(Test-Path $installer) -or (Get-FileHash $installer).Hash -ne $expected) {
    Invoke-WebRequest "https://gstreamer.freedesktop.org/pkg/windows/$version/msvc/$name" -OutFile $installer
}
if ((Get-FileHash $installer).Hash -ne $expected) { throw 'GStreamer installer checksum mismatch' }
$process = Start-Process $installer -ArgumentList '/SP-', '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER', '/TYPE=devel', "/DIR=`"$Destination`"" -PassThru -Wait
if ($process.ExitCode -ne 0) { throw "GStreamer installer exited with $($process.ExitCode)" }
if (!(Test-Path "$Destination\lib\pkgconfig\gstreamer-1.0.pc")) {
    throw "GStreamer development files are missing from $Destination"
}
if (!(Test-Path "$Destination\lib\pkgconfig\gstreamer-webrtc-1.0.pc")) {
    throw "GStreamer WebRTC development files are missing from $Destination"
}
