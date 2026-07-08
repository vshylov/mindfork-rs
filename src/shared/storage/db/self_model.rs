//! Хранилище (SQLite) — модель себя: get/upsert/атомарный update. Часть модуля [`super`]; разбито из
//! монолита db.rs (см. docs/history/refactoring-god-objects.md, этап 5).

use super::*;

impl Db {
    // ---------- модель себя (SelfModel) ----------

    /// Модель себя профиля (`None`, если ещё не создавалась). Изоляция по PK.
    pub fn self_model_get(&self, profile_id: Uuid) -> Result<Option<SelfModel>> {
        let conn = self.conn.lock().unwrap();
        Self::self_model_get_conn(&conn, profile_id)
    }

    /// Сохраняет модель себя (INSERT OR REPLACE), повышая `version`. Поле `version`
    /// в переданной модели игнорируется — авторитетный счётчик ведёт хранилище.
    ///
    /// Для правки существующей модели предпочтителен [`Self::self_model_update`]:
    /// он делает чтение-правку-запись **атомарно** (под одним захватом мьютекса),
    /// исключая гонку «прочитал → кто-то записал → записал поверх» между фоновой
    /// авто-рефлексией, ручной правкой `F3` и инструментами хода. Прямой upsert
    /// оставлен как симметричный примитив хранилища (get/upsert/update) и
    /// используется тестами; прикладные писатели идут через `self_model_update`.
    #[allow(dead_code)]
    pub fn self_model_upsert(&self, model: &SelfModel) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::self_model_upsert_conn(&conn, model)?;
        Ok(())
    }

    /// Атомарное чтение-правка-запись модели профиля: SELECT + `mutate` + upsert
    /// под **одним** захватом мьютекса соединения. `mutate` возвращает `true`, если
    /// модель изменилась (иначе запись и рост `version` не делаются). Возвращает
    /// модель после правки (с авторитетными `version`/`updated_at`, если записана)
    /// и признак записи. Так исключается гонка load-modify-save между параллельными
    /// писателями (см. док к [`Self::self_model_upsert`]).
    ///
    /// `mutate` вызывается под захваченным мьютексом БД — внутри него **нельзя**
    /// обращаться к другим методам `Db` того же соединения (реентерабельный `lock()`
    /// нереентерабельного `Mutex` = дедлок); это чистая правка значения `SelfModel`.
    pub fn self_model_update(
        &self,
        profile_id: Uuid,
        mutate: impl FnOnce(&mut SelfModel) -> bool,
    ) -> Result<(SelfModel, bool)> {
        let conn = self.conn.lock().unwrap();
        let mut model = Self::self_model_get_conn(&conn, profile_id)?
            .unwrap_or_else(|| SelfModel::new(profile_id));
        let changed = mutate(&mut model);
        if changed {
            model = Self::self_model_upsert_conn(&conn, &model)?;
        }
        Ok((model, changed))
    }

    /// Чтение модели по соединению (без захвата мьютекса — вызывается под ним).
    fn self_model_get_conn(conn: &Connection, profile_id: Uuid) -> Result<Option<SelfModel>> {
        let data: Option<String> = conn
            .query_row(
                "SELECT data FROM self_models WHERE profile_id = ?1",
                params![profile_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        match data {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    /// Запись модели по соединению (без захвата мьютекса — вызывается под ним).
    /// Возвращает фактически сохранённую модель (с повышенным `version` и свежим
    /// `updated_at`), чтобы вызывающий не перечитывал БД.
    fn self_model_upsert_conn(conn: &Connection, model: &SelfModel) -> Result<SelfModel> {
        // rusqlite не поддерживает u64 в ToSql/FromSql — версию храним как i64.
        let prev: Option<i64> = conn
            .query_row(
                "SELECT version FROM self_models WHERE profile_id = ?1",
                params![model.profile_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        let mut stored = model.clone();
        stored.version = prev.unwrap_or(0).max(0) as u64 + 1;
        stored.updated_at = Utc::now();
        conn.execute(
            "INSERT INTO self_models(profile_id, data, version, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(profile_id) DO UPDATE SET
                 data = excluded.data,
                 version = excluded.version,
                 updated_at = excluded.updated_at",
            params![
                stored.profile_id.to_string(),
                serde_json::to_string(&stored)?,
                stored.version as i64,
                stored.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    #[test]
    fn self_model_round_trip_and_isolation() {
        use crate::entities::self_model::SelfModel;
        let db = db();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();

        // Изначально нет.
        assert!(db.self_model_get(a).unwrap().is_none());

        let mut m = SelfModel::new(a);
        m.summary = "о себе A".into();
        m.add_goal("цель A");
        db.self_model_upsert(&m).unwrap();

        let loaded = db.self_model_get(a).unwrap().unwrap();
        assert_eq!(loaded.summary, "о себе A");
        assert_eq!(loaded.goals.len(), 1);
        assert_eq!(loaded.version, 1); // версия выставлена хранилищем

        // Профиль B не видит модель A.
        assert!(db.self_model_get(b).unwrap().is_none());

        // Повторный upsert повышает версию и заменяет данные.
        let mut m2 = loaded.clone();
        m2.summary = "обновлено".into();
        db.self_model_upsert(&m2).unwrap();
        let reloaded = db.self_model_get(a).unwrap().unwrap();
        assert_eq!(reloaded.summary, "обновлено");
        assert_eq!(reloaded.version, 2);
    }

    #[test]
    fn self_model_update_is_atomic_under_concurrency() {
        use std::sync::Arc;
        // Два потока параллельно дописывают инсайты в модель одного профиля. При
        // неатомарном read-modify-write часть записей терялась бы (гонка «прочитал →
        // другой записал → записал поверх»). `self_model_update` держит SELECT+upsert
        // под одним захватом мьютекса — ни одна запись не теряется.
        let db = Arc::new(db());
        let pid = Uuid::new_v4();
        let n: usize = 50;
        let handles: Vec<_> = ["A", "B"]
            .iter()
            .map(|prefix| {
                let db = db.clone();
                let prefix = prefix.to_string();
                std::thread::spawn(move || {
                    for i in 0..n {
                        db.self_model_update(pid, |m| {
                            // Накапливаем цели — считаем ровно (add_goal без потолка).
                            m.add_goal(format!("{prefix}{i}"));
                            true
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let m = db.self_model_get(pid).unwrap().unwrap();
        assert_eq!(m.goals.len(), 2 * n);
        assert_eq!(m.version, (2 * n) as u64); // каждая правка = один upsert
    }

    #[test]
    fn self_model_update_skips_write_when_unchanged() {
        let db = db();
        let pid = Uuid::new_v4();
        // mutate вернул false → записи и роста версии нет, строки в БД не появилось.
        let (model, changed) = db.self_model_update(pid, |_m| false).unwrap();
        assert!(!changed);
        assert_eq!(model.version, 0);
        assert!(db.self_model_get(pid).unwrap().is_none());
    }
}
