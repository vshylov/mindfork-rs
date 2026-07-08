//! Инструменты базы знаний (RAG): `rag_add`, `rag_search`. Чанкинг → эмбеддинг
//! (выделенный сервер, ADR 0002) → запись/kNN в sqlite-vec, **изоляция по
//! `profile_id`** (spec §9.3, §9.5).

use anyhow::Result;

use crate::entities::profile::ToolId;
use crate::entities::rag::{RagDocument, RagHit};

use crate::shared::config::{
    DEFAULT_CHUNK_MAX_CHARS, DEFAULT_CHUNK_OVERLAP_CHARS, DEFAULT_CHUNK_TARGET_CHARS, RagSettings,
};

use super::{Tool, ToolContext, ToolOutcome};

/// Минимальная длина дословного совпадения для склейки соседних чанков при
/// извлечении (короче — вероятна случайность, а не заложенное перекрытие).
const MIN_STITCH_OVERLAP: usize = 24;
/// Топ-K по умолчанию для поиска.
const DEFAULT_TOP_K: usize = 5;

/// Параметры чанкинга (размеры в символах). Конфигурируемы через настройки
/// (`config.rag`, см. spec §9.3): передаются в [`chunk_text`]/[`chunk_markdown`]
/// вместо ранее захардкоженных констант. `Default` совпадает с прежними значениями.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkParams {
    /// Целевой («мягкий») размер чанка — юниты пакуются до него.
    pub target: usize,
    /// Перекрытие соседних чанков: хвост предыдущего повторяется в начале следующего.
    /// Best practice RAG — запрос у границы чанка не теряет контекст (а при извлечении
    /// дубль снимается склейкой, см. [`stitch_hits`]).
    pub overlap: usize,
    /// Жёсткий потолок неделимого прогона (очень длинное слово/строка без пунктуации).
    pub max: usize,
}

impl Default for ChunkParams {
    fn default() -> Self {
        Self {
            target: DEFAULT_CHUNK_TARGET_CHARS,
            overlap: DEFAULT_CHUNK_OVERLAP_CHARS,
            max: DEFAULT_CHUNK_MAX_CHARS,
        }
    }
}

impl ChunkParams {
    /// Параметры из настроек RAG (`config.rag`). Невалидные значения (нулевой
    /// целевой размер) подменяются дефолтом, чтобы чанкер не зациклился/не отдал пусто.
    pub fn from_settings(rag: &RagSettings) -> Self {
        let target = if rag.chunk_target_chars == 0 {
            DEFAULT_CHUNK_TARGET_CHARS
        } else {
            rag.chunk_target_chars
        };
        // Потолок не может быть меньше цели — иначе целевая упаковка невозможна.
        let max = rag.chunk_max_chars.max(target);
        Self {
            target,
            overlap: rag.chunk_overlap_chars.min(target.saturating_sub(1)),
            max,
        }
    }
}

/// `rag_add` — добавляет текст в базу знаний (чанкинг + эмбеддинг). Возвращает
/// число записанных чанков.
pub struct RagAdd;

#[async_trait::async_trait]
impl Tool for RagAdd {
    fn id(&self) -> ToolId {
        "rag_add".into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "добавить в базу знаний"
    }
    fn description(&self) -> String {
        "Добавить текст в базу знаний для последующего семантического поиска.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {"type": "string"},
                "source": {"type": "string", "description": "Источник (имя/URL)"}
            },
            "required": ["text"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("ожидается строковое поле text"))?;
        let source = args
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("(без источника)")
            .to_string();

