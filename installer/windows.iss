; Inno Setup script -- builds the Windows installer.
;
; Build locally with:
;   iscc installer\windows.iss
; Override the version at build time (this is what CI does):
;   iscc /DMyAppVersion=1.2.3 installer\windows.iss
;
; Expects the release binary at target\release\egui_template.exe,
; so run `cargo build --release` first. Output lands in dist\.

#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

#define MyAppName "egui Template"
#define MyAppPublisher "Your Name"
#define MyAppURL "https://github.com/your-name/egui_template"
#define MyAppExeName "egui_template.exe"

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
AppId={{4B1D9E77-2C3A-4F86-9A15-6E8D0B27C914}
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
OutputBaseFilename=egui-template-{#MyAppVersion}-windows-x64-setup
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
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\resources\icon.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\icon.ico"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\icon.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent

; --- Optional: file associations ---
; Uncomment, add `ChangesAssociations=yes` to [Setup], and add an "associate" task
; to wire your own extensions up to the app:
;
; [Registry]
; Root: HKA; Subkey: "Software\Classes\EguiTemplate.File"; ValueType: string; ValueName: ""; ValueData: "egui Template file"; Flags: uninsdeletekey; Tasks: associate
; Root: HKA; Subkey: "Software\Classes\EguiTemplate.File\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#MyAppExeName},0"; Tasks: associate
; Root: HKA; Subkey: "Software\Classes\EguiTemplate.File\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""; Tasks: associate
; Root: HKA; Subkey: "Software\Classes\.myext"; ValueType: string; ValueName: ""; ValueData: "EguiTemplate.File"; Flags: uninsdeletevalue; Tasks: associate
