; YAAC's Windows installer (Inno Setup 6). Per user, no administrator rights.
; Build it with scripts/build-installer.sh, which passes every path and the version:
;
;   ISCC.exe /DAppVersion=0.1.0 /DNumericVersion=0.1.0 /DSourceDir=<dir with yaac.exe>
;            /DIconFile=<yaac.ico> /DOutputDir=<dir> installer\yaac.iss
;
; What it does, and why, is in docs/install.md. In short:
; - files go to %LOCALAPPDATA%\Programs\YAAC (no directory page; /DIR= still works);
; - before replacing or removing yaac.exe, the installed build runs `yaac quit`, which closes
;   the window and stops the hub gracefully (hub/shutdown) and waits until the file is free;
; - the hub's login entry is written by YAAC itself (`yaac autostart sync --start`), never by
;   this script, so the two cannot disagree;
; - uninstalling keeps the user's settings and data unless asked (/REMOVEDATA=yes when silent);
; - /RELAUNCH starts the installed yaac.exe when setup finishes, even when silent: the in-app
;   updater runs `setup /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /RELAUNCH` (docs/install.md).

#ifndef AppVersion
  #error Pass /DAppVersion=<version> (scripts/build-installer.sh does)
#endif
#ifndef NumericVersion
  #define NumericVersion AppVersion
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<the directory holding yaac.exe>
#endif
#ifndef IconFile
  #error Pass /DIconFile=<yaac.ico>
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
; Never change the AppId: upgrades and the uninstaller find the installed copy by it.
AppId={{68D966F3-0510-46E4-9581-B0279B70C15D}
AppName=YAAC
AppVersion={#AppVersion}
AppVerName=YAAC {#AppVersion}
AppPublisher=YAAC
VersionInfoVersion={#NumericVersion}
VersionInfoProductName=YAAC
VersionInfoDescription=YAAC Setup
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\YAAC
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
UsePreviousAppDir=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=YAAC-Setup-{#AppVersion}
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\yaac.exe
UninstallDisplayName=YAAC
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; `yaac quit` (PrepareToInstall) normally frees yaac.exe first; the Restart Manager is only the
; fallback for a copy that did not stop. YAAC is never restarted by it: the hub is started
; by YAAC's own command below and the window by the finish page.
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\cef\*"; DestDir: "{app}\cef"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#SourceDir}\yaac.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\yaacd.mjs"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\runtime\node.exe"; DestDir: "{app}\runtime"; Flags: ignoreversion
Source: "{#SourceDir}\runtime\pty-1.2.0-beta.18\*"; DestDir: "{app}\runtime\pty-1.2.0-beta.18"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#SourceDir}\runtime\LICENSE-Node.js.txt"; DestDir: "{app}\runtime"; Flags: ignoreversion
Source: "{#SourceDir}\wpf\*"; DestDir: "{app}\wpf"; Flags: ignoreversion recursesubdirs createallsubdirs skipifsourcedoesntexist

[Icons]
Name: "{autoprograms}\YAAC"; Filename: "{app}\yaac.exe"
Name: "{autodesktop}\YAAC"; Filename: "{app}\yaac.exe"; Tasks: desktopicon

[Run]
; The hub's login entry points at this copy (replacing one a development build registered)
; when the hub's "start at login" setting is on, and the hub is started again as login would.
; Before the first run of the window nothing is written: the window's first run does it.
Filename: "{app}\yaac.exe"; Parameters: "autostart sync --start"; WorkingDir: "{app}"; Flags: runhidden; StatusMsg: "Starting the YAAC hub..."
Filename: "{app}\yaac.exe"; Description: "{cm:LaunchProgram,YAAC}"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent; Check: not RelaunchRequested
; The in-app updater's /RELAUNCH: an entry without `postinstall` runs in silent mode too, after
; the hub entry above. Only after a successful install: a failed setup runs no [Run] entry.
Filename: "{app}\yaac.exe"; WorkingDir: "{app}"; Flags: nowait; Check: RelaunchRequested

[Code]
const
  RemoveDataQuestion = 'Also remove YAAC''s settings and data?' + #13#10 + #13#10 +
    'This deletes, on this computer only, YAAC''s targets and settings, its cache and drafts, ' +
    'and the SSH keys YAAC generated for its own connections. Nothing on your targets and ' +
    'nothing in your own .ssh folder is touched.' + #13#10 + #13#10 +
    'Choose No to keep them for a later install.';

{ Whether setup was started with /RELAUNCH (by YAAC's updater). }
function RelaunchRequested: Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/RELAUNCH') = 0 then
      Result := True;
end;

{ Runs the installed yaac.exe with Params from a copy in the temporary folder, so the
  installed file itself is not held open. Returns the exit code, or -1 if it did not run. }
function RunInstalled(Params: String): Integer;
var
  Installed, ToolExe: String;
  Code: Integer;
begin
  Result := -1;
  Installed := ExpandConstant('{app}\yaac.exe');
  if not FileExists(Installed) then
    Exit;
  ToolExe := ExpandConstant('{tmp}\yaac-tool.exe');
  if not FileExists(ToolExe) then
    if not CopyFile(Installed, ToolExe, False) then begin
      Log('could not copy ' + Installed + ' to ' + ToolExe);
      Exit;
    end;
  if Exec(ToolExe, Params, ExpandConstant('{tmp}'), SW_HIDE, ewWaitUntilTerminated, Code) then
    Result := Code
  else
    Log('could not run yaac ' + Params + ': ' + SysErrorMessage(Code));
  Log('yaac ' + Params + ' -> ' + IntToStr(Result));
end;

{ Closes the window and stops the hub (gracefully: targets keep running their turns) and
  waits until yaac.exe can be replaced. }
procedure StopYaac;
begin
  RunInstalled('quit --wait-for "' + ExpandConstant('{app}\yaac.exe') + '"');
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopYaac;
  Result := '';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  RemoveData: Boolean;
begin
  if CurUninstallStep <> usUninstall then
    Exit;
  if UninstallSilent then
    RemoveData := CompareText(ExpandConstant('{param:REMOVEDATA|no}'), 'yes') = 0
  else
    RemoveData := MsgBox(RemoveDataQuestion, mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES;
  StopYaac;
  RunInstalled('autostart off');
  if RemoveData then
    RunInstalled('remove-data');
end;
