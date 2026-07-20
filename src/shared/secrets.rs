//! Машинно-привязанное хранение секретов (API-ключей облачных провайдеров).
//!
//! Ключ, введённый в настройках, шифруется **ключом этой машины** и кладётся в
//! `settings.json` (см. [`crate::shared::config::AppConfig::api_keys`]). Конфиг
//! остаётся переносимым: на другой машине запись не расшифруется — ключ вводится
//! заново и добавляется **своей** записью; при возврате на первую машину её запись
//! по-прежнему читается. См. docs/research/api-key-storage.md.
//!
//! Схемы шифрования (поле `scheme` записи, свободная строка — незнакомая схема не
//! валит чтение конфига, запись просто «не наша»):
//!
//! * **`dpapi`** (Windows) — системный `CryptProtectData`/`CryptUnprotectData`:
//!   мастер-ключом *пользователя*, которым управляет ОС. Расшифровка на другой
//!   машине или другим пользователем невозможна. `pOptionalEntropy` — константа
//!   приложения ([`ENTROPY`]): не секрет, но отсекает generic-«DPAPI-дамперы».
//! * **`machine-key-v1`** (Linux) — ключ выводится из `/etc/machine-id` через
//!   HKDF-SHA256 (паттерн `sd_id128_get_machine_app_specific` — systemd прямо
//!   предписывает не использовать machine-id сырым), шифрование —
//!   ChaCha20-Poly1305 (AEAD, случайный nonce префиксом к шифротексту). Имя
//!   пользователя входит в `info` → привязка per-user, как у DPAPI.
//!
//! **Модель угроз** (docs/research/api-key-storage.md §3): защищаем **файл** —
//! копию/перенос/бэкап конфига (вне «своей» машины это бесполезный шифротекст).
//! От вредоносного кода, исполняющегося под тем же пользователем на той же машине,
//! не защищает — он вызовет тот же DPAPI/выведет тот же ключ. Это фундаментально
//! для любой схемы «приложение расшифровывает само, без ввода пользователя» (так же
//! устроены Chrome, Git Credential Manager). Прежний путь (env-переменная) не
//! безопаснее: её читает любой процесс пользователя.

use serde::{Deserialize, Serialize};

/// Схема Windows DPAPI (значение поля `scheme`).
// Вне Windows схема недоступна (запись с ней — «не наша»), поэтому константа там
// в коде не встречается: гасим dead_code, чтобы гейт `-D warnings` был зелёным на обеих ОС.
#[cfg_attr(not(windows), allow(dead_code))]
pub const SCHEME_DPAPI: &str = "dpapi";
/// Схема Linux: HKDF(machine-id) + ChaCha20-Poly1305.
pub const SCHEME_MACHINE_KEY_V1: &str = "machine-key-v1";

/// Плейнтекст пробы `check`: шифруется вместе с ключами, по успеху её расшифровки
/// запись опознаётся как «наша» (явного machine-id в переносимом конфиге не храним).
const CHECK_PLAINTEXT: &str = "mindfork-rs api-key check v1";

/// Дополнительная энтропия DPAPI / соль HKDF — константа приложения (не секрет).
const ENTROPY: &[u8] = b"mindfork-rs/api-keys/v1";

/// Строка `info` HKDF (домен ключа; к ней добавляется имя пользователя).
const HKDF_INFO: &[u8] = b"mindfork-rs api-key v1 user=";

/// Одна запись сохранённых API-ключей — **на одну машину**. Записи чужих машин
/// лежат рядом и не трогаются (они «оживут» на своих машинах): конфиг переносим,
/// ключи пер-машинные.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiKeyEntry {
    /// Человекочитаемая метка (имя компьютера + дата) — только для показа/диагностики,
    /// в логике не участвует.
    pub label: String,
    /// Схема шифрования: [`SCHEME_DPAPI`] | [`SCHEME_MACHINE_KEY_V1`]. Свободная
    /// строка — запись с незнакомой (будущей) схемой читается и сохраняется как есть.
    pub scheme: String,
    /// Шифротекст [`CHECK_PLAINTEXT`] — проба «наша ли запись» ([`is_ours`]).
    pub check: String,
    /// Шифротексты ключей по провайдеру (`openai`/`gemini`/`claude`).
    pub keys: std::collections::BTreeMap<String, String>,
}

