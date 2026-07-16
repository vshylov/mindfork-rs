# Исследование: инсталляторы Windows и Linux

**Статус: направление ЗАВЕРШЕНО** (исследование 2026-07-15; развилки Р1–Р9 подтверждены
пользователем 2026-07-16 по рекомендациям [рек.]; этапы 1–3 реализованы и проверены,
2026-07-16). План переехал в `docs/history/`. Итог: **предпосылки в коде** (fallback
словарей, язык по локали, BOM — ветка `feat/installed-mode-prereqs`), **Linux-пакеты**
deb/rpm/pkg.tar.zst через nfpm (`feat/linux-packages`), **Windows-инсталлятор** на Inno
Setup (`feat/windows-installer`) — все проверены (Windows-инсталлятор — живым прогоном на
реальном Inno Setup 6.7.3: компиляция, GUI-мастер, режимы `system`/`portable`/`path` с
эскейпом кириллического пути, round-trip чтения бинарником, апгрейд, деинсталляция; в
ходе живого прогона найден и исправлен краш `{app}` в `ShouldSkipPage`). **Этап 4
«Подпись кода» отложен** по решению пользователя — репозиторий приватный, сайта/логотипа/
иконки ещё нет; вернуться при подготовке к публичному открытию (тогда откроется путь (a)
SignPath Foundation). Заделы: winget-манифест (до подписи — portable-zip), AUR
`mindfork-rs-bin`, MSI под GPO/Intune при спросе.

Заказ был: инсталляторы **Windows** (msi или exe) и **Linux** (deb, rpm, pkg.tar.zst);
там, где формат это поддерживает, при установке выбираются **язык интерфейса** и
**расположение пользовательских файлов** (см. `defaults.json`); открытый вопрос — нужна
ли подпись Windows-бинарника и инсталлятора сертификатом и где его брать (§5).

Связанные документы: [docs/history/release-engineering.md](../history/release-engineering.md)
(релизный пайплайн; инсталляторы — его задел §5), [docs/install.md](../install.md)
(портативная установка сегодня), [spec §5.2, §12.1](../../spec.md) (расположение
данных, `defaults.json`), [docs/roadmap.md](../roadmap.md) («Авто-обновление /
инсталляторы»). Веб-факты сверены тремя параллельными обзорами (Windows-инструменты,
Linux-пакетирование, подпись кода) по первоисточникам — ключевые ссылки в §10.

---

## 1. Задача и текущее положение

Сегодня релиз (`release.yml`, тег `v*`) собирает **портативные архивы**
`mindfork-rs-vX.Y.Z-x86_64-{windows.zip,linux.tar.gz}` (бинарник + доки + словари в
`data/dictionaries/`) + `sha256sums.txt`. Установки как таковой нет: пользователь
распаковывает архив, данные живут рядом с бинарником (портативный режим по
умолчанию).

`defaults.json` рядом с бинарником ([shared/paths.rs](../../src/shared/paths.rs))
уже спроектирован **под инсталлятор** (комментарий в коде: «инсталлятор заполнит его
по выбору пользователя при установке»):

- `mode` — режим хранения данных: `portable` (подкаталог `data/` рядом с бинарником,
  дефолт) / `system` (Windows `%APPDATA%\mindfork-rs\data`, Linux
  `~/.local/share/mindfork-rs`) / `path` (произвольный каталог);
- `default_language` — язык, с которым создаются новые профили (ось A), **и** язык
  интерфейса при свежей установке (нет `settings.json` → `config.interface.language`
  берётся отсюда, `main.rs`), и язык CLI до появления настроек. Один выбор при
  установке покрывает всё это — ровно то, что просит заказ («выбрать язык
  интерфейса»).

Инсталляторы — это, по сути, доставка бинарника + словарей + **правильного
`defaults.json`** + регистрация в системе (Add/Remove Programs, деинсталлятор,
апгрейды).

---

## 2. Факты о приложении, определяющие дизайн установки

1. **Портативный дефолт несовместим с системной установкой.** Без `defaults.json`
   приложение пишет данные в `exe_dir/data` — в `C:\Program Files\…` и `/usr/bin`
   это невозможно (нет прав) → `ensure_dirs` падает при старте. Значит, каждый
   инсталлятор/пакет **обязан** класть `defaults.json` рядом с бинарником (обычно
   `{"mode":"system"}`).
2. **Словари спелл-чека грузятся только из каталога данных**
   (`paths.dictionaries_dir()` = `<root>/dictionaries`,
   [features/spellcheck/dict.rs](../../src/features/spellcheck/dict.rs)). В
   портативном режиме `root = exe_dir/data`, и релизный архив кладёт словари туда.
   При `mode=system` root — пользовательская папка, куда пакет файлы положить не
   может (а Linux-пакет — тем более, для всех пользователей сразу). Без доработки
   спелл-чек в установленном виде **молча выключен**. → Предпосылка П1 (§6):
   fallback-поиск словарей в `exe_dir/data/dictionaries` (портативная раскладка как
   источник read-only ресурсов).