        let chunks = chunk_text(text, ctx.chunk_params);
        if chunks.is_empty() {
            anyhow::bail!("text не содержит контента для индексации");
        }
        let embeddings = ctx.embedder.embed(chunks.clone()).await?;
        if embeddings.len() != chunks.len() {
            anyhow::bail!("эмбеддер вернул неверное число векторов");
        }
        for (chunk, embedding) in chunks.iter().zip(embeddings) {
            let doc = RagDocument::new(ctx.profile_id, &source, chunk, embedding);
            ctx.storage.db().rag_insert(&doc)?;
        }
        // Сохраняем исходный текст для возможной реиндексации (`/rag rebuild`).
        // Инструмент накапливает чанки источника — поэтому дописываем, не заменяем.
        ctx.storage
            .db()
            .rag_source_append(ctx.profile_id, &source, text, chrono::Utc::now())?;
        Ok(ToolOutcome::text(format!(
            "Добавлено чанков: {}.",
            chunks.len()
        )))
    }
}

/// `rag_search` — семантический поиск по базе знаний профиля.
pub struct RagSearch;

#[async_trait::async_trait]
impl Tool for RagSearch {
    fn id(&self) -> ToolId {
        "rag_search".into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "поиск в базе знаний"
    }
    fn description(&self) -> String {
        "Найти релевантные фрагменты в базе знаний по смысловому запросу.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "top_k": {"type": "integer", "minimum": 1}
            },
            "required": ["query"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("ожидается непустое поле query"))?;
        let k = args
            .get("top_k")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_TOP_K);

        let mut embeddings = ctx.embedder.embed(vec![query.to_string()]).await?;
        let query_vec = embeddings
            .pop()
            .ok_or_else(|| anyhow::anyhow!("эмбеддер не вернул вектор запроса"))?;
        let hits = ctx.storage.db().rag_search(ctx.profile_id, &query_vec, k)?;
        if hits.is_empty() {
            return Ok(ToolOutcome::text("В базе знаний ничего не найдено."));
        }
        // Склеиваем соседние чанки одного источника (по заложенному перекрытию):
        // экономит контекст и не путает модель повтором (см. [`stitch_hits`]).
        let passages = stitch_hits(hits);
        let mut out = format!("Найдено фрагментов: {}\n", passages.len());
        for p in &passages {
            out.push_str(&format!("- [{}] {}\n", p.source, p.text));
        }
        // Обратное направление (Ярус 3, Путь 3): заметки/наблюдения, ссылающиеся на
        // найденные источники — «поиск через оба органа». self-наблюдения помечаем
        // [о себе] (органы различимы). Дедуп заметок по id.
        let mut seen: std::collections::HashSet<uuid::Uuid> = std::collections::HashSet::new();
        let mut linked: Vec<String> = Vec::new();
        for p in &passages {
            let notes = ctx
                .storage
                .db()
                .notes_citing_source(ctx.profile_id, &p.source)
                .unwrap_or_default();
            for n in notes {
                if !seen.insert(n.id) {
                    continue;
                }
                let mark = if super::notes::is_self_note(&n) {
                    "[о себе] "
                } else {
                    ""
                };
                linked.push(format!("- {mark}(id={}) {}", n.id, n.content));
            }
        }
        if !linked.is_empty() {
            out.push_str("\nЗаметки со ссылкой на эти источники:\n");
            out.push_str(&linked.join("\n"));
            out.push('\n');
        }
        Ok(ToolOutcome::text(out.trim_end().to_string()))
    }
}

/// Длина строки в символах (а не байтах — корректно для кириллицы/Юникода).
fn clen(s: &str) -> usize {
    s.chars().count()
}

/// Нарезает произвольный текст на чанки с перекрытием по границам предложений/слов
/// (best practice RAG). `pub(crate)` — переиспользуется фоновой индексацией файлов
/// (`/rag add`) и инструментом `rag_add`. Для markdown есть [`chunk_markdown`].
///
/// Алгоритм: текст сегментируется на атомарные юниты (абзац целиком, если влезает
/// в цель; иначе — предложения; слишком длинные предложения — окна по словам, а
/// одиночное гигантское слово — по символам), затем юниты пакуются в чанки до
/// `CHUNK_TARGET_CHARS`, и каждый следующий чанк начинается с хвоста предыдущего
/// (перекрытие ≤ `CHUNK_OVERLAP_CHARS`). Мелкие соседние абзацы при этом
/// группируются в один чанк (а не плодят крошечные строки-чанки).
pub(crate) fn chunk_text(text: &str, params: ChunkParams) -> Vec<String> {
    let units = segment_units(text, params);
    pack_units(&units, params.target, params.overlap)
}

