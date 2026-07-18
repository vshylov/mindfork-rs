; Inno Setup-скрипт установщика mindfork-rs для Windows (§3.3 docs/history/installers.md).
; Две кастомные страницы мастера — «Язык приложения» и «Расположение данных» — и запись
; выбора в defaults.json рядом с бинарником. Двуязычный UI (ru/en), per-user установка
; без UAC. Скрипт совместим с Inno Setup 6.7.x.
;
; Версия и каталог с бинарником передаются компилятору через /D:
;   ISCC.exe /DAppVersion=0.9.0 /DBinDir="C:\path\to\dir-with-exe" packaging\windows\mindfork.iss
; Дефолты ниже позволяют скомпилировать скрипт локально/в CI для проверки синтаксиса.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef BinDir
  ; Локальный дефолт: релизная сборка из корня репозитория. SourcePath уже несёт
  ; завершающий "\", поэтому склеиваем без ведущего слэша.
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
; Per-user по умолчанию (без UAC); пользователь может выбрать «для всех» на старте.
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
; Брендинг (docs/branding.md §4.1). Иконка самого setup.exe и логотип в шапке
; мастера. PNG проверен на Inno 6 — принимается наравне с BMP и в 35 раз легче.
; Иконку установленного .exe вшивает build.rs (winresource), поэтому ярлыки
; [Icons] и UninstallDisplayIcon получают её без дополнительных настроек.
SetupIconFile={#SourcePath}..\..\artwork\mindfork.ico
WizardSmallImageFile={#SourcePath}..\..\artwork\mindfork-wizard-small.png

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
; Страница «Язык приложения».
en.AppLangCaption=Application language
ru.AppLangCaption=Язык приложения
en.AppLangSub=In which language should the application run?
ru.AppLangSub=На каком языке запускать приложение?
en.AppLangPrompt=Choose the interface language. You can change it later in the settings.
ru.AppLangPrompt=Выберите язык интерфейса. Его можно сменить позже в настройках.
; Страница «Расположение данных».
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
; Страница выбора каталога.
en.DirCaption=Data folder
ru.DirCaption=Папка данных
en.DirSub=Where should the data be saved?
ru.DirSub=Куда сохранять данные?
en.DirPrompt=Select a directory for the application data, then click Next.
ru.DirPrompt=Укажите каталог для данных приложения, затем нажмите «Далее».

[Files]
Source: "{#BinDir}\mindfork-rs.exe"; DestDir: "{app}"; Flags: ignoreversion
; Словари спелл-чека — в портативную раскладку рядом с бинарём (резерв П1): при
; mode=system их в каталоге данных нет, приложение берёт отсюда.
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

; defaults.json пишется кодом (см. [Code]); при удалении его тоже убираем. Данные
; пользователя (чаты/профили в %APPDATA% или портативные) деинсталлятор не трогает.
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
  { Страница выбора языка приложения (радиокнопки). }
  LangPage := CreateInputOptionPage(wpSelectDir,
    CustomMessage('AppLangCaption'), CustomMessage('AppLangSub'),
    CustomMessage('AppLangPrompt'), True, False);
  LangPage.Add('Русский');
  LangPage.Add('English');
  if ActiveLanguage = 'ru' then
    LangPage.SelectedValueIndex := 0
  else
    LangPage.SelectedValueIndex := 1;

  { Страница выбора расположения данных (радиокнопки). Портативный вариант — только
    для per-user установки: в Program Files (per-machine) данные рядом с exe писать
    нельзя. Индексы вычисляем, т.к. набор опций зависит от режима. }
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

  { Каталог для варианта «Другая папка…». Показывается условно (ShouldSkipPage). }
  DirPage := CreateInputDirPage(DataPage.ID,
    CustomMessage('DirCaption'), CustomMessage('DirSub'),
    CustomMessage('DirPrompt'), False, '');
  DirPage.Add('');
  DirPage.Values[0] := ExpandConstant('{userdocs}\mindfork-rs');
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := False;
  { Апгрейд: если в целевом каталоге уже есть defaults.json — параметры заданы прошлой
    установкой, не спрашиваем снова. Берём путь через WizardDirValue (текущее значение
    поля каталога), а НЕ через раскрытие константы app: она на этапе показа страниц
    мастера ещё не инициализирована, и ExpandConstant по ней падает Runtime error. }
  if FileExists(AddBackslash(WizardDirValue) + 'defaults.json') then
  begin
    if (PageID = LangPage.ID) or (PageID = DataPage.ID) or (PageID = DirPage.ID) then
      Result := True;
  end
  else if PageID = DirPage.ID then
    { Каталог нужен только при выборе «Другая папка…». }
    Result := DataPage.SelectedValueIndex <> CustomIndex;
end;

function JsonEscape(const S: String): String;
begin
  Result := S;
  { Порядок важен: сперва обратный слэш, затем кавычка (иначе слэши из \" удвоятся). }
  StringChangeEx(Result, '\', '\\', True);
  StringChangeEx(Result, '"', '\"', True);
end;

function SaveJson(const FileName, S: String): Boolean;
var
  Arr: TArrayOfString;
begin
  SetArrayLength(Arr, 1);
  Arr[0] := S;
  { UTF-8 (с BOM — приложение его отбрасывает, П3): корректно для кириллических путей. }
  Result := SaveStringsToUTF8File(FileName, Arr, False);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Path, Json, Lang, Mode, DataDir: String;
begin
  if CurStep = ssPostInstall then
  begin
    Path := ExpandConstant('{app}\defaults.json');
    { Не перезаписываем при апгрейде — выбор пользователя сохраняется. }
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
