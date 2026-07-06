//! Хранилище (SQLite) — модель себя: get/upsert/атомарный update. Часть модуля [`super`]; разбито из
//! монолита db.rs (см. docs/refactoring-god-objects.md, этап 5).

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