/// Семантический чанкинг markdown: режет по ATX-заголовкам (`#`..`######`),
/// защищает огороженные блоки кода (``` и ~~~), а к каждому чанку секции
/// добавляет её заголовок как смысловой якорь (заметно улучшает извлечение).
/// Внутри секции — тот же упаковщик с перекрытием, что и в [`chunk_text`].
/// Документ без заголовков обрабатывается как обычный текст.
pub(crate) fn chunk_markdown(text: &str, params: ChunkParams) -> Vec<String> {
    let sections = split_sections(text);
    let mut chunks = Vec::new();
    for (heading, body) in &sections {
        let units = segment_units(body, params);
        let packed = if units.is_empty() {
            // Секция без тела — заголовок сам по себе как чанк (если он есть).
            vec![String::new()]
        } else {
            pack_units(&units, params.target, params.overlap)
        };
        for p in packed {
            let chunk = match (heading.is_empty(), p.is_empty()) {
                (true, _) => p,
                (false, true) => heading.clone(),
                (false, false) => format!("{heading}\n{p}"),
            };
            let chunk = chunk.trim();
            if !chunk.is_empty() {
                chunks.push(chunk.to_string());
            }
        }
    }
    if chunks.is_empty() {
        // Нет заголовков/пустой документ — обычный чанкинг.
        return chunk_text(text, params);
    }
    chunks
}

/// Атомарные юниты для упаковки (см. [`chunk_text`]). Пустые отбрасываются.
fn segment_units(text: &str, params: ChunkParams) -> Vec<String> {
    let mut units = Vec::new();
    for paragraph in text.split("\n\n") {
        let p = paragraph.trim();
        if p.is_empty() {
            continue;
        }
        if clen(p) <= params.target {
            units.push(p.to_string());
            continue;
        }
        for sentence in split_sentences(p) {
            if clen(&sentence) <= params.max {
                units.push(sentence);
            } else {
                units.extend(break_long(&sentence, params.max));
            }
        }
    }
    units
}

/// Делит абзац на предложения по завершающей пунктуации (`. ! ? …` и их
/// CJK-аналоги), сохраняя её. Граница — пунктуация, за которой пробел/конец.
fn split_sentences(paragraph: &str) -> Vec<String> {
    let chars: Vec<char> = paragraph.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        let is_end = matches!(chars[i], '.' | '!' | '?' | '…' | '。' | '！' | '？');
        let next_ws = chars.get(i + 1).map(|c| c.is_whitespace()).unwrap_or(true);
        if is_end && next_ws {
            let seg: String = chars[start..=i].iter().collect();
            let seg = seg.trim();
            if !seg.is_empty() {
                out.push(seg.to_string());
            }
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            start = j;
            i = j;
            continue;
        }
        i += 1;
    }
    if start < chars.len() {
        let seg: String = chars[start..].iter().collect();
        let seg = seg.trim();
        if !seg.is_empty() {
            out.push(seg.to_string());
        }
    }
    out
}

