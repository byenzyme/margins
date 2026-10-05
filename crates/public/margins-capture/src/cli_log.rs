//! Bounded diagnostics for the shipped native CLI.

use chrono::{Local, SecondsFormat};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 1024 * 1024;
const BACKUPS: usize = 2;

pub fn event(kind: &str, message: impl AsRef<str>) {
    let Some(path) = log_path() else {
        return;
    };
    let _ = append_at(&path, MAX_BYTES, kind, message.as_ref());
}

pub fn path_display() -> Option<String> {
    log_path().map(|path| path.to_string_lossy().into_owned())
}

pub fn error_summary(error: &anyhow::Error) -> String {
    for source in error.chain() {
        if let Some(io) = source.downcast_ref::<std::io::Error>() {
            return match io.raw_os_error() {
                Some(code) => format!("category=io io_kind={:?} os_code={code}", io.kind()),
                None => format!("category=io io_kind={:?}", io.kind()),
            };
        }
    }
    "category=application details=stderr".to_string()
}

fn log_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let root = dirs::home_dir()?.join("Library/Logs/Margins");
    #[cfg(not(target_os = "macos"))]
    let root = dirs::state_dir()?.join("margins");
    Some(root.join("cli.log"))
}

fn append_at(path: &Path, max_bytes: u64, kind: &str, message: &str) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("CLI log path has no parent"))?;
    fs::create_dir_all(parent)?;
    let line = format!(
        "{} pid={} kind={} {}\n",
        Local::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        std::process::id(),
        one_line(kind),
        one_line(message),
    );
    let current = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if current.saturating_add(line.len() as u64) > max_bytes {
        rotate(path)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    file.flush()
}

fn rotate(path: &Path) -> std::io::Result<()> {
    let oldest = backup_path(path, BACKUPS);
    match fs::remove_file(oldest) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    for index in (1..BACKUPS).rev() {
        let from = backup_path(path, index);
        let to = backup_path(path, index + 1);
        match fs::rename(from, to) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    match fs::rename(path, backup_path(path, 1)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn backup_path(path: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", path.to_string_lossy(), index))
}

fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\n' | '\r' | '\0' => ' ',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{append_at, backup_path, error_summary};

    #[test]
    fn log_is_single_line_and_rotates_to_bounded_backups() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("cli.log");
        append_at(&path, 120, "start\nforged", "first\rmessage").unwrap();
        for index in 0..10 {
            append_at(
                &path,
                120,
                "checkpoint",
                &format!("event={index} padding=xxxxxxxx"),
            )
            .unwrap();
        }

        assert!(path.exists());
        assert!(backup_path(&path, 1).exists());
        assert!(backup_path(&path, 2).exists());
        assert!(!backup_path(&path, 3).exists());
        for candidate in [&path, &backup_path(&path, 1), &backup_path(&path, 2)] {
            let text = std::fs::read_to_string(candidate).unwrap();
            assert!(text.lines().all(|line| line.contains(" pid=")));
            assert!(!text.contains("start\nforged"));
            assert!(!text.contains("first\rmessage"));
        }
    }

    #[test]
    fn error_summaries_do_not_persist_messages_paths_or_selectors() {
        let selector = "private-project-selector";
        let application = anyhow::anyhow!("unknown project {selector}");
        let application_summary = error_summary(&application);
        assert_eq!(application_summary, "category=application details=stderr");
        assert!(!application_summary.contains(selector));

        let path = "/private/meeting/memo.md";
        let io = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            path,
        ));
        let io_summary = error_summary(&io);
        assert!(io_summary.contains("category=io"));
        assert!(io_summary.contains("PermissionDenied"));
        assert!(!io_summary.contains(path));
    }
}
