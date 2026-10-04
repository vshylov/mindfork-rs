//! The showcase in Russian — the animated demo's Russian reel
//! (docs/research/demo-reel.md, stage 3). Capture-only: declared under
//! `#[cfg(test)]` in `features/demo.rs`, like the stills' own config.
//!
//! Only the words change. Every id, timestamp and status, and the shape of the
//! tool call, are the English showcase's ([`super::showcase_messages`],
//! [`super::self_model`]), so the two reels tell one story frame for frame and
//! differ in their language alone. The texts are written for a Russian reader
//! rather than translated word for word — except the flowchart's labels, which
//! are as long as the English ones to the character: the chart's legs are
//! straight only at those widths (see the English fixture), and a first
//! translation bent the middle one.

use crate::entities::message::Message;
use crate::entities::self_model::SelfModel;
use crate::shared::i18n::Lang;

// cyrillic-ok:start — the Russian showcase is fixture data (AGENTS.md §3).

/// The showcase chat's title.
pub const CHAT_TITLE: &str = "Gemma 4 на видеокарте 12 ГБ";

const FIRST_QUESTION: &str =
    "Какой квант Gemma 4 12B влезет в карту на 12 ГБ с запасом под контекст 16k?";

const FIRST_ANSWER: &str = "Три кандидата, только веса — KV-кэш сверху:\n\n\
     | Квант | Веса | Цена по качеству |\n\
     |---|---|---|\n\
     | Q4_K_M | ~7,3 ГБ | небольшая, но заметная |\n\
     | Q5_K_M | ~8,4 ГБ | пренебрежимая |\n\
     | Q6_K | ~9,7 ГБ | на уровне шума |\n\n\
     На 16k контекста KV-кэш добавит примерно 1,5 ГБ, так что **Q5_K_M** — \
     золотая середина: запас и по качеству, *и* по памяти:\n\n\
     ```bash\n\
     llama-server -m gemma-4-12B-it-Q5_K_M.gguf -ngl 99 -c 16384 --jinja\n\
     ```";

const QUESTION: &str =
    "Подведи итог — маленькая таблица и схема выбора, и сохрани заметку о моём железе.";

const ANSWER: &str = "Коротко:\n\n\
     | Квант | Контекст 16k? | Вердикт |\n\
     |---|---|---|\n\
     | Q5_K_M | влезает, ~1,5 ГБ в запасе | **золотая середина** |\n\
     | Q6_K | впритык | качество важнее |\n\n\
     ```mermaid\n\
     flowchart TD\n\
         A{Какое окно нужно?} -->|8k| B[Q6_K — качество выше]\n\
         A -->|16k| C[Q5_K_M — оптимально]\n\
         A -->|32k| D[Q4_K_M — запас на KV]\n\
     ```\n\n\
     Заметку сохранил. Правило про кэш: $M_{kv} \\propto L$ — окно вдвое \
     длиннее, кэш вдвое больше.";

const THOUGHTS: &str = "В заметке стоит записать бюджет в 12 ГБ и выбор Q5_K_M — \
     тогда следующие вопросы о размерах начнутся с железа.";

const NOTE: &str =
    "У пользователя видеокарта на 12 ГБ; в неё влезает Gemma 4 12B в Q5_K_M с контекстом 16k.";

const SUMMARY: &str = "Я работаю локально и помогаю с практической инженерией — \
     подбор моделей, Rust, иногда планы поездок. Измеренные числа предпочитаю \
     прилагательным и записываю важное: совет должен начинаться с запомненных \
     фактов, а не со свежих догадок. Сейчас в работе — уместить Gemma 4 в карту \
     на 12 ГБ, не жертвуя контекстом; следующий эксперимент в списке — черновая \
     модель 1B для спекулятивного декодирования.";

/// In the order of the English goals table (`GOALS_RAW`): the active ones,
/// then the done ones.
const GOALS: [&str; 5] = [
    "Отвечать на вопросы о размерах по сохранённым заметкам — без переспрашиваний о железе.",
    "Делать рекомендации воспроизводимыми: точный квант, контекст и флаги.",
    "Измерить ускорение от черновой 1B и записать число, а не впечатление.",
    "Проиндексировать документацию llama.cpp server в базу знаний.",
    "Превратить исследование октябрьской поездки в заметку-чеклист для сборов.",
];

