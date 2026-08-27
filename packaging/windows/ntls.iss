; The Windows installer, built with Inno Setup.
;
; It installs one executable and its shortcuts. There is nothing else to
; install: ntls keeps its workspaces in the user's Documents folder and writes
; nothing to the registry beyond what an uninstaller needs.
;
;   iscc /DVersion=0.1.0 /DBinary=path\to\ntls.exe packaging\windows\ntls.iss

#ifndef Version
  #define Version "0.1.0"
#endif
#ifndef Binary
  #define Binary "..\..\target\release\ntls.exe"
#endif
#ifndef OutDir
  #define OutDir "..\..\dist"
#endif
#ifndef Arch
  #define Arch "x64"
#endif
; Which machines this build will install on. Passed in rather than worked out
; here, so the installer script has no idea what a target triple is.
#ifndef Architectures
  #define Architectures "x64compatible"
#endif

[Setup]
AppId={{7F6C4E2A-9F3B-4E1D-9E28-6E0B2C4B4E11}
AppName=ntls
AppVersion={#Version}
AppPublisher=ntls
DefaultDirName={autopf}\ntls
DefaultGroupName=ntls
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\ntls.exe
OutputDir={#OutDir}
OutputBaseFilename=ntls-{#Version}-windows-{#Arch}-setup
SetupIconFile=..\icon\ntls.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; Installing for the user alone needs no administrator, which is the right
; default for a program that does not install a service or a driver.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed={#Architectures}
ArchitecturesInstallIn64BitMode={#Architectures}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#Binary}"; DestDir: "{app}"; DestName: "ntls.exe"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; DestName: "README.md"; Flags: ignoreversion
Source: "..\..\LICENSE*"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{group}\ntls"; Filename: "{app}\ntls.exe"
Name: "{autodesktop}\ntls"; Filename: "{app}\ntls.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\ntls.exe"; Description: "Start ntls"; Flags: nowait postinstall skipifsilent
