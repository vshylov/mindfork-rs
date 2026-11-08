//! The story as markdown (docs/fabula.md §4, PR 6; docs/research/
//! fabula-engine.md §13): each chapter in order, its accepted version, or
//! while it is being written its latest. What the evaluation reads, and what
//! a story's landing in the chat will carry (PR 8).

use crate::shared::storage::story::{StoryDb, StoryDbError};

/// The story's chapters as one markdown text. A chapter whose text does not
/// open with a heading gets `## Chapter n`.
pub async fn markdown(db: &StoryDb) -> Result<String, StoryDbError> {
    let mut parts = Vec::new();
    for n in 1.. {
        let versions = db.chapter_versions(n).await?;
        let Some(latest) = versions.last() else {
            break;
        };
        let mut chosen = latest;
        for v in versions.iter().rev() {
            if db.accepted(&v.id).await? {
                chosen = v;
                break;
            }
        }
        let text = chosen.text.trim();
        if text.starts_with('#') {
            parts.push(text.to_owned());
        } else {
            parts.push(format!("## Chapter {n}\n\n{text}"));
        }
    }
    let mut out = parts.join("\n\n");
    if !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::story::{VersionOrigin, fixture_canon};
    use serde_json::json;

    #[tokio::test]
    async fn each_chapter_is_its_accepted_version_or_its_latest() {
        let dir = tempfile::tempdir().unwrap();
        let db = StoryDb::open(&dir.path().join("s.db"), &dir.path().join("c.db"))
            .await
            .unwrap();
        let mut author = db.author_writer().await.unwrap();
        author.import_canon(&fixture_canon()).await.unwrap();
        assert_eq!(markdown(&db).await.unwrap(), "");
        let mut log = db.run_log_writer().await.unwrap();
        let run = log.start_run(&json!({})).await.unwrap();
        author
            .add_chapter(1, VersionOrigin::Draft, "## One\n\nFirst.")
            .await
            .unwrap();
        author
            .add_chapter(1, VersionOrigin::Revision, "## One\n\nSecond.")
            .await
            .unwrap();
        author
            .add_chapter(1, VersionOrigin::Revision, "## One\n\nThird.")
            .await
            .unwrap();
        let q = author
            .add_question(
                &run,
                "ch1/v2/checkpoint",
                &crate::entities::story::QuestionBody::Chapter {
                    chapter: 1,
                    version: 2,
                },
            )
            .await
            .unwrap();
        author
            .resolve(
                &q.id,
                &crate::entities::story::Resolution {
                    decision: crate::entities::story::Decision::Approve,
                    notes: String::new(),
                    evidence: json!(null),
                },
            )
            .await
            .unwrap();
        author
            .add_chapter(2, VersionOrigin::Draft, "  No heading.  ")
            .await
            .unwrap();
        assert_eq!(
            markdown(&db).await.unwrap(),
            "## One\n\nSecond.\n\n## Chapter 2\n\nNo heading.\n"
        );
    }
}
