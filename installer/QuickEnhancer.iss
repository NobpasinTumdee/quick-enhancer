; Quick Enhancer — Inno Setup 6 script.
; Build:  ISCC installer\QuickEnhancer.iss        (after `cargo build --release`)
;   or    scripts\release.ps1                     (build + installer + checksum + latest.json)
; The version comes from the exe's version resource (Cargo.toml is the single source of truth).
; The file is UTF-8 with BOM (needed for the Thai messages).

#define AppName "Quick Enhancer"
#define AppExe "QuickEnhancer.exe"
#define ExePath AddBackslash(SourcePath) + "..\target\release\" + AppExe
#ifndef AppVersion
  #define AppVersion GetStringFileInfo(ExePath, "ProductVersion")
#endif
; PLACEHOLDERS — set before publishing (see docs/RELEASING.md).
#define AppPublisher "Quick Enhancer Publisher"
#define AppRepoUrl "https://github.com/OWNER/REPO"

[Setup]
; Never change AppId: upgrades and uninstall are keyed on it.
AppId={{525D3D6D-383C-4F5F-AF9D-6B9222E7476D}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppRepoUrl}
AppSupportURL={#AppRepoUrl}/issues
AppUpdatesURL={#AppRepoUrl}/releases
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoCompany={#AppPublisher}
; Per-user install by default (no admin prompt); the dialog offers "all users".
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\QuickEnhancer
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
InfoBeforeFile=..\licenses\THIRD_PARTY.md
OutputDir=..\dist
OutputBaseFilename=QuickEnhancer-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
; The update flow runs the installer while the app is closing.
CloseApplications=yes
RestartApplications=no
; Code signing (optional, see docs/RELEASING.md). Define a sign tool named "qesign" in the
; Inno Setup IDE (Tools → Configure Sign Tools) or pass /Sqesign=... to ISCC, then uncomment:
;SignTool=qesign
;SignedUninstaller=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "thai"; MessagesFile: "compiler:Languages\Thai.isl"

[CustomMessages]
english.DesktopIcon=Create a &desktop shortcut
thai.DesktopIcon=สร้างทางลัดบน&เดสก์ท็อป
english.DownloadTools=Downloading the external tools (FFmpeg, Real-ESRGAN, RIFE)…
thai.DownloadTools=กำลังดาวน์โหลดเครื่องมือภายนอก (FFmpeg, Real-ESRGAN, RIFE)…
english.LaunchApp=Launch Quick Enhancer
thai.LaunchApp=เปิด Quick Enhancer
english.DeleteData=Also delete your Quick Enhancer settings, history and downloaded tools?%n%n%1%n%2%n%nChoose No to keep them (recommended if you plan to reinstall).
thai.DeleteData=ลบค่าตั้ง ประวัติ และเครื่องมือที่ดาวน์โหลดไว้ของ Quick Enhancer ด้วยหรือไม่?%n%n%1%n%2%n%nเลือก “ไม่” เพื่อเก็บไว้ (แนะนำถ้าจะติดตั้งใหม่)

[Tasks]
Name: "desktopicon"; Description: "{cm:DesktopIcon}"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
; Interactive install: let the user pick and download the tools (checksums verified by the
; app). Runs as the user who started setup so tools land in their %LOCALAPPDATA%.
Filename: "{app}\{#AppExe}"; Parameters: "--install-tools"; StatusMsg: "{cm:DownloadTools}"; Flags: waituntilterminated skipifsilent runasoriginaluser
; Interactive install: optional launch from the Finish page.
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchApp}"; Flags: nowait postinstall skipifsilent runasoriginaluser
; Silent update from inside the app: relaunch it when done.
Filename: "{app}\{#AppExe}"; Flags: nowait postinstall skipifnotsilent runasoriginaluser

[Code]
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir, ToolsDir: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    DataDir := ExpandConstant('{userappdata}\QuickEnhancer');
    ToolsDir := ExpandConstant('{localappdata}\QuickEnhancer');
    if (not UninstallSilent) and (DirExists(DataDir) or DirExists(ToolsDir)) then
    begin
      { Default button is No: data is kept unless the user explicitly says Yes. }
      if MsgBox(FmtMessage(CustomMessage('DeleteData'), [DataDir, ToolsDir]),
                mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      begin
        DelTree(DataDir, True, True, True);
        DelTree(ToolsDir, True, True, True);
      end;
    end;
  end;
end;
