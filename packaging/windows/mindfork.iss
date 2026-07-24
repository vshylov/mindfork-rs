; The Inno Setup installer script for mindfork-rs on Windows (§3.3 docs/history/installers.md).
; Two custom wizard pages — "Application language" and "Data location" — and writing the
; choice into defaults.json next to the binary. A bilingual UI (ru/en), a per-user install
; with no UAC. The script is compatible with Inno Setup 6.7.x.
;
; The version and the directory with the binary are passed to the compiler via /D:
;   ISCC.exe /DAppVersion=0.9.0 /DBinDir="C:\path\to\dir-with-exe" packaging\windows\mindfork.iss
; The defaults below let the script compile locally/in CI for a syntax check.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef BinDir
  ; The local default: a release build from the repo root. SourcePath already carries a
  ; trailing "\", so we join without a leading slash.
  #define BinDir SourcePath + "..\..\target\release"
#endif

[Setup]
AppId={{A7E3F1C2-9B84-4D6E-8F1A-2C5B7D9E0463}
AppName=mindfork-rs
AppVersion={#AppVersion}
AppPublisher=Vladimir Shylov
AppPublisherURL=https://github.com/vshylov/mindfork-rs
AppSupportURL=https://github.com/vshylov/mindfork-rs
DefaultDirName={autopf}\mindfork-rs
DefaultGroupName=mindfork-rs
DisableProgramGroupPage=yes
; Per-user by default (no UAC); the user can choose "for everyone" at startup.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#SourcePath}..\..\dist
OutputBaseFilename=mindfork-rs-v{#AppVersion}-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName=mindfork-rs
UninstallDisplayIcon={app}\mindfork-rs.exe
; Branding (docs/branding.md §4.1). The icon of setup.exe itself and the logo in the
; wizard's header. PNG is verified to work on Inno 6 — accepted on par with BMP and 35x lighter.
; The installed .exe's icon is embedded by build.rs (winresource), so the
; [Icons] shortcuts and UninstallDisplayIcon pick it up with no extra settings.
SetupIconFile={#SourcePath}..\..\artwork\mindfork.ico
WizardSmallImageFile={#SourcePath}..\..\artwork\mindfork-wizard-small.png

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
; The "Application language" page.
en.AppLangCaption=Application language
ru.AppLangCaption=Язык приложения
en.AppLangSub=In which language should the application run?
ru.AppLangSub=На каком языке запускать приложение?
en.AppLangPrompt=Choose the interface language. You can change it later in the settings.
ru.AppLangPrompt=Выберите язык интерфейса. Его можно сменить позже в настройках.
; The "Data location" page.
en.DataLocCaption=Data location
ru.DataLocCaption=Расположение данных
en.DataLocSub=Where should chats, settings and profiles be stored?
ru.DataLocSub=Где хранить чаты, настройки и профили?
en.DataLocPrompt=Application data is stored separately from the program files.
ru.DataLocPrompt=Данные приложения хранятся отдельно от файлов программы.
en.OptSystem=Standard user folder (recommended)
ru.OptSystem=Стандартная папка пользователя (рекомендуется)
en.OptPortable=Portable — next to the program
ru.OptPortable=Портативно — рядом с программой
en.OptCustom=Another folder...
ru.OptCustom=Другая папка…
; The directory-picker page.
en.DirCaption=Data folder
ru.DirCaption=Папка данных
en.DirSub=Where should the data be saved?
ru.DirSub=Куда сохранять данные?
en.DirPrompt=Select a directory for the application data, then click Next.
ru.DirPrompt=Укажите каталог для данных приложения, затем нажмите «Далее».

[Files]
Source: "{#BinDir}\mindfork-rs.exe"; DestDir: "{app}"; Flags: ignoreversion
; Spellcheck dictionaries — in a portable layout next to the binary (the P1 fallback): with
; mode=system they're absent from the data directory, the app takes them from here.
Source: "{#SourcePath}..\..\dictionaries\*.aff"; DestDir: "{app}\data\dictionaries"; Flags: ignoreversion
Source: "{#SourcePath}..\..\dictionaries\*.dic"; DestDir: "{app}\data\dictionaries"; Flags: ignoreversion
Source: "{#SourcePath}..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourcePath}..\..\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourcePath}..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourcePath}..\..\docs\install.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\mindfork-rs"; Filename: "{app}\mindfork-rs.exe"
Name: "{autodesktop}\mindfork-rs"; Filename: "{app}\mindfork-rs.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

; defaults.json is written by the code (see [Code]); it's also removed on uninstall. The user's
; data (chats/profiles in %APPDATA% or portable) is untouched by the uninstaller.
[UninstallDelete]
Type: files; Name: "{app}\defaults.json"

[Code]
var
  LangPage: TInputOptionWizardPage;
  DataPage: TInputOptionWizardPage;
  DirPage: TInputDirWizardPage;
  SystemIndex, PortableIndex, CustomIndex: Integer;

procedure InitializeWizard;
begin
  { The application-language picker page (radio buttons). }
  LangPage := CreateInputOptionPage(wpSelectDir,
    CustomMessage('AppLangCaption'), CustomMessage('AppLangSub'),
    CustomMessage('AppLangPrompt'), True, False);
  LangPage.Add('Русский');
  LangPage.Add('English');
  if ActiveLanguage = 'ru' then
    LangPage.SelectedValueIndex := 0
  else
    LangPage.SelectedValueIndex := 1;

  { The data-location picker page (radio buttons). The portable option is only
    for a per-user install: in Program Files (per-machine) you can't write data
    next to the exe. The indices are computed, since the option set depends on the mode. }
  DataPage := CreateInputOptionPage(LangPage.ID,
    CustomMessage('DataLocCaption'), CustomMessage('DataLocSub'),
    CustomMessage('DataLocPrompt'), True, False);
  DataPage.Add(CustomMessage('OptSystem'));
  SystemIndex := 0;
  if IsAdminInstallMode then
  begin
    PortableIndex := -1;
    DataPage.Add(CustomMessage('OptCustom'));
    CustomIndex := 1;
  end
  else
  begin
    DataPage.Add(CustomMessage('OptPortable'));
    PortableIndex := 1;
    DataPage.Add(CustomMessage('OptCustom'));
    CustomIndex := 2;
  end;
  DataPage.SelectedValueIndex := SystemIndex;

  { The directory field for the "Another folder..." option. Shown conditionally (ShouldSkipPage). }
  DirPage := CreateInputDirPage(DataPage.ID,
    CustomMessage('DirCaption'), CustomMessage('DirSub'),
    CustomMessage('DirPrompt'), False, '');
  DirPage.Add('');
  DirPage.Values[0] := ExpandConstant('{userdocs}\mindfork-rs');
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := False;
  { Upgrade: if defaults.json already exists in the target directory — the settings were set by a
    previous install, don't ask again. We take the path via WizardDirValue (the directory
    field's current value), NOT via expanding the app constant: it isn't initialized yet at the
    wizard-page-display stage, and ExpandConstant on it throws a Runtime error. }
  if FileExists(AddBackslash(WizardDirValue) + 'defaults.json') then
  begin
    if (PageID = LangPage.ID) or (PageID = DataPage.ID) or (PageID = DirPage.ID) then
      Result := True;
  end
  else if PageID = DirPage.ID then
    { The directory is only needed when "Another folder..." is selected. }
    Result := DataPage.SelectedValueIndex <> CustomIndex;
end;

function JsonEscape(const S: String): String;
begin
  Result := S;
  { Order matters: the backslash first, then the quote (otherwise slashes from \" would double up). }
  StringChangeEx(Result, '\', '\\', True);
  StringChangeEx(Result, '"', '\"', True);
end;

function SaveJson(const FileName, S: String): Boolean;
var
  Arr: TArrayOfString;
begin
  SetArrayLength(Arr, 1);
  Arr[0] := S;
  { UTF-8 (with a BOM — the app drops it, P3): correct for Cyrillic paths. }
  Result := SaveStringsToUTF8File(FileName, Arr, False);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Path, Json, Lang, Mode, DataDir: String;
begin
  if CurStep = ssPostInstall then
  begin
    Path := ExpandConstant('{app}\defaults.json');
    { Don't overwrite on an upgrade — the user's choice is preserved. }
    if not FileExists(Path) then
    begin
      if LangPage.SelectedValueIndex = 0 then
        Lang := 'ru'
      else
        Lang := 'en';

      if DataPage.SelectedValueIndex = SystemIndex then
        Mode := 'system'
      else if (PortableIndex >= 0) and (DataPage.SelectedValueIndex = PortableIndex) then
        Mode := 'portable'
      else
        Mode := 'path';

      if Mode = 'path' then
      begin
        DataDir := DirPage.Values[0];
        Json := '{"mode":"path","path":"' + JsonEscape(DataDir) +
          '","default_language":"' + Lang + '"}';
      end
      else
        Json := '{"mode":"' + Mode + '","default_language":"' + Lang + '"}';

      SaveJson(Path, Json);
    end;
  end;
end;
