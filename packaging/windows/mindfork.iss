; The Inno Setup installer script for mindfork on Windows (§3.3 docs/history/installers.md).
; The stock legal pages (the MIT license, then the disclaimer — see [Setup] and
; [Languages] below, one text per language) and two custom ones — "Application
; language" and "Data location" — with the choice written
; into defaults.json next to the binary. A bilingual UI (ru/en), a per-user install
; with no UAC.
;
; The script requires **Inno Setup 7** — for SetupArchitecture (see [Setup]); everything
; else here compiles on 6.7.x as well. CI installs the pinned compiler with
; tools/install_inno.ps1, which is also the way to get locally the exact build the
; releases are compiled with (docs/research/inno-setup-7.md).
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

; The numeric version for the VersionInfo* fields below. A rehearsal tag carries a
; prerelease suffix (`v0.9.9-rc1`, docs/research/release-pipeline.md §6) and Windows
; file metadata takes digits and dots only — Inno answers a suffix with
; `Value of [Setup] section directive "VersionInfoVersion" is invalid` and aborts,
; which is exactly how the first rehearsal failed. AppVersion keeps the suffix (it is
; what the user sees and what names the artifact); this drops it for the metadata.
#define Dash Pos("-", AppVersion)
#if Dash > 0
  #define NumericVersion Copy(AppVersion, 1, Dash - 1)
#else
  #define NumericVersion AppVersion
#endif