const TRAITS: [&str; 3] = [
    "методичный",
    "нетерпелив к расплывчатым ответам",
    "не выносит голословности",
];

const INTERESTS: [&str; 3] = [
    "настройка локальных моделей",
    "инструменты для терминала",
    "спекулятивное декодирование",
];

const RELATIONSHIP: &str = "Работаем вместе и без церемоний; шутки заходят лучше после цифр.";

/// In the order of the English narrative table (`NARRATIVE_RAW`), which is
/// chronological.
const NARRATIVE: [&str; 6] = [
    "Чеклист для сборов вырос из трёх разрозненных чатов — именно в сведении заметки и окупаются.",
    "Здесь таблицы лучше прозы: сравнения читают, абзацы пробегают глазами.",
    "Планы поездок то и дело вклиниваются между инженерными вопросами — в терминале живут, это не тема разговора.",
    "Бюджет в 12 ГБ видеопамяти всплывает снова и снова — сохранил его заметкой, чтобы не переспрашивать.",
    "Ответы со ссылкой на заметку, из которой они взяты, вызывают меньше уточнений — расписки лучше повторов.",
    "Показать точную команду, которую я запустил, — оказалось, самый быстрый способ заслужить доверие.",
];

// cyrillic-ok:end

/// The showcase conversation in Russian: the English one with its words
/// replaced.
pub fn showcase_messages() -> Vec<Message> {
    let mut messages = super::showcase_messages();
    for (message, text) in messages
        .iter_mut()
        .zip([FIRST_QUESTION, FIRST_ANSWER, QUESTION, ANSWER])
    {
        message.text = text.into();
    }
    let answer = &mut messages[3];
    answer.thoughts = Some(THOUGHTS.into());
    let call = &mut answer.tool_calls[0];
    call.arguments = super::note_save_arguments(NOTE);
    call.result = Some(super::note_save_result(Lang::Ru));
    messages
}

/// The self-model in Russian: the English one with its words replaced.
pub fn self_model() -> SelfModel {
    let mut model = super::self_model();
    model.summary = SUMMARY.into();
    for (goal, text) in model.goals.iter_mut().zip(GOALS) {
        goal.description = text.into();
    }
    model.user_model.perceived_traits = TRAITS.map(String::from).into();
    model.user_model.current_interests = INTERESTS.map(String::from).into();
    model.user_model.relationship_dynamic = RELATIONSHIP.into();
    for (segment, text) in model.narrative.iter_mut().zip(NARRATIVE) {
        segment.text = text.into();
    }
    model
}

mod tests {
    use super::{GOALS, NARRATIVE, self_model, showcase_messages};

    /// The English showcase with other words: the same ids, times, roles and
    /// tool call shape — a `zip` that silently stopped short would leave an
    /// English text behind, so the counts are pinned too.
    #[test]
    fn the_russian_showcase_is_the_english_one_in_other_words() {
        let (en, ru) = (super::super::showcase_messages(), showcase_messages());
        assert_eq!(en.len(), 4);
        assert_eq!(ru.len(), en.len());
        for (e, r) in en.iter().zip(&ru) {
            assert_eq!((e.id, e.role, e.timestamp), (r.id, r.role, r.timestamp));
            assert_ne!(e.text, r.text);
            assert_eq!(e.thoughts.is_some(), r.thoughts.is_some());
            assert_eq!(e.tool_calls.len(), r.tool_calls.len());
        }
        let (e, r) = (&en[3].tool_calls[0], &ru[3].tool_calls[0]);
        assert_eq!((&e.id, &e.name), (&r.id, &r.name));
        assert_ne!(e.result, r.result);

        let (en, ru) = (super::super::self_model(), self_model());
        assert_eq!(en.goals.len(), GOALS.len());
        assert_eq!(en.narrative.len(), NARRATIVE.len());
        for (e, r) in en.goals.iter().zip(&ru.goals) {
            assert_eq!((e.id, e.created_at), (r.id, r.created_at));
            assert_ne!(e.description, r.description);
        }
        for (e, r) in en.narrative.iter().zip(&ru.narrative) {
            assert_eq!((e.id, e.created_at), (r.id, r.created_at));
            assert_ne!(e.text, r.text);
        }
        assert_ne!(en.summary, ru.summary);
    }
}