/// Дробит слишком длинную строку (без завершающей пунктуации) на окна по словам;
/// одиночное слово длиннее потолка рвётся по символам (последнее средство).
fn break_long(s: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;
    for word in s.split_whitespace() {
        let wlen = clen(word);
        if wlen > max {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            let chars: Vec<char> = word.chars().collect();
            for w in chars.chunks(max) {
                out.push(w.iter().collect());
            }
            continue;
        }
        let add = if cur.is_empty() { wlen } else { wlen + 1 };
        if cur_len + add > max && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur.push_str(word);
            cur_len = wlen;
        } else {
            if !cur.is_empty() {
                cur.push(' ');
                cur_len += 1;
            }
            cur.push_str(word);
            cur_len += wlen;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Упаковывает юниты в чанки до целевого размера, начиная каждый следующий с
/// хвоста предыдущего (перекрытие ≤ `overlap` символов, по границам юнитов).
fn pack_units(units: &[String], target: usize, overlap: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut cur_len = 0usize;
    for unit in units {
        let ulen = clen(unit);
        let add = if cur.is_empty() { ulen } else { ulen + 1 };
        if !cur.is_empty() && cur_len + add > target {
            chunks.push(cur.join("\n"));
            // Хвост для перекрытия: последние юниты в пределах `overlap` символов
            // (минимум один — иначе цикл не двигался бы).
            let mut tail: Vec<&str> = Vec::new();
            let mut tlen = 0usize;
            for &u in cur.iter().rev() {
                let a = if tail.is_empty() {
                    clen(u)
                } else {
                    clen(u) + 1
                };
                if tlen + a > overlap && !tail.is_empty() {
                    break;
                }
                tail.push(u);
                tlen += a;
            }
            tail.reverse();
            cur = tail;
            cur_len = tlen;
        }
        if !cur.is_empty() {
            cur_len += 1;
        }
        cur.push(unit);
        cur_len += ulen;
    }
    if !cur.is_empty() {
        chunks.push(cur.join("\n"));
    }
    chunks
}

/// Разбивает markdown на секции `(заголовок, тело)` по ATX-заголовкам, не трогая
/// `#` внутри огороженных блоков кода. Преамбула до первого заголовка → `("", …)`.
fn split_sections(text: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut heading = String::new();
    let mut body = String::new();
    let mut fence: Option<&str> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(f) = fence {
            if trimmed.starts_with(f) {
                fence = None;
            }
            body.push_str(line);
            body.push('\n');
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(if trimmed.starts_with("```") {
                "```"
            } else {
                "~~~"
            });
            body.push_str(line);
            body.push('\n');
            continue;
        }
        if is_atx_heading(trimmed) {
            if !heading.is_empty() || !body.trim().is_empty() {
                sections.push((std::mem::take(&mut heading), std::mem::take(&mut body)));
            }
            heading = trimmed.trim_end().to_string();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    if !heading.is_empty() || !body.trim().is_empty() {
        sections.push((heading, body));
    }
    sections
}

/// Это строка ATX-заголовка markdown (`#`..`######` + пробел)?
fn is_atx_heading(line: &str) -> bool {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    (1..=6).contains(&hashes) && line.chars().nth(hashes) == Some(' ')
}

/// Связный фрагмент извлечения — один или несколько склеенных по перекрытию
/// чанков одного источника (см. [`stitch_hits`]).
pub(crate) struct StitchedPassage {
    pub source: String,
    pub text: String,
    pub distance: f32,
}

/// Склеивает соседние чанки одного источника, если конец одного дословно
/// совпадает с началом другого (перекрытие, заложенное при чанкинге): объединяет
/// в один связный фрагмент без дубля — экономит контекст и не путает модель
/// повтором. Фрагменты упорядочены по лучшему (минимальному) расстоянию.
pub(crate) fn stitch_hits(hits: Vec<RagHit>) -> Vec<StitchedPassage> {
    // Группируем по источнику, сохраняя порядок первого появления.
    let mut by_source: Vec<(String, Vec<(String, f32)>)> = Vec::new();
    for h in hits {
        match by_source.iter_mut().find(|(s, _)| *s == h.source) {
            Some(g) => g.1.push((h.chunk_text, h.distance)),
            None => by_source.push((h.source, vec![(h.chunk_text, h.distance)])),
        }
    }
    let mut passages = Vec::new();
    for (source, mut items) in by_source {
        // Итеративно склеиваем любые две части с реальным перекрытием.
        while let Some((i, j, text, dist)) = find_mergeable(&items) {
            let (hi, lo) = (i.max(j), i.min(j));
            items.remove(hi);
            items.remove(lo);
            items.push((text, dist));
        }
        for (text, distance) in items {
            passages.push(StitchedPassage {
                source: source.clone(),
                text,
                distance,
            });
        }
    }
    passages.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    passages
}

/// Ищет первую пару частей `(i, j)`, которые можно склеить (конец `i` совпадает с
/// началом `j`); возвращает индексы, объединённый текст и лучшее расстояние.
fn find_mergeable(items: &[(String, f32)]) -> Option<(usize, usize, String, f32)> {
    for i in 0..items.len() {
        for j in 0..items.len() {
            if i == j {
                continue;
            }
            if let Some(text) = merge_overlap(&items[i].0, &items[j].0) {
                return Some((i, j, text, items[i].1.min(items[j].1)));
            }
        }
    }
    None
}

/// Если конец `a` дословно совпадает с началом `b` (≥ `MIN_STITCH_OVERLAP` симв.),
/// возвращает `a` + хвост `b` без повтора. Учитывает повторный markdown-заголовок
/// в начале `b` (тот же, что у `a`): снимает его перед сопоставлением и не дублирует.
fn merge_overlap(a: &str, b: &str) -> Option<String> {
    if let Some(k) = overlap_len(a, b) {
        let tail: String = b.chars().skip(k).collect();
        return Some(format!("{a}{tail}"));
    }
    // `b` начинается с того же заголовка, что и `a` — сопоставляем тело.
    if let (Some(ha), Some((hb, rest_b))) = (leading_heading(a), strip_leading_heading(b))
        && ha == hb
        && let Some(k) = overlap_len(a, &rest_b)
    {
        let tail: String = rest_b.chars().skip(k).collect();
        return Some(format!("{a}{tail}"));
    }
    None
}

/// Длина наибольшего суффикса `a`, равного префиксу `b` (≥ `MIN_STITCH_OVERLAP`).
fn overlap_len(a: &str, b: &str) -> Option<usize> {
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    let max = ac.len().min(bc.len());
    let mut k = max;
    while k >= MIN_STITCH_OVERLAP {
        if ac[ac.len() - k..] == bc[..k] {
            return Some(k);
        }
        k -= 1;
    }
    None
}

/// Ведущая строка-заголовок ATX (если первая строка — заголовок).
fn leading_heading(s: &str) -> Option<String> {
    let first = s.lines().next()?;
    is_atx_heading(first.trim_start()).then(|| first.trim_end().to_string())
}

/// Снимает ведущий ATX-заголовок: возвращает `(заголовок, остаток)`.
fn strip_leading_heading(s: &str) -> Option<(String, String)> {
    let mut lines = s.splitn(2, '\n');
    let first = lines.next()?;
    if is_atx_heading(first.trim_start()) {
        Some((
            first.trim_end().to_string(),
            lines.next().unwrap_or("").to_string(),
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use uuid::Uuid;

    #[test]
    fn chunking_groups_small_paragraphs() {
        // Мелкие соседние абзацы группируются в один чанк (а не плодят крошечные).
        let chunks = chunk_text(
            "первый абзац\n\nвторой абзац\n\nтретий абзац",
            ChunkParams::default(),
        );
        assert_eq!(chunks.len(), 1, "{chunks:?}");
        assert!(chunks[0].contains("первый абзац"));
        assert!(chunks[0].contains("третий абзац"));
    }

    #[test]
    fn chunking_splits_long_paragraph_with_overlap() {
        // Длинный абзац из предложений режется на несколько чанков с перекрытием.
        let sentence = "Это предложение средней длины для проверки чанкинга. ";
        let text = sentence.repeat(60); // ~3000 символов
        let params = ChunkParams::default();
        let chunks = chunk_text(&text, params);
        assert!(
            chunks.len() >= 2,
            "ожидаем несколько чанков: {}",
            chunks.len()
        );
        // Каждый чанк в разумных пределах (потолок + перекрытие).
        for c in &chunks {
            assert!(clen(c) <= params.max + params.overlap, "{}", clen(c));
        }
        // Перекрытие: конец первого чанка дословно встречается в начале второго.
        assert!(
            overlap_len(&chunks[0], &chunks[1]).is_some(),
            "ожидаем перекрытие между соседними чанками"
        );
    }

    #[test]
    fn chunking_never_breaks_mid_word() {
        // Очень длинное «слово» (без пробелов) рвётся, но обычные слова — целиком.
        let text = format!("короткое начало {} конец", "ё".repeat(2500));
        let chunks = chunk_text(&text, ChunkParams::default());
        assert!(chunks.iter().any(|c| c.contains("короткое начало")));
        assert!(chunks.iter().any(|c| c.contains("конец")));
    }

    #[test]
    fn chunk_params_from_settings_respects_config() {
        // Меньший целевой размер режет тот же текст на больше чанков.
        let text = "Это предложение средней длины для проверки чанкинга. ".repeat(20);
        let big = chunk_text(&text, ChunkParams::default());
        let small = chunk_text(
            &text,
            ChunkParams::from_settings(&RagSettings {
                chunk_target_chars: 200,
                chunk_overlap_chars: 40,
                chunk_max_chars: 400,
            }),
        );
        assert!(
            small.len() > big.len(),
            "меньший target → больше чанков: small={} big={}",
            small.len(),
            big.len()
        );
    }

    #[test]
    fn chunk_params_from_settings_sanitizes_invalid() {
        // Нулевой target подменяется дефолтом; перекрытие не превышает target.
        let p = ChunkParams::from_settings(&RagSettings {
            chunk_target_chars: 0,
            chunk_overlap_chars: 9999,
            chunk_max_chars: 10,
        });
        assert_eq!(p.target, DEFAULT_CHUNK_TARGET_CHARS);
        assert!(p.overlap < p.target);
        assert!(p.max >= p.target, "потолок не меньше цели");
    }

    #[test]
    fn markdown_chunks_carry_their_heading() {
        let md = "# Заголовок\n\nтекст раздела один\n\n## Подраздел\n\nтекст подраздела";
        let chunks = chunk_markdown(md, ChunkParams::default());
        assert!(chunks.iter().any(|c| c.starts_with("# Заголовок")));
        assert!(chunks.iter().any(|c| c.starts_with("## Подраздел")));
        // Каждый чанк начинается со своего заголовка (смысловой якорь).
        assert!(chunks.iter().all(|c| c.starts_with('#')));
    }

    #[test]
    fn markdown_ignores_hash_inside_code_fence() {
        let md = "# Реальный заголовок\n\n```python\n# это комментарий, не заголовок\nx = 1\n```";
        let sections = split_sections(md);
        // Один заголовок-секция (комментарий в коде не стал заголовком).
        assert_eq!(sections.len(), 1, "{sections:?}");
        assert_eq!(sections[0].0, "# Реальный заголовок");
        assert!(sections[0].1.contains("# это комментарий"));
    }

    #[test]
    fn stitch_merges_overlapping_neighbors() {
        let mk = |text: &str, d: f32| RagHit {
            id: Uuid::new_v4(),
            source: "doc.md".into(),
            chunk_text: text.into(),
            distance: d,
        };
        // Конец A дословно совпадает с началом B (≥ MIN_STITCH_OVERLAP символов).
        let a = "альфа бета гамма дельта эпсилон дзета";
        let b = "гамма дельта эпсилон дзета эта тета йота";
        let merged = stitch_hits(vec![mk(a, 0.2), mk(b, 0.3)]);
        assert_eq!(merged.len(), 1, "должны склеиться в один фрагмент");
        assert_eq!(
            merged[0].text,
            "альфа бета гамма дельта эпсилон дзета эта тета йота"
        );
        assert_eq!(merged[0].distance, 0.2, "берётся лучшее расстояние");
    }

    #[test]
    fn stitch_keeps_unrelated_hits_separate() {
        let mk = |src: &str, text: &str| RagHit {
            id: Uuid::new_v4(),
            source: src.into(),
            chunk_text: text.into(),
            distance: 0.5,
        };
        let out = stitch_hits(vec![
            mk("a.txt", "совершенно разный текст один"),
            mk("b.txt", "никак не связанный текст два"),
        ]);
        assert_eq!(out.len(), 2, "разные источники не склеиваются");
    }

    #[tokio::test]
    async fn add_then_search_returns_relevant_chunk() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        RagAdd
            .invoke(
                &ctx,
                serde_json::json!({
                    "text": "кошки любят рыбу\n\nсобаки любят кости",
                    "source": "факты"
                }),
            )
            .await
            .unwrap();

        let out = RagSearch
            .invoke(&ctx, serde_json::json!({"query": "кошки рыба", "top_k": 1}))
            .await
            .unwrap();
        assert!(
            out.result.contains("кошки любят рыбу"),
            "got: {}",
            out.result
        );
        assert!(out.result.contains("факты"));
    }

    #[tokio::test]
    async fn search_surfaces_notes_citing_matched_source() {
        // Ярус 3, Путь 3 (обратное направление): rag_search показывает заметки,
        // ссылающиеся на найденный источник — «поиск через оба органа».
        use crate::entities::note::Note;
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        RagAdd
            .invoke(
                &ctx,
                serde_json::json!({"text": "кошки любят рыбу", "source": "факты"}),
            )
            .await
            .unwrap();
        let note = Note::new(profile, "мой вывод о кошках", vec![]);
        let nid = note.id;
        storage.db().note_insert(&note).unwrap();
        storage
            .db()
            .note_cite_source_insert(profile, nid, "факты")
            .unwrap();

        let out = RagSearch
            .invoke(&ctx, serde_json::json!({"query": "кошки рыба", "top_k": 1}))
            .await
            .unwrap();
        assert!(out.result.contains("Заметки со ссылкой на эти источники"));
        assert!(out.result.contains("мой вывод о кошках"));
    }

    #[tokio::test]
    async fn search_isolated_by_profile() {
        // Документы профиля A не должны находиться при поиске профиля B
        // (разные профили в одном хранилище).
        let dir = tempfile::tempdir().unwrap();
        let storage = std::sync::Arc::new(
            crate::shared::storage::Storage::open(crate::shared::paths::Paths::with_root(
                dir.path(),
            ))
            .unwrap(),
        );
        let engine: std::sync::Arc<dyn crate::shared::api::EngineBackend> =
            std::sync::Arc::new(crate::shared::api::mock::MockBackend::scripted(vec![]));
        let embedder: std::sync::Arc<dyn crate::shared::api::Embedder> =
            std::sync::Arc::new(crate::shared::api::mock::MockEmbedder::new(16));

        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        // Общий пучок зависимостей: оба профиля делят одно хранилище (проверка
        // изоляции по profile_id).
        let deps = crate::features::tools::ToolDeps {
            storage: storage.clone(),
            engine: engine.clone(),
            embedder: embedder.clone(),
        };
        let mk = |pid| super::super::testkit::ctx_with_deps(pid, deps.clone());

        RagAdd
            .invoke(&mk(a), serde_json::json!({"text": "секрет профиля A"}))
            .await
            .unwrap();
        let out = RagSearch
            .invoke(&mk(b), serde_json::json!({"query": "секрет профиля A"}))
            .await
            .unwrap();
        assert!(out.result.contains("ничего не найдено"));
    }
}