/// Доступна ли на этой машине схема шифрования секретов (иначе сохранённые ключи
/// не поддерживаются — остаётся env-путь). На Windows — всегда; на Linux зависит от
/// наличия machine-id.
// Потребители: тесты (пропуск на системах без machine-id) и экран настроек этапа 2
// (поле «API-ключ» скрывается/поясняется, когда сохранение недоступно).
#[allow(dead_code)]
pub fn scheme_available() -> bool {
    local_scheme().is_some()
}

/// Опознаёт «нашу» запись — расшифровкой пробы `check` (DPAPI вернул успех /
/// AEAD-аутентификация сошлась). Записи других машин и других схем — `false`.
pub fn is_ours(entry: &ApiKeyEntry) -> bool {
    decrypt(&entry.scheme, &entry.check).as_deref() == Some(CHECK_PLAINTEXT)
}

/// Расшифрованный ключ провайдера из «нашей» записи; `None` — записи нет, она чужая
/// или ключ этого провайдера в ней не задан.
pub fn stored_key(entries: &[ApiKeyEntry], provider_key: &str) -> Option<String> {
    let entry = entries.iter().find(|e| is_ours(e))?;
    decrypt(&entry.scheme, entry.keys.get(provider_key)?)
}

/// Кладёт ключ провайдера в запись **этой** машины (создаёт её при отсутствии).
/// Пустой `key` — удаляет ключ; опустевшая запись убирается целиком. Чужие записи
/// не трогаются. `Err` — схема на этой машине недоступна или сбой шифрования.
pub fn put_key(
    entries: &mut Vec<ApiKeyEntry>,
    provider_key: &str,
    key: &str,
    label: impl FnOnce() -> String,
) -> Result<(), SecretError> {
    let idx = entries.iter().position(is_ours);
    if key.is_empty() {
        if let Some(i) = idx {
            entries[i].keys.remove(provider_key);
            if entries[i].keys.is_empty() {
                entries.remove(i);
            }
        }
        return Ok(());
    }
    let scheme = local_scheme().ok_or(SecretError::Unavailable)?;
    let cipher = encrypt(scheme, key)?;
    match idx {
        Some(i) => {
            entries[i].keys.insert(provider_key.into(), cipher);
        }
        None => entries.push(ApiKeyEntry {
            label: label(),
            check: encrypt(scheme, CHECK_PLAINTEXT)?,
            scheme: scheme.into(),
            keys: std::collections::BTreeMap::from([(provider_key.into(), cipher)]),
        }),
    }
    Ok(())
}

/// Имя компьютера для метки записи (диагностика: чья это запись). Только для
/// показа — в логике опознания записи не участвует (см. [`is_ours`]). Фолбэк — `?`.
pub fn machine_label() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "?".into())
}

/// Ошибка работы с сохранёнными секретами.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// На этой машине нет доступной схемы (Linux без machine-id) — сохранение
    /// ключей не поддерживается, остаётся env-путь.
    #[error("на этой машине не поддерживается сохранение ключей (нет machine-id)")]
    Unavailable,
    /// Сбой платформенного шифрования (DPAPI/AEAD).
    #[error("не удалось зашифровать секрет")]
    Encrypt,
}

/// Схема шифрования этой машины (`None` — недоступна).
fn local_scheme() -> Option<&'static str> {
    #[cfg(windows)]
    {
        Some(SCHEME_DPAPI)
    }
    #[cfg(not(windows))]
    {
        machine_ikm().map(|_| SCHEME_MACHINE_KEY_V1)
    }
}

