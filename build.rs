//! Build-скрипт: копирует словари спелл-чека рядом с бинарником.
//!
//! Приложение портативно и читает `dictionaries/` рядом с исполняемым файлом
//! (в dev — `target/<profile>/`, см. shared/paths.rs). Сами словари в репозиторий
//! не входят (см. .gitignore); этот скрипт копирует их из `dictionaries/` корня
//! проекта в выходной каталог, чтобы `cargo run` сразу видел спелл-чек без ручного
//! копирования. Отсутствие словарей — не ошибка (спелл-чек просто выключится).

use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    // Перекопировать только при изменении исходных словарей.
    println!("cargo:rerun-if-changed=dictionaries");

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("dictionaries");
    if !src.is_dir() {
        return; // словарей нет — ничего не копируем
    }

    let Some(profile_dir) = profile_dir() else {
        println!("cargo:warning=не удалось определить каталог сборки для словарей");
        return;
    };
    let dst = profile_dir.join("dictionaries");
    if let Err(err) = copy_dir(&src, &dst) {
        println!("cargo:warning=не удалось скопировать словари: {err}");
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
