; Fenix's Windows installer, built by the release workflow with Inno Setup 6:
;
;   iscc /DAppVersion=2.2.1 /DSourceDir=<folder with fenix.exe> /DOutputDir=<where> packaging\windows\fenix.iss
;
; For you alone by default -- no administrator needed, in
; %LOCALAPPDATA%\Programs\Fenix -- or for everyone on the machine, in
; Program Files, when you pick that. A newer installer upgrades the one
; there (the same AppId), closing Fenix first. Uninstalling leaves your
; settings, sessions and extensions where they are.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{6F3B1C7E-2A4D-4E8B-9C51-FE1A0D3B7C42}
AppName=Fenix
AppVersion={#AppVersion}
AppVerName=Fenix {#AppVersion}
AppPublisher=Thomas Pedneault
AppPublisherURL=https://github.com/tpedneault/fenix
AppSupportURL=https://github.com/tpedneault/fenix/issues
AppUpdatesURL=https://github.com/tpedneault/fenix/releases
DefaultDirName={autopf}\Fenix
DefaultGroupName=Fenix
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\fenix.exe
UninstallDisplayName=Fenix
SetupIconFile=..\..\fenix.ico
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
CloseApplications=yes
RestartApplications=no
ChangesEnvironment=yes
ChangesAssociations=yes
OutputDir={#OutputDir}
OutputBaseFilename=fenix-{#AppVersion}-x86_64-pc-windows-msvc-setup
VersionInfoVersion={#AppVersion}
VersionInfoProductName=Fenix
VersionInfoDescription=Fenix installer

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "path"; Description: "Put fenix on PATH, to start it from a terminal"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "openwith"; Description: "Add ""Open with Fenix"" to Explorer's right-click menu"; Flags: unchecked

[Files]
Source: "{#SourceDir}\fenix.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\fenix-pdfium.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\THIRD-PARTY.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Fenix"; Filename: "{app}\fenix.exe"
Name: "{autodesktop}\Fenix"; Filename: "{app}\fenix.exe"; Tasks: desktopicon

[Registry]
; `fenix` from Win+R and the Start menu's search.
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\fenix.exe"; ValueType: string; ValueName: ""; ValueData: "{app}\fenix.exe"; Flags: uninsdeletekey
; "Open with Fenix" on a file, a folder, and a folder's background.
Root: HKA; Subkey: "Software\Classes\*\shell\Fenix"; ValueType: string; ValueName: ""; ValueData: "Open with Fenix"; Tasks: openwith; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\*\shell\Fenix"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\fenix.exe"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\*\shell\Fenix\command"; ValueType: string; ValueName: ""; ValueData: """{app}\fenix.exe"" ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Directory\shell\Fenix"; ValueType: string; ValueName: ""; ValueData: "Open with Fenix"; Tasks: openwith; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\shell\Fenix"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\fenix.exe"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Directory\shell\Fenix\command"; ValueType: string; ValueName: ""; ValueData: """{app}\fenix.exe"" ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\Fenix"; ValueType: string; ValueName: ""; ValueData: "Open with Fenix"; Tasks: openwith; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\Fenix"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\fenix.exe"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\Fenix\command"; ValueType: string; ValueName: ""; ValueData: """{app}\fenix.exe"" ""%V"""; Tasks: openwith

[Run]
Filename: "{app}\fenix.exe"; Description: "{cm:LaunchProgram,Fenix}"; Flags: nowait postinstall skipifsilent

[Code]
// PATH: yours for an install of your own, the machine's for everyone's.
function EnvironmentKey(var Root: Integer): String;
begin
  if IsAdminInstallMode then begin
    Root := HKEY_LOCAL_MACHINE;
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  end else begin
    Root := HKEY_CURRENT_USER;
    Result := 'Environment';
  end;
end;

function PathHas(const Paths, Dir: String): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') > 0;
end;

procedure AddToPath(const Dir: String);
var
  Root: Integer;
  Key, Paths: String;
begin
  Key := EnvironmentKey(Root);
  if not RegQueryStringValue(Root, Key, 'Path', Paths) then
    Paths := '';
  if PathHas(Paths, Dir) then
    exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then
    Paths := Paths + ';';
  RegWriteExpandStringValue(Root, Key, 'Path', Paths + Dir);
end;

procedure RemoveFromPath(const Dir: String);
var
  Root, At: Integer;
  Key, Paths, Padded: String;
begin
  Key := EnvironmentKey(Root);
  if not RegQueryStringValue(Root, Key, 'Path', Paths) then
    exit;
  Padded := ';' + Paths + ';';
  At := Pos(';' + Uppercase(Dir) + ';', Uppercase(Padded));
  if At = 0 then
    exit;
  Delete(Padded, At, Length(Dir) + 1);
  Paths := Copy(Padded, 2, Length(Padded) - 2);
  RegWriteExpandStringValue(Root, Key, 'Path', Paths);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('path') then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
