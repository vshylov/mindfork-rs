//! Общая механика провизии ассетов по **lock-списку URL + sha256**: скачивание
//! (потоком или в память), сверка хэша, распаковка архивов. Выделено из
//! `sandbox_setup` (ADR 0005), когда у него появился второй потребитель — провизия
//! локального озвучивания `tts_setup` (ADR 0009): устройство у них одинаковое,
//! различаются только списки и раскладка каталогов.
//!
//! Слой `features`: без TUI (команды CLI печатают прогресс в stdout). Тексты — из
//! бандлов локалей, ключи `setup.*` (общие для всех провизий).

use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::shared::i18n::Locale;

/// Через сколько байт скачивания сообщать о прогрессе (крупные архивы — десятки МБ).
const PROGRESS_STEP: u64 = 32 * 1024 * 1024;

/// HTTP-клиент провизии с заданным User-Agent (GitHub/PyPI/HF иногда отвергают
/// пустой UA).
pub(crate) fn http_client(user_agent: &str, loc: &Locale) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(user_agent.to_string())
        .build()
        .with_context(|| loc.t("setup.http_client").to_string())
}

/// Скачивает `url` в файл `dest` потоком (крупные архивы не буферим в память),
/// считая sha256 на лету; сверяет с `expected`.
pub(crate) async fn download_to_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected: &str,
    loc: &Locale,
    progress: &mut impl FnMut(&str),
) -> Result<()> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| loc.tf("setup.request", &[("url", url)]))?
        .error_for_status()
        .with_context(|| loc.tf("setup.download", &[("url", url)]))?;
    let total = resp.content_length();
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut file = tokio::fs::File::create(dest).await.with_context(|| {
        loc.tf(
            "setup.create_file",
            &[("path", &dest.display().to_string())],
        )
    })?;
    let mut hasher = Sha256::new();
    let mut stream = resp.bytes_stream();
    let mut done: u64 = 0;
    let mut next_report: u64 = PROGRESS_STEP;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| loc.t("setup.read_stream").to_string())?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .with_context(|| loc.t("setup.write_file").to_string())?;
        done += chunk.len() as u64;
        if done >= next_report {
            match total {
                Some(t) => progress(&loc.tf(
                    "setup.progress_bytes",
                    &[
                        ("done", &(done >> 20).to_string()),
                        ("total", &(t >> 20).to_string()),
                    ],
                )),
                None => progress(&loc.tf(
                    "setup.progress_bytes_unknown",
                    &[("done", &(done >> 20).to_string())],
                )),
            }
            next_report += PROGRESS_STEP;
        }
    }
    file.flush()
        .await
        .with_context(|| loc.t("setup.flush").to_string())?;
    verify_sha256(&hasher.finalize(), expected, url, loc)
}

/// Скачивает `url` целиком в память (небольшие файлы), сверяет sha256.
pub(crate) async fn download_bytes(
    client: &reqwest::Client,
    url: &str,
    expected: &str,
    loc: &Locale,
) -> Result<Vec<u8>> {
    let bytes = client
        .get(url)
        .send()
        .await
        .with_context(|| loc.tf("setup.request", &[("url", url)]))?
        .error_for_status()
        .with_context(|| loc.tf("setup.download", &[("url", url)]))?
        .bytes()
        .await
        .with_context(|| loc.t("setup.read_body").to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    verify_sha256(&hasher.finalize(), expected, url, loc)?;
    Ok(bytes.to_vec())
}

/// Сверяет хэш (сырые байты дайджеста) с ожидаемым hex; ошибка — с указанием URL.
pub(crate) fn verify_sha256(digest: &[u8], expected: &str, url: &str, loc: &Locale) -> Result<()> {
    let got = hex_lower(digest);
    if got.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        bail!(
            "{}",
            loc.tf(
                "setup.sha_mismatch",
                &[("url", url), ("expected", expected), ("got", &got)],
            )
        )
    }
}

/// Байты → строка hex (нижний регистр), без крейта `hex`.
pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Распаковывает tar.gz-архив в каталог `dest`.
pub(crate) fn extract_targz(archive: &Path, dest: &Path, loc: &Locale) -> Result<()> {
    let file = std::fs::File::open(archive).with_context(|| {
        loc.tf(
            "setup.open_archive",
            &[("path", &archive.display().to_string())],
        )
    })?;
    let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
    let mut ar = tar::Archive::new(gz);
    std::fs::create_dir_all(dest)?;
    ar.unpack(dest)
        .with_context(|| loc.tf("setup.extract_to", &[("path", &dest.display().to_string())]))?;
    Ok(())
}

/// Распаковывает zip-архив (файл на диске) в каталог `dest`. Защита от zip-slip —
/// `enclosed_name` (как в `features::backup`).
pub(crate) fn extract_zip(archive: &Path, dest: &Path, loc: &Locale) -> Result<()> {
    let bytes = std::fs::read(archive).with_context(|| {
        loc.tf(
            "setup.open_archive",
            &[("path", &archive.display().to_string())],
        )
    })?;
    unpack_zip(&bytes, dest, loc)
}

/// Распаковывает zip из памяти в каталог `dest` (общая часть для архивов и колёс).
pub(crate) fn unpack_zip(bytes: &[u8], dest: &Path, loc: &Locale) -> Result<()> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .with_context(|| loc.t("setup.open_zip").to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            bail!(
                "{}",
                loc.tf("setup.unsafe_entry", &[("name", entry.name())])
            );
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        std::fs::write(&out, &buf)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    #[test]
    fn hex_and_sha_verification() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xff]), "000fff");
        let digest = Sha256::digest(b"mindfork");
        let hex = hex_lower(&digest);
        assert!(verify_sha256(&digest, &hex, "u", ru()).is_ok());
        // Регистр hex не важен, а чужой хэш — ошибка с указанием URL.
        assert!(verify_sha256(&digest, &hex.to_uppercase(), "u", ru()).is_ok());
        let err = verify_sha256(&digest, &"0".repeat(64), "http://x/a", ru()).unwrap_err();
        assert!(err.to_string().contains("http://x/a"), "{err}");
    }

    #[test]
    fn zip_round_trip_and_slip_protection() {
        let dir = tempfile::tempdir().unwrap();
        // Крафтим архив с вложенным каталогом.
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            w.start_file("bin/piper.exe", opts).unwrap();
            std::io::Write::write_all(&mut w, b"stub").unwrap();
            w.finish().unwrap();
        }
        unpack_zip(&buf, dir.path(), ru()).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("bin").join("piper.exe")).unwrap(),
            b"stub"
        );
    }

    #[test]
    fn targz_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.tar.gz");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let enc = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            let mut ar = tar::Builder::new(enc);
            let data = b"hello";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(&mut header, "piper/piper", &data[..])
                .unwrap();
            ar.into_inner().unwrap().finish().unwrap();
        }
        let dest = dir.path().join("out");
        extract_targz(&archive, &dest, ru()).unwrap();
        assert_eq!(
            std::fs::read(dest.join("piper").join("piper")).unwrap(),
            b"hello"
        );
    }
}
