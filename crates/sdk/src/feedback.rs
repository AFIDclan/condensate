//! Saving a feedback recording to disk, for reading or for handing to an LLM.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::Client;

/// Save recording `id` into a new folder under `parent`:
///
/// ```text
/// <parent>/<date>-<id>/
///   transcript.md   the timeline: who said what, with screenshots in place
///   feedback.json   the same as data
///   shots/*.jpg     the screenshots transcript.md links to
/// ```
///
/// Saving again refreshes the transcript (it may have been transcribed since) and fetches only
/// missing screenshots. Returns the folder.
pub async fn export(client: &Client, id: &str, parent: &Path) -> Result<PathBuf> {
    let detail = client.feedback(id).await?;
    let (y, mo, d, h, mi) = condensate_core::utc_parts(detail.summary.started_at);
    let dir = parent.join(format!("{y}-{mo:02}-{d:02}-{h:02}{mi:02}-{id}"));
    let shots = dir.join("shots");
    std::fs::create_dir_all(&shots).with_context(|| format!("couldn't create {}", shots.display()))?;
    for shot in &detail.shots {
        let path = shots.join(&shot.file);
        if !path.exists() {
            let bytes = client.feedback_file(id, &shot.file).await?;
            std::fs::write(&path, &bytes)?;
        }
    }
    std::fs::write(dir.join("transcript.md"), client.feedback_transcript(id).await?)?;
    std::fs::write(dir.join("feedback.json"), serde_json::to_vec_pretty(&detail)?)?;
    Ok(dir)
}
