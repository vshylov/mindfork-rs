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
; The three URLs surface in Windows "Apps & features" (the ARP entry). The project
; site is the publisher link; support/updates deliberately stay on GitHub — that is
; where issues and release artifacts actually live.
AppPublisherURL=https://mindfork.io
AppSupportURL=https://github.com/vshylov/mindfork-rs/issues
AppUpdatesURL=https://github.com/vshylov/mindfork-rs/releases
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
; The optional Python sandbox (ADR 0005). Off by default; ticking it is the
; deliberate act that both provisions the sandbox and switches the tool on, so
; the label says both — it changes a security-relevant setting.
en.SandboxTask=Install the Python sandbox and enable Python execution (downloads ~300 MB)
ru.SandboxTask=Установить Python-песочницу и включить выполнение Python (загрузка ~300 МБ)
en.SandboxStatus=Installing the Python sandbox (this may take several minutes)...
ru.SandboxStatus=Установка Python-песочницы (может занять несколько минут)…

[Files]
; AfterInstall (not CurStepChanged(ssPostInstall), where this used to live): [Run]
; entries are processed BEFORE ssPostInstall — measured, not assumed — and the
; optional `sandbox setup` run needs defaults.json to already be there, or it would
; download into the default data directory instead of the one picked on the
; "Data location" page. Writing it right after the binary lands is early enough
; for every path. WriteDefaults is idempotent (it skips an existing file).
Source: "{#BinDir}\mindfork-rs.exe"; DestDir: "{app}"; Flags: ignoreversion; AfterInstall: WriteDefaults
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
Name: "installsandbox"; Description: "{cm:SandboxTask}"; Flags: unchecked

; The optional Python sandbox for the `python_exec` tool (ADR 0005): `sandbox setup`
; downloads wasmer + CPython + the wheels into `<data>/sandbox/` from the lock list.
; `--enable-python` then switches `tools.python_enabled` on in settings.json — done by
; the app, not from here: the installer must not write user data (it would have to
; re-implement the data-root resolution in Pascal and could clobber an existing config),
; and the flag only takes effect after the assets are actually in place.
; Notes on the flags:
;  * Inno processes [Run] entries BEFORE CurStepChanged(ssPostInstall) — measured,
;    not assumed — which is why defaults.json is written from AfterInstall on the
;    [Files] entry instead. Without that the download would land in the default
;    data directory rather than the one picked on the "Data location" page;
;  * no `runhidden`: mindfork-rs.exe is a console binary, so the console window it
;    opens is what shows the download progress of a multi-minute job;
;  * `runasoriginaluser` matters for a per-machine install (Setup is elevated then,
;    and the data directory in `system` mode is per-user — without this the sandbox
;    would land in the elevating admin's %APPDATA%);
;  * a failed download does NOT fail the install (Inno ignores a non-zero exit code
;    of a [Run] entry). The sandbox is optional and re-runnable at any time with
;    `mindfork-rs sandbox setup`; the error stays visible in the console.
[Run]
Filename: "{app}\mindfork-rs.exe"; Parameters: "sandbox setup --enable-python"; WorkingDir: "{app}"; \
  StatusMsg: "{cm:SandboxStatus}"; Tasks: installsandbox; \
  Flags: waituntilterminated runasoriginaluser

; defaults.json is written by the code (see [Code]); it's also removed on uninstall. The user's
; data (chats/profiles in %APPDATA% or portable) is untouched by the uninstaller — that
; deliberately includes `<data>/sandbox/`, which sits next to the chats and is
; re-downloadable rather than being ours to delete.
[UninstallDelete]
Type: files; Name: "{app}\defaults.json"

[Code]
var
  LangPage: TInputOptionWizardPage;
  DataPage: TInputOptionWizardPage;
  DirPage: TInputDirWizardPage;
  SystemIndex, PortableIndex, CustomIndex: Integer;
  { Named like the data-location indices below, so the option order can be changed
    without silently inverting the language written into defaults.json. }
  EnLangIndex, RuLangIndex: Integer;

procedure InitializeWizard;
begin
  { The application-language picker page (radio buttons). }
  LangPage := CreateInputOptionPage(wpSelectDir,
    CustomMessage('AppLangCaption'), CustomMessage('AppLangSub'),
    CustomMessage('AppLangPrompt'), True, False);
  { English first, matching the [Languages] order. The pre-selected option still
    follows the language the wizard itself is running in, not the list order. }
  LangPage.Add('English');
  EnLangIndex := 0;
  LangPage.Add('Русский'); // the Russian option's own label; cyrillic-ok
  RuLangIndex := 1;
  if ActiveLanguage = 'ru' then
    LangPage.SelectedValueIndex := RuLangIndex
  else
    LangPage.SelectedValueIndex := EnLangIndex;

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

// Writes the wizard's choices into the defaults.json next to the binary. Called from
// the main binary's AfterInstall (see [Files]) — early enough for the optional [Run]
// entry, which Inno processes before ssPostInstall. Idempotent: on an upgrade the
// existing file is left alone, so the user's earlier choice is preserved.
// (Line comments, not a { } block: an app constant in braces would close it early.)
procedure WriteDefaults;
var
  Path, Json, Lang, Mode, DataDir: String;
begin
  Path := ExpandConstant('{app}\defaults.json');
  if not FileExists(Path) then
  begin
    if LangPage.SelectedValueIndex = RuLangIndex then
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
