#ifndef RouterVersion
  #error RouterVersion is required
#endif
#ifndef RouterArchitecture
  #error RouterArchitecture is required
#endif
#ifndef RouterPayload
  #error RouterPayload is required
#endif

[Setup]
AppId={{01A1FBE7-EC4F-78DF-865A-B7B83339A86F}
AppName=Provider Router
AppVersion={#RouterVersion}
VersionInfoVersion={#RouterVersion}
DefaultDirName={localappdata}\Programs\ProviderRouter
DisableDirPage=yes
DisableProgramGroupPage=yes
DefaultGroupName=Provider Router
UninstallDisplayIcon={app}\provider-router.exe
PrivilegesRequired=lowest
ArchitecturesAllowed={#RouterArchitecture}
ArchitecturesInstallIn64BitMode={#RouterArchitecture}
CloseApplications=no
RestartApplications=no
Compression=lzma2
SolidCompression=yes
#ifdef RouterSigning
SignTool=providerRouter
SignedUninstaller=yes
#endif

[Files]
Source: "{#RouterPayload}"; Flags: dontcopy noencryption

[Icons]
Name: "{userprograms}\Provider Router"; Filename: "{app}\provider-router.exe"; WorkingDir: "{app}"

[UninstallDelete]
Type: files; Name: "{app}\provider-router.exe"

[Code]
function RunLifecycle(const Executable, Arguments: String): String;
var
  ExitCode: Integer;
  Captured: TExecOutput;
begin
  Result := '';
  try
    if not ExecAndCaptureOutput(Executable, Arguments, ExtractFileDir(Executable),
      SW_SHOWNORMAL, ewWaitUntilTerminated, ExitCode, Captured) then
      Result := 'Could not start Provider Router. ' + SysErrorMessage(ExitCode)
    else if ExitCode <> 0 then begin
      Result := Trim(StringJoin(#13#10, Captured.StdErr));
      if Result = '' then
        Result := 'Provider Router setup failed with exit code ' + IntToStr(ExitCode) + '.';
    end;
  except
    Result := GetExceptionMessage;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Destination: String;
begin
  Destination := ExpandConstant('{app}\provider-router.exe');
  try
    ExtractTemporaryFile('provider-router-payload.exe');
    Result := RunLifecycle(ExpandConstant('{tmp}\provider-router-payload.exe'),
      '__install --destination "' + Destination + '"');
  except
    Result := GetExceptionMessage;
  end;
end;

function InitializeUninstall: Boolean;
var
  Executable, Failure: String;
begin
  Executable := ExpandConstant('{app}\provider-router.exe');
  Result := True;
  if FileExists(Executable) then begin
    Failure := RunLifecycle(Executable, '__uninstall');
    Result := Failure = '';
    if not Result then
      SuppressibleMsgBox(Failure, mbError, MB_OK, IDOK);
  end;
end;