/// Шифрует секрет указанной схемой → hex-строка для JSON.
fn encrypt(scheme: &str, plaintext: &str) -> Result<String, SecretError> {
    let bytes = match scheme {
        #[cfg(windows)]
        SCHEME_DPAPI => dpapi::protect(plaintext.as_bytes()).ok_or(SecretError::Encrypt)?,
        SCHEME_MACHINE_KEY_V1 => {
            let key = machine_key().ok_or(SecretError::Unavailable)?;
            encrypt_with_key(&key, plaintext.as_bytes()).ok_or(SecretError::Encrypt)?
        }
        _ => return Err(SecretError::Unavailable),
    };
    Ok(hex_encode(&bytes))
}

/// Расшифровывает hex-строку указанной схемой. `None` — чужая машина, незнакомая
/// схема, порча данных (все случаи равнозначны: «прочитать нельзя»).
fn decrypt(scheme: &str, hex: &str) -> Option<String> {
    let bytes = hex_decode(hex)?;
    let plain = match scheme {
        #[cfg(windows)]
        SCHEME_DPAPI => dpapi::unprotect(&bytes)?,
        SCHEME_MACHINE_KEY_V1 => decrypt_with_key(&machine_key()?, &bytes)?,
        _ => return None,
    };
    String::from_utf8(plain).ok()
}

// ── Схема `machine-key-v1`: HKDF(machine-id) + ChaCha20-Poly1305 ────────────────

/// Длина nonce ChaCha20-Poly1305 (префикс шифротекста).
const NONCE_LEN: usize = 12;

/// Выводит 32-байтовый ключ из входного материала (machine-id) и имени пользователя.
/// Чистая функция — тестируется на любой ОС инъекцией `ikm`.
fn derive_key(ikm: &[u8], user: &str) -> [u8; 32] {
    let mut info = HKDF_INFO.to_vec();
    info.extend_from_slice(user.as_bytes());
    let mut okm = [0u8; 32];
    // `expand` в 32 байта (= размер выхода SHA-256) не может превысить лимит HKDF.
    hkdf::Hkdf::<sha2::Sha256>::new(Some(ENTROPY), ikm)
        .expand(&info, &mut okm)
        .expect("HKDF: 32 байта всегда допустимы");
    okm
}

/// Шифрует `plaintext`: результат = `nonce || ciphertext+tag`. Чистая функция.
fn encrypt_with_key(key: &[u8; 32], plaintext: &[u8]) -> Option<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, OsRng};
    use chacha20poly1305::{AeadCore, ChaCha20Poly1305, KeyInit};

    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let mut out = nonce.to_vec();
    out.extend_from_slice(&cipher.encrypt(&nonce, plaintext).ok()?);
    Some(out)
}

/// Расшифровывает `nonce || ciphertext+tag`. `None` — чужой ключ (AEAD не
/// аутентифицировался), порча или слишком короткий вход. Чистая функция.
fn decrypt_with_key(key: &[u8; 32], data: &[u8]) -> Option<Vec<u8>> {
    use chacha20poly1305::aead::Aead;
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit};

    if data.len() <= NONCE_LEN {
        return None;
    }
    let (nonce, ct) = data.split_at(NONCE_LEN);
    ChaCha20Poly1305::new(key.into())
        .decrypt(nonce.into(), ct)
        .ok()
}

/// Ключ этой машины для схемы `machine-key-v1` (`None` — нет machine-id).
fn machine_key() -> Option<[u8; 32]> {
    Some(derive_key(&machine_ikm()?, &current_user()))
}

/// Входной материал ключа: идентификатор экземпляра ОС. Основной источник —
/// `/etc/machine-id` (systemd), фолбэк — `/var/lib/dbus/machine-id`. `None` —
/// не-systemd система без обоих (схема недоступна, остаётся env-путь).
/// На Windows не используется (там DPAPI).
fn machine_ikm() -> Option<Vec<u8>> {
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(s) = std::fs::read_to_string(path) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.as_bytes().to_vec());
            }
        }
    }
    None
}

/// Имя текущего пользователя (компонент `info` HKDF → привязка per-user). Пустое
/// имя допустимо — привязка тогда только к машине.
fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
}

