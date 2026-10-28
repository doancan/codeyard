; Codeyard Windows installer (Inno Setup 6).
;   iscc /DAppVersion=0.1.0 installer\windows\codeyard.iss
; Input: target\release\codeyard.exe, the licence texts (installed to {app}\licenses, same list as
; scripts\licenses.sh). SSH agents are embedded in Codeyard.exe by scripts/build-windows.ps1.
; Output: target\windows\Codeyard-<version>-windows-x64-setup.exe
; Normally run by scripts\bundle-windows.ps1.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
; Fixed ID so every version updates the same uninstall entry.
AppId={{6C1F3B0E-5B7D-4E4F-9B52-7D2A8C0E1A41}
AppName=Codeyard
AppVersion={#AppVersion}
AppPublisher=Codeyard
DefaultDirName={autopf}\Codeyard
DefaultGroupName=Codeyard
UninstallDisplayIcon={app}\Codeyard.exe
SetupIconFile=..\..\crates\app\assets\windows\codeyard.ico
OutputDir=..\..\target\windows
OutputBaseFilename=Codeyard-{#AppVersion}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; No admin rights needed: per user by default, the user can pick "all users".
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
WizardStyle=modern
DisableProgramGroupPage=yes
ChangesEnvironment=yes
ChangesAssociations=no
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked
Name: "addtopath"; Description: "Add Codeyard to &PATH (so `codeyard .` works in a terminal)"; GroupDescription: "Command line:"
Name: "contextmenu"; Description: "Add ""Open with Codeyard"" to the Explorer context menu"; GroupDescription: "Explorer:"

[Files]
Source: "..\..\target\release\codeyard.exe"; DestDir: "{app}"; DestName: "Codeyard.exe"; Flags: ignoreversion
Source: "..\..\target\release\cyx.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\crates\app\assets\windows\codeyard.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-MIT"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "..\..\LICENSE-APACHE"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "..\..\THIRD_PARTY_NOTICES.md"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "..\..\THIRD_PARTY_CRATES.md"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "..\..\crates\app\assets\icons\LICENSE"; DestDir: "{app}\licenses"; DestName: "icons-LICENSE"; Flags: ignoreversion
Source: "..\..\crates\syntax\grammars\dockerfile\LICENSE"; DestDir: "{app}\licenses"; DestName: "dockerfile-grammar-LICENSE"; Flags: ignoreversion
Source: "..\..\vendor\gpui\LICENSE-APACHE"; DestDir: "{app}\licenses"; DestName: "gpui-LICENSE-APACHE"; Flags: ignoreversion
Source: "..\..\vendor\xattr\LICENSE-MIT"; DestDir: "{app}\licenses"; DestName: "xattr-LICENSE-MIT"; Flags: ignoreversion
Source: "..\..\vendor\xattr\LICENSE-APACHE"; DestDir: "{app}\licenses"; DestName: "xattr-LICENSE-APACHE"; Flags: ignoreversion
Source: "..\..\crates\spell\dicts\en\license"; DestDir: "{app}\licenses"; DestName: "spell-dict-en-license"; Flags: ignoreversion
Source: "..\..\crates\syntax\LICENSES\*"; DestDir: "{app}\licenses\syntax-LICENSES"; Flags: ignoreversion

[Icons]
Name: "{group}\Codeyard"; Filename: "{app}\Codeyard.exe"; IconFilename: "{app}\codeyard.ico"
Name: "{autodesktop}\Codeyard"; Filename: "{app}\Codeyard.exe"; IconFilename: "{app}\codeyard.ico"; Tasks: desktopicon

[Registry]
; Explorer right-click > "Open with Codeyard" on folders and files (per user or all users, by install mode).
Root: HKA; Subkey: "Software\Classes\Directory\shell\Codeyard"; ValueType: string; ValueName: ""; ValueData: "Open with Codeyard"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\shell\Codeyard"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\Codeyard.exe"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\Codeyard\command"; ValueType: string; ValueName: ""; ValueData: """{app}\Codeyard.exe"" ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\Codeyard"; ValueType: string; ValueName: ""; ValueData: "Open with Codeyard"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\Codeyard"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\Codeyard.exe"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\Codeyard\command"; ValueType: string; ValueName: ""; ValueData: """{app}\Codeyard.exe"" ""%V"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\*\shell\Codeyard"; ValueType: string; ValueName: ""; ValueData: "Open with Codeyard"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\*\shell\Codeyard"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\Codeyard.exe"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\*\shell\Codeyard\command"; ValueType: string; ValueName: ""; ValueData: """{app}\Codeyard.exe"" ""%1"""; Tasks: contextmenu
; PATH: HKCU in per-user mode, HKLM in all-users mode.
Root: HKA; Subkey: "{code:EnvKey}"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\Codeyard.exe"; Description: "Launch Codeyard"; Flags: nowait postinstall skipifsilent
; In-app update (crates/app/src/update_install.rs runs Setup with /SILENT ... /relaunch=1): start the new version once.
; Setup's own restart of closed applications is turned off there (/NORESTARTAPPLICATIONS), so this is the only launch.
; `--after-update` makes the new version reopen the windows the old one left (crates/app/src/update_restart.rs).
Filename: "{app}\Codeyard.exe"; Parameters: "--after-update"; Flags: nowait runasoriginaluser; Check: RelaunchAfterUpdate

[Code]
function RelaunchAfterUpdate: Boolean;
begin
  Result := ExpandConstant('{param:relaunch|0}') = '1';
end;

function EnvKey(Param: string): string;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

function NeedsAddPath(Dir: string): Boolean;
var
  Existing: string;
  Root: Integer;
begin
  if IsAdminInstallMode then Root := HKEY_LOCAL_MACHINE else Root := HKEY_CURRENT_USER;
  if not RegQueryStringValue(Root, EnvKey(''), 'Path', Existing) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Existing) + ';') = 0;
end;

procedure RemoveFromPath(Dir: string);
var
  Existing: string;
  Root, P: Integer;
begin
  if IsAdminInstallMode then Root := HKEY_LOCAL_MACHINE else Root := HKEY_CURRENT_USER;
  if not RegQueryStringValue(Root, EnvKey(''), 'Path', Existing) then exit;
  P := Pos(';' + Uppercase(Dir), Uppercase(Existing));
  if P = 0 then exit;
  Delete(Existing, P, Length(Dir) + 1);
  RegWriteExpandStringValue(Root, EnvKey(''), 'Path', Existing);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