[Setup]
AppId={{A7E3F1C2-9B84-4D6E-8F1A-2C5B7D9E0463}
; Naming: the app presents as the brand `mindfork` — AppName, the shortcuts,
; the installed exe — while DefaultDirName and OutputBaseFilename keep the
; project name `mindfork-rs`, like every data directory
; (docs/research/binary-rename.md).
AppName=mindfork
AppVersion={#AppVersion}
AppPublisher=Vladimir Shylov
; The three URLs surface in Windows "Apps & features" (the ARP entry). The project
; site is the publisher link; support/updates deliberately stay on GitHub — that is
; where issues and release artifacts actually live.
AppPublisherURL=https://mindfork.io
AppSupportURL=https://github.com/vshylov/mindfork-rs/issues
AppUpdatesURL=https://github.com/vshylov/mindfork-rs/releases
; The legal pages, in the order the wizard shows them:
;  * the MIT text on Inno's own license page (accept/decline gates Next);
;  * the disclaimer on the "info before install" page: read-only, Next continues.
;    It is a supplement to the license, not a second contract, so it gets one
;    acceptance rather than two;
;  * the privacy policy, read-only as well. Inno has only one InfoBeforeFile, so
;    that page is built in [Code] (PrivacyPage) rather than declared here, and
;    picks its language there instead of in [Languages].
; WHICH text each page shows is a per-language choice, so it lives in [Languages]
; below, on the same line as that language's message file — one place to look,
; rather than a default here and an override there. What the entries point at:
;  * en — the root LICENSE, extension-less file and all: plain ASCII with no
;    markdown, which is exactly the shape a gate test already holds it to
;    (credits::license_file_carries_nothing_but_the_mit_text), so the English
;    license page needs no copy of its own; and the generated disclaimer.rtf,
;    because DISCLAIMER.md IS markdown and Inno renders only text or RTF;
;  * ru — license-ru.rtf and disclaimer-ru.rtf, generated from the translations
;    under docs/legal/. They are unofficial translations, each saying so in its
;    own first paragraph, with the English original governing
;    (docs/history/legal-ru-translations.md). Generated for the markdown, and for the
;    encoding: an RTF of \uNNNN? escapes is pure ASCII, where a Cyrillic .txt
;    would leave the compiler guessing.
; All five .rtf files come from tools/wizard_rtf.py, kept in step with their
; sources by `--check` in CI's lint job. Never edit one by hand: the installer
; would then present a different text than the repository, the release archives
; and the app's F1 tabs.
DefaultDirName={autopf}\mindfork-rs
DefaultGroupName=mindfork
DisableProgramGroupPage=yes
; Per-user by default (no UAC); the user can choose "for everyone" at startup.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
; The setup binary itself is 64-bit (Inno 7; the default is still a 32-bit setup, which
; is what 6 could only build), matching the application it carries. The two directives
; below already refuse a system that cannot run the program — but they refuse it from
; *inside* a wizard that started. A 64-bit setup cannot start there at all, which is the
; harder cut-off, and the one asked for: the program is x64-only, so there is no
; scenario where the wizard should get as far as drawing a page. Cost, measured: the
; setup grows from 3.13 MB to 3.88 MB (+746 KB) — see docs/research/inno-setup-7.md §3.
; The Architectures* pair stays explicit even though SetupArchitecture=x64 now makes
; x64compatible their default: they state the *installation's* rule, not the compiler's,
; and neither should silently follow the other.
; The optional PATH entry below writes to the environment; this is what makes Inno
; broadcast WM_SETTINGCHANGE, so a shell opened afterwards sees it without a logout.
ChangesEnvironment=yes
SetupArchitecture=x64
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#SourcePath}..\..\dist
OutputBaseFilename=mindfork-rs-v{#AppVersion}-x86_64-setup
; Version info for setup.exe itself — the same strings build.rs stamps into
; mindfork.exe, so one product speaks with one name. Explicit rather than left to
; Inno's defaults for two reasons. The defaults are partly wrong: VersionInfoVersion
; is 0.0.0.0 unless set, so the installer shipped a zero version, and
; VersionInfoProductVersion follows it. And the ones that are right are right only
; by inheritance — VersionInfoProductName falls back to AppName, VersionInfoCompany
; to AppPublisher — which is a value that silently changes when a neighbouring
; directive does. Code signing pins these strings through a file metadata
; restriction (docs/research/code-signing.md §6.2), so "right by accident" is not
; good enough. VersionInfoCopyright is the copyright line from LICENSE: build.rs
; reads that file directly, this one cannot, so the gate test
; credits::the_installer_and_the_binary_declare_the_same_product holds them together.
VersionInfoVersion={#NumericVersion}
VersionInfoProductVersion={#NumericVersion}
VersionInfoProductName=mindfork
VersionInfoDescription=mindfork Setup
VersionInfoCompany=Vladimir Shylov
VersionInfoCopyright=Copyright (c) 2026 Vladimir Shylov
VersionInfoOriginalFileName=mindfork-rs-v{#AppVersion}-x86_64-setup.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName=mindfork
UninstallDisplayIcon={app}\mindfork.exe
; Branding (docs/branding.md §4.1). The icon of setup.exe itself and the logo in the
; wizard's header. PNG is accepted on par with BMP and 35x lighter — verified on
; Inno 6, and taken unchanged by 7 (the header logo was looked at on a 64-bit setup).
; The installed .exe's icon is embedded by build.rs (winresource), so the
; [Icons] shortcuts and UninstallDisplayIcon pick it up with no extra settings.
SetupIconFile={#SourcePath}..\..\assets\mindfork.ico
WizardSmallImageFile={#SourcePath}..\..\assets\mindfork-wizard-small.png

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"; \
  LicenseFile: "{#SourcePath}..\..\LICENSE"; InfoBeforeFile: "{#SourcePath}disclaimer.rtf"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"; \
  LicenseFile: "{#SourcePath}license-ru.rtf"; InfoBeforeFile: "{#SourcePath}disclaimer-ru.rtf"

; The stock "info before install" page calls itself "Information" and asks the user to
; read "important information" — true of a readme, and an understatement for a notice
; whose closing line says not to use the software if you disagree with it. Naming the
; page after what it holds is also what makes the license → disclaimer pair legible as
; two steps rather than one page plus a stray readme. The ru caption is the borrowed
; "дисклеймер" (cyrillic-ok: the ru caption itself): the native alternative mostly
; means a slip of the tongue. The app's F1 tab used to carry the same word and is now
; the "Legal" tab (ui.help.tab.legal) — not a divergence but the same rule applied twice:
; this page holds one document and is named after it, that tab holds two (the
; disclaimer and the privacy policy) and is named after what they have in common.
; The text on the page is Russian too now
; (see [Languages]); before this, only the caption ever was.
; InfoBeforeClickLabel ("When you are ready to continue with Setup, click Next") is
; left at Inno's default — it says the right thing already.
[Messages]
en.WizardInfoBefore=Disclaimer
ru.WizardInfoBefore=Дисклеймер
en.InfoBeforeLabel=Please read the disclaimer below before continuing. It supplements the license you have just accepted and does not modify it.
ru.InfoBeforeLabel=Пожалуйста, прочитайте дисклеймер перед продолжением. Он дополняет принятую вами лицензию и не изменяет её.

[CustomMessages]
; The "Privacy policy" page — read-only, straight after the disclaimer, so the
; three legal texts stand together at the start of the wizard. The policy grants
; nothing and asks for nothing, so it gets no acceptance of its own: Next
; continues, exactly like the disclaimer page (docs/research/code-signing.md
; §8.1). The prompt names the installed copy rather than the website, because at
; this point the user has no browser open and will have the file.
en.AddToPathTask=Add mindfork to PATH (so `mindfork` works in any terminal)
ru.AddToPathTask=Добавить mindfork в PATH (чтобы `mindfork` работал в любом терминале)
en.PrivacyCaption=Privacy policy
ru.PrivacyCaption=Политика конфиденциальности
en.PrivacySub=What mindfork stores, and what it sends where.
ru.PrivacySub=Что mindfork хранит и что куда отправляет.
en.PrivacyPrompt=The program has no telemetry and sends nothing to its author. The full text below is installed as PRIVACY.md next to the program. When you are ready to continue, click Next.
ru.PrivacyPrompt=Программа не собирает телеметрию и ничего не отправляет автору. Полный текст ниже устанавливается рядом с программой как PRIVACY.md. Когда будете готовы продолжить, нажмите «Далее».
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
; The privacy policy as the wizard shows it, in both languages. `dontcopy` keeps
; them out of the install (the readable PRIVACY.md below is what lands on disk);
; the [Code] section pulls the right one out with ExtractTemporaryFile.
; **They come first on purpose**: with SolidCompression, extracting a file means
; decompressing everything listed before it, so a temporary file near the bottom
; would stall the wizard behind the whole payload.
Source: "{#SourcePath}privacy.rtf"; Flags: dontcopy
Source: "{#SourcePath}privacy-ru.rtf"; Flags: dontcopy
; AfterInstall (not CurStepChanged(ssPostInstall), where this used to live): [Run]
; entries are processed BEFORE ssPostInstall — measured, not assumed — and the
; optional `sandbox setup` run needs defaults.json to already be there, or it would
; download into the default data directory instead of the one picked on the
; "Data location" page. Writing it right after the binary lands is early enough
; for every path. WriteDefaults is idempotent (it skips an existing file).
Source: "{#BinDir}\mindfork.exe"; DestDir: "{app}"; Flags: ignoreversion; AfterInstall: WriteDefaults
; Spellcheck dictionaries — in a portable layout next to the binary (the P1 fallback): with
; mode=system they're absent from the data directory, the app takes them from here.
Source: "{#SourcePath}..\..\dictionaries\*.aff"; DestDir: "{app}\data\dictionaries"; Flags: ignoreversion
Source: "{#SourcePath}..\..\dictionaries\*.dic"; DestDir: "{app}\data\dictionaries"; Flags: ignoreversion
; ...and their licences beside them: third-party work under BSD-style and LGPL
; terms, both of which require the notice to accompany a redistribution
; (dictionaries/SOURCES.md).
Source: "{#SourcePath}..\..\dictionaries\licenses\*.txt"; DestDir: "{app}\data\dictionaries\licenses"; Flags: ignoreversion
Source: "{#SourcePath}..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourcePath}..\..\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourcePath}..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
; The model-output disclaimer travels with the license it supplements.
Source: "{#SourcePath}..\..\DISCLAIMER.md"; DestDir: "{app}"; Flags: ignoreversion
; ...and so do the Russian translations of the pair, flattened next to their
; originals: whoever read the ru pages in this wizard can find that text again.
Source: "{#SourcePath}..\..\docs\legal\LICENSE.ru.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourcePath}..\..\docs\legal\DISCLAIMER.ru.md"; DestDir: "{app}"; Flags: ignoreversion
; The privacy policy the wizard just showed, so it can be re-read offline — and
; its translation, on the same footing as the pair above.
Source: "{#SourcePath}..\..\PRIVACY.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourcePath}..\..\docs\legal\PRIVACY.ru.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourcePath}..\..\docs\install.md"; DestDir: "{app}"; Flags: ignoreversion
; The licence texts of every Rust package in the locked graph, generated for this
; exact build (cargo about, see about.toml) — MIT, BSD and Apache all require the
; notice to travel with a redistribution. release.yml and packaging.yml generate
; it before compiling this script; a local compile needs
; `cargo about generate about.hbs -o THIRD-PARTY-NOTICES.md` first.
Source: "{#SourcePath}..\..\THIRD-PARTY-NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
; The vendored syntax grammars are compiled into the binary by build.rs, so their
; licences have no file of their own to sit beside (syntaxes\SOURCES.md).
Source: "{#SourcePath}..\..\syntaxes\licenses\*.txt"; DestDir: "{app}\licenses\syntaxes"; Flags: ignoreversion

[Icons]
Name: "{group}\mindfork"; Filename: "{app}\mindfork.exe"
Name: "{autodesktop}\mindfork"; Filename: "{app}\mindfork.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked
Name: "installsandbox"; Description: "{cm:SandboxTask}"; Flags: unchecked
; `mindfork llama setup`, `mindfork sandbox setup` and the first-run guidance all
; name the command rather than a path, which is only true from a shell that can
; find it. Off by default: an installer that edits the environment without being
; asked is the kind of surprise this one avoids
; (docs/research/robustness-and-defaults.md D6).
Name: "addtopath"; Description: "{cm:AddToPathTask}"; Flags: unchecked

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
;  * no `runhidden`: mindfork.exe is a console binary, so the console window it
;    opens is what shows the download progress of a multi-minute job;
;  * `runasoriginaluser` matters for a per-machine install (Setup is elevated then,
;    and the data directory in `system` mode is per-user — without this the sandbox
;    would land in the elevating admin's %APPDATA%);
;  * a failed download does NOT fail the install (Inno ignores a non-zero exit code
;    of a [Run] entry). The sandbox is optional and re-runnable at any time with
;    `mindfork sandbox setup`; the error stays visible in the console.
[Run]
Filename: "{app}\mindfork.exe"; Parameters: "sandbox setup --enable-python"; WorkingDir: "{app}"; \
  StatusMsg: "{cm:SandboxStatus}"; Tasks: installsandbox; \
  Flags: waituntilterminated runasoriginaluser

; defaults.json is written by the code (see [Code]); it's also removed on uninstall. The user's
; data (chats/profiles in %APPDATA% or portable) is untouched by the uninstaller — that
; deliberately includes `<data>/sandbox/`, which sits next to the chats and is
; re-downloadable rather than being ours to delete.
; The PATH entry for the `addtopath` task. Two entries, because the key depends on
; the install mode: a per-user install writes the user's environment, an elevated
; one the machine's. `{olddata}` keeps whatever is there; `expandsz` keeps other
; entries' `%VARIABLES%` expandable, which a plain string value would destroy for
; the whole value. The removal on uninstall is in [Code] — `uninsdeletevalue` here
; would delete the entire Path.
[Registry]
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: not IsAdminInstallMode and NeedsAddPath(ExpandConstant('{app}'))
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: IsAdminInstallMode and NeedsAddPath(ExpandConstant('{app}'))

[UninstallDelete]
Type: files; Name: "{app}\defaults.json"

[Code]
var
  PrivacyPage: TOutputMsgMemoWizardPage;
  LangPage: TInputOptionWizardPage;
  DataPage: TInputOptionWizardPage;
  DirPage: TInputDirWizardPage;
  SystemIndex, PortableIndex, CustomIndex: Integer;
  { Named like the data-location indices below, so the option order can be changed
    without silently inverting the language written into defaults.json. }
  EnLangIndex, RuLangIndex: Integer;

{ The privacy policy as RTF, in the language the wizard is running in — the same
  per-language choice [Languages] makes for the license and the disclaimer, made
  here because a page built in [Code] gets no such entry. The files are pure
  ASCII by construction (tools/wizard_rtf.py escapes everything above it), which
  is why an AnsiString carries them unharmed. }
function PrivacyRtf: AnsiString;
var
  RtfFile: String;
  Rtf: AnsiString;
begin
  if ActiveLanguage = 'ru' then
    RtfFile := 'privacy-ru.rtf'
  else
    RtfFile := 'privacy.rtf';
  ExtractTemporaryFile(RtfFile);
  { AddBackslash rather than a literal separator, as on the data-location page. }
  if LoadStringFromFile(AddBackslash(ExpandConstant('{tmp}')) + RtfFile, Rtf) then
    Result := Rtf
  else
    Result := '';
end;

procedure InitializeWizard;
begin
  { The privacy policy page: read-only, straight after the disclaimer, and shown
    on every run — a legal notice, not a setting, so unlike the two pages below
    it is never skipped on an upgrade. }
  PrivacyPage := CreateOutputMsgMemoPage(wpInfoBefore,
    CustomMessage('PrivacyCaption'), CustomMessage('PrivacySub'),
    CustomMessage('PrivacyPrompt'), PrivacyRtf);

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

{ ---- The optional PATH entry (task `addtopath`) ---------------------------- }

{ Which environment the install writes: the user's, or the machine's when the
  wizard was elevated ("for everyone"). One place, so the [Registry] entries,
  the check below and the uninstaller cannot disagree about where it went. }
procedure PathKey(var RootKey: Integer; var SubKey: String);
begin
  if IsAdminInstallMode then
  begin
    RootKey := HKEY_LOCAL_MACHINE;
    SubKey := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  end
  else
  begin
    RootKey := HKEY_CURRENT_USER;
    SubKey := 'Environment';
  end;
end;

{ Called from [Registry]: is this directory missing from PATH? Without it an
  upgrade would append the same directory again, every time. The comparison pads
  both sides with ';' so a directory that is merely a prefix of an existing entry
  does not count as present. }
function NeedsAddPath(Param: String): Boolean;
var
  RootKey: Integer;
  SubKey, OrigPath: String;
begin
  PathKey(RootKey, SubKey);
  if not RegQueryStringValue(RootKey, SubKey, 'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Param) + ';', ';' + Uppercase(OrigPath) + ';') = 0;
end;

{ The other half: an uninstall takes its own entry back out. [Registry]'s
  `uninsdeletevalue` cannot be used for this — it would delete the whole Path
  value, taking every other program's entry with it. }
procedure RemoveFromPath(Dir: String);
var
  RootKey: Integer;
  SubKey, Path: String;
  P: Integer;
begin
  PathKey(RootKey, SubKey);
  if not RegQueryStringValue(RootKey, SubKey, 'Path', Path) then
    exit;
  { Find the entry with its separators, then cut it and the ';' that joined it. }
  P := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';');
  if P = 0 then
    exit;
  Delete(Path, P - 1, Length(Dir) + 1);
  { A leading ';' is left when the entry was the first one. }
  if (Length(Path) > 0) and (Path[1] = ';') then
    Delete(Path, 1, 1);
  RegWriteExpandStringValue(RootKey, SubKey, 'Path', Path);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  { After the files are gone, so a failed uninstall does not leave PATH pointing
    at a directory that is still there. }
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