// ── Схема `dpapi` (Windows) ────────────────────────────────────────────────────

#[cfg(windows)]
mod dpapi {
    use super::ENTROPY;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData,
    };

    /// Шифрует данные DPAPI (ключом пользователя; управляет ОС). `None` — сбой winapi.
    pub(super) fn protect(data: &[u8]) -> Option<Vec<u8>> {
        crypt(data, true)
    }

    /// Расшифровывает данные DPAPI. `None` — другая машина/пользователь, порча,
    /// иная энтропия (все случаи равнозначны: «прочитать нельзя»).
    pub(super) fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        crypt(data, false)
    }

    /// Общая обёртка: оба вызова DPAPI имеют одинаковую форму (blob in → blob out,
    /// выходной буфер выделяет ОС и его нужно освободить `LocalFree`).
    fn crypt(data: &[u8], protect: bool) -> Option<Vec<u8>> {
        let input = blob(data);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: `input`/`entropy` указывают на живые срезы на всё время вызова;
        // прочие указатели — нулевые (описание/reserved/prompt не используем);
        // `out` заполняет ОС, освобождаем `LocalFree` строго один раз ниже.
        let ok = unsafe {
            if protect {
                CryptProtectData(
                    &input,
                    std::ptr::null(),
                    &entropy,
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut out,
                )
            } else {
                CryptUnprotectData(
                    &input,
                    std::ptr::null_mut(),
                    &entropy,
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut out,
                )
            }
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        // SAFETY: при успехе ОС гарантирует валидный буфер длиной `cbData`.
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
        // SAFETY: `pbData` выделен ОС именно под `LocalFree`; больше не используется.
        unsafe { LocalFree(out.pbData as *mut core::ffi::c_void) };
        Some(bytes)
    }

    /// Оборачивает срез в DPAPI-blob (структура «длина + указатель»).
    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }
}