3. **Зависимости бинарника тривиальны**: reqwest на `rustls` (без OpenSSL), rusqlite
   `bundled` (SQLite внутри), arboard на `x11rb` (чистый Rust). Linux-бинарь зависит
   фактически только от glibc (+libgcc) — Depends пакетов пишутся одной строкой.
4. **Резолв путей идёт от `std::env::current_exe()`.** На Linux это `/proc/self/exe`
   — возвращается путь **реального файла**, а не симлинка, через который программу
   запустили ([rust-lang/rust#43617](https://github.com/rust-lang/rust/issues/43617)).
   Это делает законной раскладку «реальный бинарь + сайдкары в `/usr/lib/<pkg>/`,
   симлинк из `/usr/bin`» **без правок кода** (§4.2).
5. **`defaults.json` должен переживать апгрейд** — это выбор пользователя при
   установке; повторная установка/обновление не должны его перезаписывать.
6. **Uninstall не трогает пользовательские данные** (`%APPDATA%`/XDG) — удаляется
   только установленное. Портативные данные (`{app}\data`) Inno и так не удалит
   (удаляет лишь то, что ставил).
7. `Defaults::read` сегодня **не терпит UTF-8 BOM** (whitespace-проверка + прямой
   `serde_json::from_slice`) — файл, записанный «с BOM» (некоторые редакторы,
   Pascal-хелперы Inno), уронит запуск. В проекте BOM уже отбрасывается в двух местах
   (`rag_ingest::read_text`, импортёр LameLLaMA) → Предпосылка П3 (§6).

---

## 3. Windows: exe (Inno Setup) против msi (WiX)

### 3.1 Ландшафт (июль 2026)

| Инструмент | Версия | Статус | На GHA `windows-latest` |
|---|---|---|---|
| **Inno Setup 6** | 6.7.3 (2026-05) | активен; для OSS бесплатен (с 6.5.0 есть *добровольная* коммерческая лицензия) | **да** (6.7.1; надёжнее ставить `choco install innosetup`) |
| Inno Setup 7 | 7.0.2 (2026-07-13) | только вышел; «full backward compatibility» с 6 | нет |
| **WiX Toolset** | v7.0.0 (2026-04) | активен; **v3/v4 — EOL с 02.2025**; с v6 — Open Source Maintenance Fee (EULA-плата для зарабатывающих) | только **v3.14** (EOL-линия) |
| NSIS | 3.11 (2025-03) | живой, редкие релизы | нет (удалён из образа windows-2025) |
| cargo-wix | 0.3.9 (2025-03) | живой; по умолчанию целится в WiX v3 | — |
| cargo-dist | 0.32.0 (2026-05) | релизы идут, но компания axo.dev свернулась (домен продаётся) — риск сопровождения | — |
| cargo-packager | 0.11.8 (2024-11) | **стагнация ~1.5 года** | — |

Rust-обёртки (cargo-dist / cargo-packager / tauri-bundler) делают **шаблонные**
инсталляторы: кастомные wizard-страницы «язык + папка данных» и запись
`defaults.json` по выбору пользователя они не выражают (либо требуют написать весь
шаблон .nsi/.wxs руками — выгода обёртки исчезает). Как основной инструмент не
подходят.

### 3.2 Наши пять «особых» требований — построчно

| Требование | Inno Setup | WiX/MSI |
|---|---|---|
| Страница «язык приложения» (радио) | штатно: `CreateInputOptionPage(Exclusive:=True)` | свой Dialog + RadioButtonGroup + правка publish-графа WixUI |
| Страница «папка данных» (radio + folder picker для папки ≠ каталога установки) | штатно: `CreateInputDirPage` | болезненно: `BrowseDlg` через косвенное `_BrowseProperty` + фиктивная Directory-запись |
| Запись `defaults.json` по выбору | `[Code]`: `SaveStrings…File` на `ssPostInstall` | deferred CustomAction (PowerShell/cmd/DTF) или стороннее расширение; штатного `util:JsonFile` нет ([wix#7711](https://github.com/orgs/wixtoolset/discussions/7711)) |
| Не перезаписывать при апгрейде | `if not FileExists(...)` — одна строка | доп. логика в CustomAction против компонентных правил MSI |
| Двуязычный UI инсталлятора (ru+en) | штатно: `[Languages]` + `ShowLanguageDialog`; **Russian.isl — официальный перевод** (с 6.5.0) | MSI однокультурный: либо 2 msi, либо трюк со встраиванием language transforms (torch); штатной поддержки нет ([wix#7544](https://github.com/wixtoolset/issues/issues/7544)) |

Плюс у Inno: per-user установка без UAC (`PrivilegesRequired=lowest`,
`{autopf}` → `%LOCALAPPDATA%\Programs`), диалог «для меня / для всех»
(`PrivilegesRequiredOverridesAllowed=dialog`), тихий режим
(`/VERYSILENT /DIR= /LANG=` + свои `{param:…}`), апгрейд по тому же `AppId` с
`UsePrevious*`. winget принимает `InstallerType: inno` и сам знает тихие ключи.

**Оценка трудоёмкости: Inno — 1–2 дня на скрипт + CI; MSI — дни-недели** (UI-диалоги,
CustomAction с откатом, мультиязычие трансформами, dual-context per-user капризен,
и вилка «EOL v3 бесплатно vs v6/v7 с Maintenance-Fee-EULA»).

### 3.3 Рекомендация и эскиз дизайна (Р1, Р2)

**Inno Setup 6.7.x, формат exe** (Р1). MSI добавлять только при реальном спросе на
корпоративную доставку (GPO/Intune) — задел. NSIS — запасной вариант без
преимуществ. (Нюанс winget для неподписанного exe — см. §5.4.)

Эскиз `packaging/windows/mindfork.iss`:

- `[Setup]`: `AppId={{…GUID…}}`, `DefaultDirName={autopf}\mindfork-rs`,
  `PrivilegesRequired=lowest`, `PrivilegesRequiredOverridesAllowed=dialog`,
  `ArchitecturesInstallIn64BitMode=x64compatible`; версия — `/DAppVersion=X.Y.Z` из
  тега в CI; `OutputBaseFilename=mindfork-rs-vX.Y.Z-x86_64-setup`.
- `[Languages]`: `en` + `ru` (официальный `Russian.isl`; на случай отсутствия в
  GHA-дистрибутиве — вендорим .isl в `packaging/windows/`).
- `[Files]`: `mindfork-rs.exe`, `README/CHANGELOG/LICENSE/install.md` → `{app}`;
  словари → `{app}\data\dictionaries` (портативная раскладка = источник read-only
  ресурсов, П1).
- **Страница 1 «Язык приложения»** (радио: Русский / English; дефолт — язык
  инсталлятора). Пишется в `default_language`.
- **Страница 2 «Где хранить данные»** (радио): «Стандартная папка пользователя
  (рекомендуется)» → `system`; «Портативно, рядом с программой» → `portable`
  (**скрывается при установке per-machine** — Program Files не для данных); «Другая
  папка…» → `path` + `CreateInputDirPage`.
- Обе страницы **пропускаются при апгрейде** (`ShouldSkipPage`, если
  `{app}\defaults.json` уже существует).
- `CurStepChanged(ssPostInstall)`: если `defaults.json` не существует — собрать JSON
  (эскейп `\` → `\\` в пути) и записать UTF-8 (`SaveStringsToUTF8File`; BOM
  нейтрализуется предпосылкой П3).
- Тихий режим: `/VERYSILENT /LANG=russian /DataMode=system|portable|path
  /DataDir="…" /AppLang=ru|en` (чтение через `{param:…}`; дефолты — system + язык
  инсталлятора) — нужно и для winget.
- Uninstall: удаляет `{app}` (бинарь, словари, `defaults.json`); данные в
  `%APPDATA%`/кастомном пути не трогает; портативные `{app}\data` Inno сам не
  удалит (не ставил их) — упомянуть в финальной странице/README.

### 3.4 CI

Новый job в `release.yml` (после `build`): windows-раннер → скачать артефакт
бинарника → `iscc packaging\windows\mindfork.iss /DAppVersion=%VERSION%` → артефакт
`…-setup.exe` → в общий `release`-job (архивы остаются — портативный сценарий
никуда не девается, это наш дефолтный жанр). `sha256sums.txt` накрывает новые
артефакты автоматически.

---

## 4. Linux: deb + rpm + pkg.tar.zst

### 4.1 Инструменты (июль 2026)

| Инструмент | Версия | Форматы | Авто-Depends | Примечание |
|---|---|---|---|---|
| **nfpm** (goreleaser) | v2.47.0 (2026-06) | **deb, rpm, archlinux (.pkg.tar.zst)**, apk, ipk | нет (руками) | один YAML → все форматы; `type: symlink`/`config|noreplace` из коробки; один Go-бинарь |
| cargo-deb | 3.7.0 (2026-05) | deb | да (`dpkg-shlibdeps`) | метаданные в Cargo.toml; `--no-build` для готового бинаря |
| cargo-generate-rpm | 0.21.0 (2026-05) | rpm | да (`--auto-req`) | без rpmbuild, чистый Rust |
| cargo-aur | 1.7.1 (2024-03) | PKGBUILD (-bin) | — | только AUR-рецепт, не .pkg.tar.zst |

**Рекомендация (Р4): nfpm один на все три формата.** Аргументы: у нас уже есть
prebuilt-бинарь из `release.yml` (nfpm ровно для этого); **идентичная раскладка** во
всех трёх форматах описывается один раз (включая archlinux, который cargo-инструменты
не покрывают); авто-Depends не нужен — стек «rustls + bundled SQLite + x11rb» сводит
зависимости к `libc6 (>= 2.35)` в deb (rpm/arch можно не указывать). Альтернатива
«cargo-deb + cargo-generate-rpm (+ что-то для arch)» легитимна ради метаданных в
Cargo.toml и честного `dpkg-shlibdeps`, но это три конфига вместо одного при
однострочном выигрыше.

### 4.2 Раскладка пакета: `defaults.json` нельзя в `/usr/bin`

FHS 3.0/Debian Policy запрещают данные в `/usr/bin` (плоское пространство имён
исполняемых команд; generic-имя `defaults.json` там немыслимо). Проверенный паттерн:

```
/usr/lib/mindfork-rs/mindfork-rs          реальный бинарь
/usr/lib/mindfork-rs/defaults.json        {"mode":"system"} (config|noreplace)
/usr/lib/mindfork-rs/data/dictionaries/   словари (.aff/.dic; читаются через П1)
/usr/bin/mindfork-rs -> ../lib/mindfork-rs/mindfork-rs    (type: symlink в nfpm)
/usr/share/doc/mindfork-rs/               README, CHANGELOG, LICENSE, install.md
```

Работает **без правок кода резолва путей**: `current_exe()` на Linux возвращает
target симлинка (§2 п.4) → `exe_dir = /usr/lib/mindfork-rs` → `defaults.json`
найден → данные в `~/.local/share/mindfork-rs`. Debian Policy §9.1.1 явно допускает
подкаталог `/usr/lib/<pkg>` со смешанным (в т.ч. арх-независимым) содержимым;
прецедент «вендорские дефолты как данные под /usr/lib» — `/usr/lib/os-release`.
`defaults.json` помечаем `config|noreplace` (nfpm: deb-conffile / rpm
`%config(noreplace)`) — правка пользователя переживёт апгрейд. Словари кладём в
`data/dictionaries` рядом с бинарём — та же портативная раскладка, что у zip и у
Windows-инсталлятора (один инвариант на все платформы, П1).

Альтернатива (Р5b): бинарь остаётся в `/usr/bin`, а приложение учится читать
`/etc/mindfork-rs/defaults.json` (+ словари из `/usr/share/mindfork-rs/`). FHS-чище
и «правильный» conffile, но добавляет платформенную ветку в резолв путей и второй
механизм поиска — при том, что симлинк-раскладка полностью штатна. Не рекомендую.

### 4.3 Интерактивных выборов на Linux не бывает

deb/rpm/pacman ставятся **неинтерактивно** (debconf — для системной конфигурации и
прямо не предназначен для пер-пользовательских предпочтений; у rpm/pacman механизма
нет вовсе). Требование заказа «выбор языка и расположения» здесь неприменимо —
пакеты фиксируют `{"mode":"system"}`, а язык решается приложением при первом
запуске. Сегодня «нет `default_language` в `defaults.json`» = `ru` (serde-дефолт) —
для международного пользователя deb/rpm это плохой дефолт. → Предпосылка П2 (§6):
автоопределение языка по локали ОС (закрывает и задел из roadmap «Определение языка
по системной локали»).

### 4.4 Базлайн glibc (Р9)

Сборка на `ubuntu-22.04` = glibc **2.35**: Ubuntu 22.04+/Debian 12+/Fedora 36+/
RHEL 10 — да; **RHEL/Rocky/Alma 9 (glibc 2.34) — нет**. Рекомендация: принять и
задекларировать (ниша TUI-приложения — Fedora/Ubuntu/Arch-десктопы; EL9-десктоп
экзотичен). Альтернативы, если EL9 понадобится: `cargo-zigbuild` с пином
`x86_64-unknown-linux-gnu.2.34` (не проверялось) или musl-static (осторожно:
аллокатор musl деградирует многопоточный tokio в разы — нужен mimalloc; crossterm
при этом terminfo не требует, musl-статике ничего не мешает).

### 4.5 Подпись пакетов не нужна; контрольные суммы уже есть

`dpkg -i`/`apt install ./x.deb` подписи **не проверяют** (доверие в deb-мире — на
уровне репозитория; `dpkg-sig` вообще удалён из Debian 12+); `rpm` проверяет только
при импортированном ключе; `pacman -U` с дефолтным `LocalFileSigLevel = Optional`
ставит неподписанное. Практика OSS для GitHub Releases — контрольные суммы (у нас
`sha256sums.txt` уже есть; GitHub с 2025 сам показывает digest ассетов). Опционально
и дёшево: **GitHub Artifact Attestations** (`actions/attest-build-provenance`,
Sigstore-провенанс, бесплатно для публичных реп; проверка `gh attestation verify`).
GPG-подписи заводить не стоит, пока нет собственного apt/dnf-репозитория.

### 4.6 Arch: .pkg.tar.zst + AUR как задел

nfpm собирает archlinux-пакет, который ставится `pacman -U` (систематических жалоб
в трекере nfpm нет; сам nfpm распространяется через AUR `nfpm-bin`). Перед первым
релизом — одноразовый смоук в контейнере `archlinux:latest` (легко автоматизировать
в CI — §8, DoD). **Идиоматичный канал для Arch — всё же AUR** (`mindfork-rs-bin`:
PKGBUILD + .SRCINFO, указывающие на GitHub Releases; суффикс `-bin` обязателен по
правилам AUR; обновления пользователю приносит AUR-хелпер, а `sha256sums` PKGBUILD
заодно верифицируют наши артефакты) — отдельный небольшой задел после первого
релиза пакетов.

---

## 5. Подпись кода Windows (открытый вопрос заказа)

### 5.1 Что происходит без подписи

Два слоя фрикции: браузер (Edge/Chrome «isn't commonly downloaded» → Keep → Keep
anyway) и запуск (Mark-of-the-Web → SmartScreen «Windows protected your PC», кнопка
«Run anyway» спрятана за «More info», издатель — «Unknown publisher»). Ключевая
механика (Microsoft, 2026): **без подписи репутация копится на хэш файла и
обнуляется каждым релизом**; с подписью — копится на сертификат и переносится между
релизами. Порог не публикуется («несколько недель и сотни чистых установок»).
Ужесточение: **Smart App Control** на свежих Windows 11 блокирует неподписанное
**без** кнопки обхода. Для нишевой TUI-аудитории (разработчики) неподписанный старт
— обычная практика, но каждый релиз будет «жёлтым».

### 5.2 Ландшафт 2026: что изменилось

- С 06.2023 (CA/B Forum) ключ подписи обязан жить в FIPS-железе/HSM — «просто .pfx
  в секретах CI» больше не существует; с 03.2026 максимальный срок сертификата —
  **460 дней** (ежегодное продление — норма).
- **EV больше не даёт мгновенной репутации SmartScreen** — Microsoft документировала
  это явно (изменение ~2024): «Paying a premium for EV solely to avoid SmartScreen
  warnings is no longer justified». OV = EV с точки зрения SmartScreen. EV к тому же
  продаётся только юрлицам.
- Timestamping (RFC 3161) обязателен всегда — подпись живёт после истечения
  сертификата.

### 5.3 Варианты (где брать сертификат)

| Вариант | Цена/год | Кому доступен | CI (GitHub Actions) | Примечания |
|---|---|---|---|---|
| **Без подписи** | 0 | всем | — | репутация с нуля каждый релиз; Smart App Control блокирует; для winget см. §5.4 |
| **SignPath Foundation** | **0** | **публичные OSS-проекты** (OSI-лицензия, публичный репозиторий, релизы, MFA, «code signing policy» на странице проекта) | официальный Action; подписываются только сборки из доверенного CI; **ручное одобрение каждого релиза** | издатель в диалогах — «SignPath Foundation» (не ваше имя); **официально рекомендован Microsoft** для OSS |
| **Certum Open Source** | ~€69 первый год, **~€29 продление** (+€35 за физ. карту, облако SimplySign — без неё) | **физлица почти из любой страны** (видеоверификация ~2 дня) | возможен, но костыльно (SimplySign: интерактивный OTP → TOTP-скрипт/контейнер; сессия ~2 ч) | subject: «Open Source Developer, &lt;Имя&gt;»; лимит 5000 подписей/мес; самый дешёвый «свой» сертификат |
| **Azure Artifact Signing** (экс-Trusted Signing; GA 01.2026) | $9.99/мес (≈$120/год) | **физлица: только США/Канада**; организации: США/Канада/ЕС/UK (+ платная Azure-подписка) | отличный: официальный Action, OIDC, подписывает всё, что умеет signtool | короткоживущие сертификаты из managed HSM; identity = ваше имя; географию обещают расширять — перепроверять |
| Коммерческий OV | ~$150–300 | физлица/организации, worldwide | токен — только self-hosted; облачные (eSigner/KeyLocker) — да, с доплатой | репутация копится, начальные предупреждения будут |
| EV | ~$280–700 | только юрлица (D-U-N-S) | как OV | **преимущества перед OV больше нет** — не покупать |

### 5.4 Нюанс winget

winget **не требует** подписи (манифесты пинят SHA256, валидация гоняет
антивирусы), но и **не обходит** SmartScreen: свежий кейс (halloy, 06.2026) —
`winget install` неподписанного **Inno-exe** виснет на SmartScreen-блокировке;
с MSI/portable-zip такого нет. Практический вывод: до появления подписи в winget
публиковать **портативный zip** (тип `zip`/`portable` — он у нас уже есть), а не
inno-exe; с подписью — можно и exe.

### 5.5 Рекомендация (Р8)

Подпись — **не блокер** первых релизов инсталлятора, но желательна как отдельный
этап. Дерево решения:

1. Репозиторий публичный (или готовы сделать публичным) → **SignPath Foundation**:
   бесплатно, рекомендовано Microsoft; цена — «SignPath Foundation» вместо личного
   имени в диалогах + ручное одобрение релиза + требования к процессу (MFA,
   политика подписи на странице проекта).
2. Нужен сертификат **на своё имя**, физлицо вне США/Канады → **Certum Open Source**
   (~€69/€29) — единственный дешёвый путь; автоматизация в CI возможна, но через
   обходные приёмы.
3. **Azure Artifact Signing** — лучший по цене/автоматизации, **как только** станет
   доступен по географии (физлицо США/Канада или юрлицо ЕС/UK/США/Канады);
   статус региональности перепроверять.
4. EV не покупать. В любом варианте: RFC3161-timestamp, подписывать каждый релиз
   одним identity, подписывать и `mindfork-rs.exe`, и сам setup (у Inno —
   `SignTool=`-директива подпишет и деинсталлятор).

---

## 6. Предпосылки в коде приложения (маленькие, до инсталляторов)

- **П1. Fallback словарей на портативную раскладку рядом с бинарником.**
  `dict::load` принимает один каталог; добавить второй источник —
  `exe_dir/data/dictionaries` (когда root ≠ `exe_dir/data`): пары, чьё базовое имя
  не найдено в каталоге данных, догружаются из exe-каталога (пользовательский
  словарь того же имени выигрывает). Один инвариант на все платформы: релизный zip,
  Windows-инсталлятор и Linux-пакеты кладут словари одинаково. (~30 строк +
  `Paths::bundled_dictionaries_dir()` + тесты.)
- **П2. Автоопределение языка по локали ОС** при отсутствии явного
  `default_language`: `Defaults.default_language: Lang` → `Option<Lang>`; `None` →
  детект по локали (крейт `sys-locale`, чистый и крошечный; `ru*` → `Ru`, иначе
  `En`; расширяемо на внешние локали Ярусa 3). Меняет поведение только свежих
  установок и новых профилей при отсутствии поля (сегодня — всегда `Ru`); Windows-
  инсталлятор пишет язык явно, Linux-пакеты и голый zip получают разумный дефолт.
  Закрывает задел roadmap «Определение языка по системной локали ОС».
- **П3. Терпимость `Defaults::read` к UTF-8 BOM** (отбрасывать до парса — прецеденты
  `rag_ingest::read_text`, импортёр LameLLaMA). Защищает от Pascal-хелперов Inno и
  ручной правки файла редакторами, пишущими BOM. (2 строки + тест.)

Все три — additive, без миграций (ADR 0006 не задевается).

---

## 7. Развилки (подтвердить до реализации)

- **Р1. Формат Windows-инсталлятора.**
  - (a) **[рек.]** Inno Setup 6.7.x (exe): все пять особых требований — штатные
    возможности, 1–2 дня работы, официальный русский перевод, бесплатен для OSS,
    есть на GHA. MSI — задел при спросе на GPO/Intune.
  - (b) WiX MSI: «настоящий» установочный формат Windows, но каждое наше требование
    — обходной приём (кастомные диалоги, JSON через CustomAction, мультиязычие
    трансформами), дни-недели работы + вилка EOL-v3/платный-v6.
  - (c) Оба сразу — двойная стоимость сопровождения без спроса.
- **Р2. Режим установки Windows.**
  - (a) **[рек.]** per-user по умолчанию (`PrivilegesRequired=lowest`, без UAC,
    `%LOCALAPPDATA%\Programs`) + диалог «для меня / для всех»
    (`…OverridesAllowed=dialog`). При per-machine вариант «портативно» скрывается.
  - (b) только per-machine (Program Files, UAC) — портативный вариант данных
    становится недоступен, а выгоды нет.
- **Р3. Что пишет инсталлятор в `default_language`.**
  - (a) **[рек.]** отдельная страница «язык приложения» с дефолтом от языка
    инсталлятора (выбор языка инсталлятора ≠ обязательный выбор языка приложения,
    но хороший дефолт).
  - (b) молча взять язык инсталлятора без страницы — меньше кликов, но выбор
    неявный.
- **Р4. Инструмент Linux-пакетов.**
  - (a) **[рек.]** nfpm: один YAML → deb+rpm+archlinux, идентичная раскладка,
    symlink/config|noreplace из коробки, активнейшее сопровождение.
  - (b) cargo-deb + cargo-generate-rpm (+ отдельное решение для arch): метаданные в
    Cargo.toml и авто-Depends, но три конфига и arch не покрыт.
- **Р5. Раскладка Linux-пакета.**
  - (a) **[рек.]** `/usr/lib/mindfork-rs/` (бинарь + `defaults.json` +
    `data/dictionaries`) + симлинк `/usr/bin/mindfork-rs` — ноль правок кода
    резолва путей, Policy-совместимо.
  - (b) бинарь в `/usr/bin` + приложение учится читать `/etc/mindfork-rs/defaults.json`
    и `/usr/share/mindfork-rs/` — FHS-пуризм ценой второй механики поиска в коде.
- **Р6. Язык при неинтерактивной установке (П2).**
  - (a) **[рек.]** `default_language: Option<Lang>` + автоопределение по локали ОС
    при `None` (Linux-пакеты и zip получают язык системы; закрывает задел roadmap).
  - (b) Linux-пакеты пишут фиксированный `"en"` — предсказуемо, но русскоязычная
    система получит английский первый профиль.
  - (c) оставить как есть (`ru` при отсутствии поля) — плохой дефолт для
    международных пакетов.
- **Р7. Словари в установленном виде (П1).**
  - (a) **[рек.]** fallback-каталог `exe_dir/data/dictionaries` (read-only ресурсы
    рядом с бинарником; пакеты и инсталлятор кладут словари туда).
  - (b) копировать словари в каталог данных при первом запуске — дубли на диске,
    устаревание копий.
  - (c) не решать — в установленном виде спелл-чек молча выключен (текущее
    поведение).
- **Р8. Подпись Windows.**
  - (a) **[рек. при публичном репозитории]** SignPath Foundation (бесплатно,
    рекомендация Microsoft; издатель «SignPath Foundation», ручное одобрение
    релизов).
  - (b) **[рек. иначе / «своё имя»]** стартовать без подписи, отдельным этапом —
    Certum Open Source (~€69/€29, физлицо любой страны); в winget до подписи
    отдавать портативный zip, не inno-exe (§5.4).
  - (c) Azure Artifact Signing $9.99/мес — когда пройдёт по географии
    (физлицо: США/Канада; юрлицо: +ЕС/UK); перепроверять статус.
  - (d) коммерческий OV (~$150–300) / EV — EV не даёт больше ничего, не брать.
- **Р9. Базлайн glibc для deb/rpm.**
  - (a) **[рек.]** оставить `ubuntu-22.04` (glibc 2.35), честно задекларировав
    «RHEL/Rocky 9 не поддержан» (как сейчас в install.md).
  - (b) `cargo-zigbuild` с пином glibc 2.34 ради EL9 (не проверялось).
  - (c) musl-static (нужен mimalloc против деградации аллокатора).

Микро-решения, которые предлагаю зафиксировать без развилки (сказать, если не так):
страницы выбора пропускаются при апгрейде (`defaults.json` существует);
деинсталлятор данные не трогает; артефакты называются
`mindfork-rs-vX.Y.Z-x86_64-setup.exe` + конвенционные имена пакетов
(`mindfork-rs_X.Y.Z-1_amd64.deb`, `mindfork-rs-X.Y.Z-1.x86_64.rpm`,
`mindfork-rs-X.Y.Z-1-x86_64.pkg.tar.zst`); всё попадает в `sha256sums.txt`.

---

## 8. План этапов (после подтверждения развилок; этап = ветка/PR)

| Этап | Ветка | Содержимое | DoD |
|---|---|---|---|
| 1. Предпосылки в коде | `feat/installed-mode-prereqs` | П1 (fallback словарей) + П2 (`Option<Lang>` + автодетект локали) + П3 (BOM) + доки (install.md §2.1, spec §5.2) | юнит-тесты; ручной смоук: бинарь + `defaults.json {"mode":"system"}` в чистом каталоге → словари подхвачены из `data/`, язык по локали |
| 2. Linux-пакеты | `feat/linux-packages` | `packaging/nfpm.yaml` (раскладка §4.2), job в `release.yml` (nfpm → deb/rpm/archlinux → артефакты + sha256) | **CI-смоук установки**: контейнеры `ubuntu:24.04` (`apt install ./…deb`), `fedora:latest` (`dnf install ./…rpm`), `archlinux:latest` (`pacman -U`) → `mindfork-rs --version` под обычным пользователем; `defaults.json` виден через симлинк |
| 3. Windows-инсталлятор | `feat/windows-installer` | `packaging/windows/mindfork.iss` (дизайн §3.3), job в `release.yml` (iscc → setup.exe) | ручной смоук на Windows: свежая установка per-user (обе страницы, все 3 режима данных), апгрейд поверх (страницы пропущены, `defaults.json` цел), тихая установка, uninstall не трогает данные |
| 4. (опц.) Подпись | `feat/windows-signing` | по Р8: SignPath/Certum-интеграция в `release.yml` (подпись exe + setup, timestamp) | подписанный артефакт: `signtool verify /pa`, свойства файла показывают издателя |
| 5. (заделы) | — | winget-манифест (до подписи — zip, §5.4), AUR `mindfork-rs-bin`, GitHub Artifact Attestations, MSI при спросе, apt/COPR-репозитории | — |

Живой прогон движка направлению не нужен (движок/память/инструменты не
затрагиваются) — вместо него смоуки установки из DoD. Документация по AGENTS.md §4:
install.md (новые способы установки), README (бейджи/ссылки), roadmap (задел
закрыт), CHANGELOG (`[Unreleased]` → Добавлено), журнал CLAUDE.md.

---

## 9. Вне объёма

- Авто-обновление (self-update) и уведомление «доступна новая версия» в TUI —
  отдельное направление (release-engineering §5).
- macOS (dmg/homebrew), arm64-сборки, flatpak/snap/AppImage.
- Собственные репозитории (apt PPA, dnf COPR, pacman-репо) — тогда же и GPG.
- Локализация инсталлятора сверх ru/en (Inno умеет 30+ языков — добавляется парой
  строк, когда появятся внешние локали приложения на этих языках).

---

## 10. Ключевые источники (проверено 2026-07-15)

**Windows-инструменты:** [Inno Setup downloads](https://jrsoftware.org/isdl.php) ·
[официальные переводы (Russian.isl)](https://jrsoftware.org/files/istrans/) ·
[CreateInputOptionPage](https://jrsoftware.org/ishelp/topic_isxfunc_createinputoptionpage.htm) /
[CreateInputDirPage](https://jrsoftware.org/ishelp/topic_isxfunc_createinputdirpage.htm) /
[события скрипта](https://jrsoftware.org/ishelp/topic_scriptevents.htm) ·
[PrivilegesRequiredOverridesAllowed](https://jrsoftware.org/ishelp/topic_setup_privilegesrequiredoverridesallowed.htm) ·
[Setup Command Line](https://jrsoftware.org/ishelp/topic_setupcmdline.htm) ·
[коммерческие лицензии Inno (6.5.0+)](https://jrsoftware.org/isorder.php) ·
[WiX releases](https://github.com/wixtoolset/wix/releases) ·
[EOL WiX v3/v4](https://www.firegiant.com/blog/2025/2/6/wix-v3-and-wix-v4-are-no-longer-in-community-support/) ·
[WiX Maintenance Fee](https://www.firegiant.com/blog/2025/4/7/wix-v600-available/) ·
[нет util:JsonFile](https://github.com/orgs/wixtoolset/discussions/7711) ·
[мультиязычный MSI — WIP](https://github.com/wixtoolset/issues/issues/7544) ·
[состав GHA windows-2025](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md) ·
[cargo-wix](https://github.com/volks73/cargo-wix/releases) ·
[cargo-dist](https://github.com/axodotdev/cargo-dist/releases) ·
[cargo-packager](https://github.com/crabnebula-dev/cargo-packager/releases) ·
[winget: манифесты/типы](https://learn.microsoft.com/en-us/windows/package-manager/package/manifest)

**Linux-пакетирование:** [nfpm: конфигурация/форматы](https://nfpm.goreleaser.com/docs/configuration/) ·
[nfpm releases](https://github.com/goreleaser/nfpm/releases) ·
[cargo-deb](https://github.com/kornelski/cargo-deb) ·
[cargo-generate-rpm](https://github.com/cat-in-136/cargo-generate-rpm) ·
[FHS 3.0 /usr/bin](https://refspecs.linuxfoundation.org/FHS_3.0/fhs/ch04s04.html) /
[/usr/lib](https://refspecs.linuxfoundation.org/FHS_3.0/fhs/ch04s06.html) ·
[Debian Policy §9.1.1](https://www.debian.org/doc/debian-policy/ch-opersys.html) ·
[current_exe резолвит симлинк на Linux](https://github.com/rust-lang/rust/issues/43617) ·
[AUR submission guidelines (-bin)](https://wiki.archlinux.org/title/AUR_submission_guidelines) ·
[debconf-devel(7): не реестр](https://manpages.debian.org/unstable/debconf-doc/debconf-devel.7.en.html) ·
[Securing Debian: подпись deb](https://www.debian.org/doc/manuals/securing-debian-manual/deb-pack-sign.en.html) ·
[pacman SigLevel](https://wiki.archlinux.org/title/Pacman/Package_signing) ·
[Artifact Attestations GA](https://github.blog/changelog/2024-06-25-artifact-attestations-is-generally-available/) ·
[glibc RHEL10/2.39](https://lwn.net/Articles/1021827/) ·
[musl-аллокатор и производительность](https://nickb.dev/blog/default-musl-allocator-considered-harmful-to-performance/)

**Подпись кода:** [SmartScreen reputation (MS, 2026)](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation) ·
[Code signing options (MS, 2026; рекомендация SignPath для OSS)](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options) ·
[Artifact Signing FAQ (география)](https://learn.microsoft.com/en-us/azure/artifact-signing/faq) ·
[azure/artifact-signing-action](https://github.com/Azure/trusted-signing-action) ·
[SignPath Foundation: условия](https://signpath.org/terms.html) ·
[SignPath GitHub Action](https://github.com/SignPath/github-action-submit-signing-request) ·
[Certum Open Source](https://www.certum.eu/en/code-signing-certificates/) ·
[hands-on Certum 10.2025 (цены)](https://piers.rocks/2025/10/30/certum-open-source-code-sign.html) ·
[CA/B: аппаратные ключи с 06.2023](https://cabforum.org/working-groups/code-signing/requirements/) ·
[CSC-31: 460 дней с 03.2026](https://cabforum.org/2025/11/17/ballot-csc-31-maximum-validity-reduction/) ·
[EV не обходит SmartScreen (ToDesktop)](https://www.todesktop.com/blog/posts/windows-apps-psa-ev-certs-do-not-grant-immediate-reputation-anymore) ·
[winget: подпись не требуется](https://github.com/microsoft/winget-cli/discussions/4327) ·
[кейс halloy: неподписанный inno-exe в winget](https://github.com/microsoft/winget-pkgs/issues/385483)
