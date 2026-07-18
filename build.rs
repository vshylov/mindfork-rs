//! Build-скрипт: словари спелл-чека рядом с бинарником + иконка Windows-`.exe`.
//!
//! **Словари.** В портативном режиме приложение читает данные из подкаталога `data/`
//! рядом с исполняемым файлом (в dev — `target/<profile>/data/`, см. shared/paths.rs),
//! значит словари ожидаются в `data/dictionaries/`. Словари лежат в `dictionaries/`
//! корня проекта (в репозитории; их же упаковывает релизный workflow в
//! `data/dictionaries/`); этот скрипт копирует их в выходной каталог, чтобы
//! `cargo run` сразу видел спелл-чек без ручного копирования. Отсутствие словарей —
//! не ошибка (спелл-чек просто выключится).
//!
//! **Иконка.** Под Windows-таргет в `.exe` вшивается ресурс иконки из
//! `artwork/mindfork.ico` — иначе Проводник, таскбар и Alt+Tab показывают дефолтную
//! иконку. Отсюда же её бесплатно подхватывают ярлыки и `UninstallDisplayIcon`
//! инсталлятора (`packaging/windows/mindfork.iss`). См. docs/branding.md §4.1.

use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    // Перекопировать только при изменении исходных словарей.
    println!("cargo:rerun-if-changed=dictionaries");

    embed_windows_icon();

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("dictionaries");
    if !src.is_dir() {
        return; // словарей нет — ничего не копируем
    }

    let Some(profile_dir) = profile_dir() else {
        println!("cargo:warning=не удалось определить каталог сборки для словарей");
        return;
    };
    // Портативный корень данных = `<profile>/data/` (см. shared/paths.rs).
    let dst = profile_dir.join("data").join("dictionaries");
    if let Err(err) = copy_dir(&src, &dst) {
        println!("cargo:warning=не удалось скопировать словари: {err}");
    }
}

/// Вшивает иконку в Windows-`.exe` (ресурс `IDI_ICON1`) — вариант для Windows-хоста.
///
/// Двойной гейт неизбежен: `winresource` объявлен в
/// `[target.'cfg(windows)'.build-dependencies]`, а у **build**-зависимостей `cfg`
/// вычисляется по **хосту** (build-скрипт исполняется на нём) — значит на Linux-хосте
/// крейта нет и обращение к нему не скомпилируется. Поэтому `#[cfg(windows)]` по хосту
/// (есть ли крейт) плюс проверка `CARGO_CFG_TARGET_OS` по таргету (нужна ли иконка).
///
/// Сбой намеренно **не валит сборку**, а уходит в `cargo:warning`: `winresource` на
/// MSVC-таргете зовёт `rc.exe` из Windows SDK, и на машине без SDK приложение всё
/// равно должно собираться — иконка косметическая.
#[cfg(windows)]
fn embed_windows_icon() {
    println!("cargo:rerun-if-changed=artwork/mindfork.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return; // Windows-хост, но таргет другой — ресурс неприменим
    }
    let icon = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("artwork/mindfork.ico");
    if !icon.is_file() {
        println!(
            "cargo:warning=иконка не найдена, .exe будет без неё: {}",
            icon.display()
        );
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon.to_string_lossy().as_ref());
    if let Err(err) = res.compile() {
        println!("cargo:warning=не удалось вшить иконку в .exe: {err}");
    }
}

/// Вариант для не-Windows хоста: `winresource` недоступен (см. выше).
///
/// Релизный workflow собирает Windows на windows-раннере, так что штатный путь не
/// страдает. Предупреждаем только при кросс-сборке Linux → Windows, чтобы молча
/// не выдать `.exe` без иконки.
#[cfg(not(windows))]
fn embed_windows_icon() {
    println!("cargo:rerun-if-changed=artwork/mindfork.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!(
            "cargo:warning=кросс-сборка под Windows с не-Windows хоста: \
             иконка в .exe не вшита (winresource доступен только на Windows-хосте)"
        );
    }
}

/// Каталог с бинарником (`target/<profile>/`), выведенный из `OUT_DIR`:
/// `…/target/<profile>/build/<crate>-<hash>/out` → на 3 уровня вверх.
fn profile_dir() -> Option<PathBuf> {
    let out = PathBuf::from(env::var("OUT_DIR").ok()?);
    out.ancestors().nth(3).map(Path::to_path_buf)
}

/// Копирует обычные файлы из `src` в `dst` (без рекурсии и скрытых файлов).
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        // пропускаем скрытые (например .gitkeep) и подкаталоги
        if name.to_string_lossy().starts_with('.') || !path.is_file() {
            continue;
        }
        fs::copy(&path, dst.join(&name))?;
    }
    Ok(())
}