// ── hex-кодек (шифротекст в JSON) ──────────────────────────────────────────────
// Своя реализация вместо крейта base64 — прецедент `features::sandbox_setup::hex_lower`;
// разница в размере строки для конфига несущественна, зависимость не нужна.

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap_or('0'));
    }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let b = s.as_bytes();
    (0..b.len() / 2)
        .map(|i| {
            let hi = (b[i * 2] as char).to_digit(16)?;
            let lo = (b[i * 2 + 1] as char).to_digit(16)?;
            Some(((hi << 4) | lo) as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_and_rejects_malformed() {
        let data = vec![0u8, 1, 15, 16, 200, 255];
        assert_eq!(hex_decode(&hex_encode(&data)).unwrap(), data);
        assert_eq!(hex_encode(&[0xab, 0x0f]), "ab0f");
        assert!(hex_decode("abc").is_none()); // нечётная длина
        assert!(hex_decode("zz").is_none()); // не hex
    }

    #[test]
    fn aead_round_trip_with_derived_key() {
        let key = derive_key(b"machine-id-abc", "user1");
        let enc = encrypt_with_key(&key, b"sk-secret-value").unwrap();
        assert_ne!(&enc[NONCE_LEN..], b"sk-secret-value"); // не плейнтекст
        assert_eq!(decrypt_with_key(&key, &enc).unwrap(), b"sk-secret-value");
    }

    #[test]
    fn aead_rejects_foreign_key_and_tampering() {
        let mine = derive_key(b"machine-A", "user1");
        let theirs = derive_key(b"machine-B", "user1");
        let other_user = derive_key(b"machine-A", "user2");
        let enc = encrypt_with_key(&mine, b"secret").unwrap();
        // Другая машина и другой пользователь той же машины прочитать не могут.
        assert!(decrypt_with_key(&theirs, &enc).is_none());
        assert!(decrypt_with_key(&other_user, &enc).is_none());
        // Порча шифротекста ловится аутентификацией AEAD.
        let mut bad = enc.clone();
        *bad.last_mut().unwrap() ^= 0xff;
        assert!(decrypt_with_key(&mine, &bad).is_none());
        // Слишком короткий вход (нет даже nonce) — не паника, а `None`.
        assert!(decrypt_with_key(&mine, &[0u8; NONCE_LEN]).is_none());
    }

    #[test]
    fn nonce_is_random_so_ciphertexts_differ() {
        let key = derive_key(b"machine-id", "u");
        let a = encrypt_with_key(&key, b"same").unwrap();
        let b = encrypt_with_key(&key, b"same").unwrap();
        assert_ne!(
            a, b,
            "одинаковый плейнтекст не должен давать одинаковый шифротекст"
        );
    }

    #[test]
    fn derive_key_is_deterministic_and_domain_separated() {
        assert_eq!(derive_key(b"m", "u"), derive_key(b"m", "u"));
        assert_ne!(derive_key(b"m", "u"), derive_key(b"m", "v"));
        assert_ne!(derive_key(b"m", "u"), derive_key(b"n", "u"));
    }

    /// Полный цикл поверх записи конфига — на платформенной схеме этой машины
    /// (Windows: DPAPI; Linux: machine-id, если он есть — иначе тест пропускается).
    #[test]
    fn entry_round_trip_on_local_scheme() {
        if !scheme_available() {
            return; // не-systemd Linux без machine-id: сохранение ключей не поддержано
        }
        let mut entries: Vec<ApiKeyEntry> = vec![];
        put_key(&mut entries, "openai", "sk-test-123", || "test".into()).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(is_ours(&entries[0]));
        // Секрет не лежит открытым текстом.
        let json = serde_json::to_string(&entries).unwrap();
        assert!(
            !json.contains("sk-test-123"),
            "плейнтекст утёк в сериализацию: {json}"
        );
        assert_eq!(
            stored_key(&entries, "openai").as_deref(),
            Some("sk-test-123")
        );
        assert_eq!(stored_key(&entries, "claude"), None);
        // Второй провайдер кладётся в ту же запись.
        put_key(&mut entries, "claude", "sk-ant-9", || "test".into()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(stored_key(&entries, "claude").as_deref(), Some("sk-ant-9"));
        // Пустой ключ удаляет провайдера; опустевшая запись исчезает.
        put_key(&mut entries, "openai", "", || "test".into()).unwrap();
        assert_eq!(stored_key(&entries, "openai"), None);
        assert_eq!(entries.len(), 1);
        put_key(&mut entries, "claude", "", || "test".into()).unwrap();
        assert!(entries.is_empty());
    }

    /// Запись чужой машины (нерасшифровываемая) и запись с незнакомой схемой не
    /// опознаются как наши, ключей не отдают и **не трогаются** при правке.
    #[test]
    fn foreign_entries_are_ignored_and_preserved() {
        if !scheme_available() {
            return;
        }
        let foreign = ApiKeyEntry {
            label: "other-pc".into(),
            scheme: SCHEME_MACHINE_KEY_V1.into(),
            check: hex_encode(&[7u8; 40]), // чужой шифротекст
            keys: std::collections::BTreeMap::from([("openai".into(), hex_encode(&[9u8; 40]))]),
        };
        let future = ApiKeyEntry {
            label: "future-pc".into(),
            scheme: "keychain-v9".into(), // схема, которой мы не знаем
            check: "00".into(),
            keys: std::collections::BTreeMap::from([("openai".into(), "00".into())]),
        };
        let mut entries = vec![foreign.clone(), future.clone()];
        assert!(!is_ours(&entries[0]) && !is_ours(&entries[1]));
        assert_eq!(stored_key(&entries, "openai"), None);

        put_key(&mut entries, "openai", "sk-mine", || "mine".into()).unwrap();
        assert_eq!(
            entries.len(),
            3,
            "наша запись добавляется, чужие не заменяются"
        );
        assert_eq!(entries[0], foreign, "чужая запись не тронута");
        assert_eq!(entries[1], future, "запись незнакомой схемы не тронута");
        assert_eq!(stored_key(&entries, "openai").as_deref(), Some("sk-mine"));
    }
}
