; Inno Setup script -- builds the Windows installer.
;
; Build locally with:
;   iscc installer\windows.iss
; Override the version at build time (this is what CI does):
;   iscc /DMyAppVersion=1.2.3 installer\windows.iss
;
; Installs the folder scripts/stage-windows.sh lays out (target\stage\windows: the
; executable, OpenBabel's DLLs, plugins and data, and the app-local Visual C++ runtime), so run
; `cargo build --release`, `bash scripts/fetch-openbabel.sh` and `bash scripts/stage-windows.sh`
; first. Override the folder with /DStageDir=... Output lands in dist\.

#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

#define MyAppName "Open DrawChemistry"
#define MyAppPublisher "IndigoCarmine"
#define MyAppURL "https://github.com/IndigoCarmine/open_draw_chemistry"
#define MyAppExeName "open_draw_chemistry.exe"

#ifndef StageDir
  #define StageDir "..\target\stage\windows"
#endif

; "x64compatible" (native x64 + ARM64 running x64 code) is preferred, but it only
; exists on Inno Setup 6.3+; older compilers error on it, so fall back to "x64".
#if Ver >= EncodeVer(6,3,0)
  #define ArchId "x64compatible"
#else
  #define ArchId "x64"
#endif

[Setup]
; A stable AppId is what makes upgrades replace the previous install instead of
; stacking up alongside it. Generate your own GUID and then never change it.
AppId={{6F4D1816-8E52-4698-955B-A19E832D150F}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}/releases
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
; Lets the user pick a per-user install (no admin rights needed) or machine-wide.
PrivilegesRequiredOverridesAllowed=dialog commandline
OutputDir=..\dist
OutputBaseFilename=open-draw-chemistry-{#MyAppVersion}-windows-x64-setup
SetupIconFile=..\resources\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
UninstallDisplayName={#MyAppName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed={#ArchId}
ArchitecturesInstallIn64BitMode={#ArchId}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; The staged app (scripts/stage-windows.sh), the same files as the portable zip: the executable;
; OpenBabel (3D generation, SMILES, 3D files) -- its DLLs and .obf plugins beside the executable,
; since OpenBabel finds the plugins beside openbabel-3.dll, and its data at {app}\data, which is
; where openbabel_rs looks in an installed application; and msvcp140, vcruntime140 and
; vcruntime140_1.dll, the Visual C++ runtime deployed app-local, so that neither a clean machine
; nor a per-user (non-admin) install needs vc_redist.x64.exe.
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\resources\icon.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\icon.ico"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\icon.ico"; Tasks: desktopicon

[Run]
; The OLE server for ChemDraw objects in Word/PowerPoint (per user, HKCU\Software\Classes):
; registers this app's class, and points ChemDraw's class here only when ChemDraw is absent.
Filename: "{app}\{#MyAppExeName}"; Parameters: "--register-ole"; StatusMsg: "Registering the OLE server..."; Flags: runhidden waituntilterminated runasoriginaluser
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Removes exactly what --register-ole wrote.
Filename: "{app}\{#MyAppExeName}"; Parameters: "--unregister-ole"; Flags: runhidden waituntilterminated; RunOnceId: "UnregisterOle"

; --- Optional: file associations ---
; Uncomment, add `ChangesAssociations=yes` to [Setup], and add an "associate" task
; to wire your own extensions up to the app:
;
; [Registry]
; Root: HKA; Subkey: "Software\Classes\OpenDrawChemistry.File"; ValueType: string; ValueName: ""; ValueData: "ChemDraw document"; Flags: uninsdeletekey; Tasks: associate
; Root: HKA; Subkey: "Software\Classes\OpenDrawChemistry.File\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#MyAppExeName},0"; Tasks: associate
; Root: HKA; Subkey: "Software\Classes\OpenDrawChemistry.File\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""; Tasks: associate
; Root: HKA; Subkey: "Software\Classes\.myext"; ValueType: string; ValueName: ""; ValueData: "OpenDrawChemistry.File"; Flags: uninsdeletevalue; Tasks: associate
