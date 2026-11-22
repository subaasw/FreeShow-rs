; SPDX-License-Identifier: GPL-3.0-only
[Setup]
AppId=org.freeshow.rust
AppName=FreeShow Rust
AppVersion={#AppVersion}
AppPublisher=FreeShow Rust contributors
DefaultDirName={localappdata}\Programs\FreeShow Rust
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
UninstallDisplayIcon={app}\freeshow.ico
SetupIconFile={#Payload}\freeshow.ico
LicenseFile={#Payload}\share\LICENSE
Compression=lzma2
SolidCompression=yes
OutputBaseFilename=freeshow-rust-{#AppVersion}-windows-x86_64-setup

[Files]
Source: "{#Payload}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{userprograms}\FreeShow Rust"; Filename: "{app}\freeshow.exe"; WorkingDir: "{app}"; IconFilename: "{app}\freeshow.ico"
