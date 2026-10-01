use std::path::{Path, PathBuf};

pub(crate) use margins::session_index::SessionInfoDto;

use crate::Settings;

pub(crate) fn list_sessions_with_notes(
    work_dir: &Path,
    settings: &Settings,
    project_id: Option<&str>,
    recording_name: Option<&str>,
) -> Result<Vec<SessionInfoDto>, String> {
    margins::session_index::list_sessions_with_notes(
        &session_index_work_dir(work_dir, settings, project_id),
        project_id,
        recording_name,
    )
}

pub(crate) fn recent_people_candidates(
    work_dir: &Path,
    settings: &Settings,
    project_id: Option<&str>,
) -> Vec<String> {
    margins::session_index::recent_people_candidates(
        &session_index_work_dir(work_dir, settings, project_id),
        settings.people_folder.as_str(),
        project_id,
    )
}

pub(crate) fn note_path_for_session_or_capture(
    margins_dir: &Path,
    name: &str,
    _settings: &Settings,
) -> Result<PathBuf, String> {
    margins::session_index::note_path_for_session_or_capture(margins_dir, name)
}

fn session_index_work_dir(
    work_dir: &Path,
    settings: &Settings,
    project_id: Option<&str>,
) -> PathBuf {
    let _ = (settings, project_id);
    work_dir.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_index_uses_project_root_not_inbox_folder() {
        let settings = Settings {
            vault_path: Some("/vault".to_string()),
            inbox_folder: "inbox".to_string(),
            projects: vec![crate::settings::ProjectSource {
                id: "vault".to_string(),
                name: "Vault".to_string(),
                path: "/vault".to_string(),
                inbox_folder: "inbox".to_string(),
                people_folder: "people".to_string(),
                readiness: "ready".to_string(),
            }],
            active_project_id: Some("vault".to_string()),
            ..Default::default()
        };

        assert_eq!(
            session_index_work_dir(Path::new("/vault"), &settings, Some("vault")),
            PathBuf::from("/vault")
        );
    }
}
