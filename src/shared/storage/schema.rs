//! Версии схем сохраняемых данных и **чистый каркас JSON-миграций**.
//!
//! Единственный дом констант версий схем (пер-артефакт: настройки/профили/чаты/БД —
//! меняются с разной скоростью, один глобальный номер заставлял бы «мигрировать»
//! нетронутые файлы). Здесь — только Value-уровневая логика (определение версии,
//! прогон шагов); файловый I/O, бэкап и control-parse в типизированные структуры —
//! в [`crate::features::data_migration`] (оркестрация; `shared` не может зависеть от
//! `features`, а pre-migration бэкап живёт в `features::backup`). См.
//! [docs/history/release-engineering.md](../../../docs/history/release-engineering.md) §3.4 и ADR 0006.
//!
//! **Политика bump'а** (release-engineering.md Ф12): additive-изменение (новое поле с
//! `#[serde(default)]`, новая таблица/колонка с дефолтом) — **без bump**, как раньше;
//! breaking (переименование/перенос/смена семантики/удаление поля) — bump константы +
//! шаг миграции + golden-фикстура старого формата + пункт в CHANGELOG (рубрика «Данные»).

use std::cmp::Ordering;

use anyhow::{Context, Result};
use serde_json::Value;

/// Версия схемы `settings.json`. Совпадает с [`crate::shared::config::SCHEMA_VERSION`]
/// (дефолт поля `AppConfig.schema_version`) — инвариант проверяется тестом.
pub const SETTINGS_SCHEMA: u32 = 1;
/// Версия схемы `profiles.json`.
pub const PROFILES_SCHEMA: u32 = 1;
/// Версия схемы файла чата `chats/<id>.json`.
pub const CHAT_SCHEMA: u32 = 1;
/// Версия схемы SQLite (`PRAGMA user_version`). Раннер миграций БД — в
/// [`crate::shared::storage::db`] (baseline 0→1 + шаги в транзакциях).
pub const DB_SCHEMA: u32 = 1;

/// Шаг миграции JSON: чистая трансформация «версия `< to` → `to`».
pub struct Step {
    /// Целевая версия, к которой приводит шаг.
    pub to: u32,
    /// Краткое описание для лога миграции (не пользовательский текст). Потребитель —
    /// авторинг первой реальной миграции (сейчас шагов нет).
    #[allow(dead_code)]
    pub summary: &'static str,
    /// Трансформация значения. Должна быть чистой (без I/O).
    pub apply: fn(Value) -> Result<Value>,
}

/// Описание версионируемого JSON-артефакта: как определить версию значения и цепочка
/// шагов к текущей. Формат сегодня не меняется (все схемы = 1, `steps` пусты) —
/// каркас «в бою» на пустых миграциях; первая реальная миграция добавит шаг + фикстуру.
pub struct JsonArtifact {
    /// Имя для лога/ошибок (`"settings.json"`).
    pub name: &'static str,
    /// Текущая (целевая) версия схемы.
    pub current: u32,
    /// Определяет версию значения структурно (существующие файлы не переписываются:
    /// «нет поля версии → 1»).
    pub detect: fn(&Value) -> u32,
    /// Шаги `1→2→…→current`, по возрастанию `to`.
    pub steps: &'static [Step],
}

/// Вердикт по версии значения относительно текущей схемы артефакта.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assessment {
    /// Версия совпадает с текущей — миграция не нужна.
    UpToDate,
    /// Версия старше — нужен прогон шагов `from → current`.
    Migrate { from: u32 },
    /// Версия **новее** приложения (данные из более новой версии mindfork) — миграция
    /// невозможна; вызывающий отказывает в запуске (release-engineering.md Ф10).
    Downgrade { from: u32 },
}

impl JsonArtifact {
    /// Определяет версию значения и сравнивает с текущей.
    pub fn assess(&self, v: &Value) -> Assessment {
        let from = (self.detect)(v);
        match from.cmp(&self.current) {
            Ordering::Equal => Assessment::UpToDate,
            Ordering::Less => Assessment::Migrate { from },
            Ordering::Greater => Assessment::Downgrade { from },
        }
    }

    /// Прогоняет шаги `from → current`. Результат ещё не провалидирован типизированным
    /// парсом — это делает оркестратор (control-parse перед записью).
    pub fn apply_steps(&self, mut v: Value, from: u32) -> Result<Value> {
        for step in self.steps.iter().filter(|s| s.to > from) {
            v = (step.apply)(v)
                .with_context(|| format!("шаг миграции {} → v{}", self.name, step.to))?;
        }
        Ok(v)
    }
}

/// Версия `settings.json` — по полю `schema_version` (отсутствует → 1).
fn detect_settings(v: &Value) -> u32 {
    v.get("schema_version").and_then(Value::as_u64).unwrap_or(1) as u32
}

/// Версия `profiles.json` — исторически это голый массив (→ 1); форма-конверт с полем
/// `schema_version` появится при первом breaking-изменении.
fn detect_profiles(v: &Value) -> u32 {
    if v.is_array() {
        1
    } else {
        v.get("schema_version").and_then(Value::as_u64).unwrap_or(1) as u32
    }
}

/// Версия файла чата — по полю `v` (отсутствует → 1). Поле **не** пишется, пока
/// схема = 1 (ноль churn в существующих файлах).
fn detect_chat(v: &Value) -> u32 {
    v.get("v").and_then(Value::as_u64).unwrap_or(1) as u32
}

/// Реестр артефактов (реальный, все `current = 1`, `steps` пусты).
pub fn settings_artifact() -> JsonArtifact {
    JsonArtifact {
        name: "settings.json",
        current: SETTINGS_SCHEMA,
        detect: detect_settings,
        steps: &[],
    }
}

pub fn profiles_artifact() -> JsonArtifact {
    JsonArtifact {
        name: "profiles.json",
        current: PROFILES_SCHEMA,
        detect: detect_profiles,
        steps: &[],
    }
}

pub fn chat_artifact() -> JsonArtifact {
    JsonArtifact {
        name: "chats/<id>.json",
        current: CHAT_SCHEMA,
        detect: detect_chat,
        steps: &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schema_constants_match_config_default() {
        // Инвариант: дефолт поля `AppConfig.schema_version` и current настроек — заодно.
        assert_eq!(SETTINGS_SCHEMA, crate::shared::config::SCHEMA_VERSION);
    }

    #[test]
    fn detect_uses_field_or_defaults_to_one() {
        assert_eq!(detect_settings(&json!({"schema_version": 3})), 3);
        assert_eq!(detect_settings(&json!({})), 1);
        assert_eq!(detect_profiles(&json!([])), 1); // голый массив = v1
        assert_eq!(detect_profiles(&json!({"schema_version": 2})), 2);
        assert_eq!(detect_chat(&json!({"v": 5})), 5);
        assert_eq!(detect_chat(&json!({"title": "x"})), 1);
    }

    // Синтетический артефакт «current = 2» со ступенью 1→2 — проверяет движок отдельно
    // от реального реестра (где все схемы = 1 и шагов нет).
    fn to_v2(mut v: Value) -> Result<Value> {
        v["schema_version"] = json!(2);
        Ok(v)
    }
    const SYNTH_STEPS: &[Step] = &[Step {
        to: 2,
        summary: "test bump",
        apply: to_v2,
    }];
    fn synth() -> JsonArtifact {
        JsonArtifact {
            name: "synthetic",
            current: 2,
            detect: detect_settings,
            steps: SYNTH_STEPS,
        }
    }

    #[test]
    fn assess_classifies_up_to_date_migrate_downgrade() {
        let a = synth();
        assert_eq!(
            a.assess(&json!({"schema_version": 2})),
            Assessment::UpToDate
        );
        assert_eq!(
            a.assess(&json!({"schema_version": 1})),
            Assessment::Migrate { from: 1 }
        );
        assert_eq!(a.assess(&json!({})), Assessment::Migrate { from: 1 });
        assert_eq!(
            a.assess(&json!({"schema_version": 3})),
            Assessment::Downgrade { from: 3 }
        );
    }

    #[test]
    fn apply_steps_runs_only_needed_steps() {
        let a = synth();
        let out = a
            .apply_steps(json!({"schema_version": 1, "x": 7}), 1)
            .unwrap();
        assert_eq!(out["schema_version"], json!(2));
        assert_eq!(out["x"], json!(7), "прочие поля сохранены");
        // from == current → шаги не выполняются (значение неизменно).
        let noop = a.apply_steps(json!({"schema_version": 2}), 2).unwrap();
        assert_eq!(noop["schema_version"], json!(2));
    }

    #[test]
    fn apply_step_error_propagates() {
        fn boom(_v: Value) -> Result<Value> {
            anyhow::bail!("сломано")
        }
        const STEPS: &[Step] = &[Step {
            to: 2,
            summary: "boom",
            apply: boom,
        }];
        let a = JsonArtifact {
            name: "x",
            current: 2,
            detect: detect_settings,
            steps: STEPS,
        };
        assert!(a.apply_steps(json!({}), 1).is_err());
    }

    #[test]
    fn real_registry_is_all_current_v1() {
        for a in [settings_artifact(), profiles_artifact(), chat_artifact()] {
            assert_eq!(a.current, 1);
            assert!(a.steps.is_empty(), "{}: шагов пока нет", a.name);
        }
    }
}
