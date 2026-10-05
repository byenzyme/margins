// ---------------------------------------------------------------------------
// auth.rs — bearer-token auth for WP2
//
// On first startup the token is generated with rand and written to
// `<data_dir>/token` with 0600 permissions.  Subsequent startups read the
// existing token so it survives restarts.
// ---------------------------------------------------------------------------

use anyhow::Context;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

fn secure_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
}

#[cfg(unix)]
fn repair_private_permissions(path: &Path) -> anyhow::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).with_context(|| {
        format!(
            "failed to secure token file permissions: {}",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn repair_private_permissions(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

fn sync_parent_directory(data_dir: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    File::open(data_dir)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("failed to sync token directory: {}", data_dir.display()))?;
    Ok(())
}

fn create_secure_temp(data_dir: &Path) -> anyhow::Result<(PathBuf, File)> {
    for _ in 0..16 {
        let path = data_dir.join(format!(
            ".token.tmp-{}-{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to create secure token file in {}",
                        data_dir.display()
                    )
                });
            }
        }
    }
    anyhow::bail!(
        "failed to allocate a unique secure token file in {}",
        data_dir.display()
    )
}

fn replace_file_with(
    temp_path: &Path,
    token_path: &Path,
    replace: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> anyhow::Result<()> {
    replace(temp_path, token_path)
        .with_context(|| format!("failed to install token file: {}", token_path.display()))
}

#[cfg(unix)]
fn replace_file_atomically(temp_path: &Path, token_path: &Path) -> anyhow::Result<()> {
    replace_file_with(temp_path, token_path, |from, to| std::fs::rename(from, to))
}

#[cfg(windows)]
fn replace_file_atomically(temp_path: &Path, token_path: &Path) -> anyhow::Result<()> {
    replace_file_with(temp_path, token_path, |from, to| {
        super::windows_atomic_replace::replace_file(from, to)
    })
}

#[cfg(not(any(unix, windows)))]
fn replace_file_atomically(_temp_path: &Path, token_path: &Path) -> anyhow::Result<()> {
    anyhow::bail!(
        "atomic token replacement is unsupported on this platform: {}",
        token_path.display()
    )
}

fn persist_token_atomically(data_dir: &Path, token_path: &Path, token: &str) -> anyhow::Result<()> {
    let (temp_path, mut temp_file) = create_secure_temp(data_dir)?;
    let result = (|| -> anyhow::Result<()> {
        repair_private_permissions(&temp_path)?;
        temp_file
            .write_all(token.as_bytes())
            .with_context(|| format!("failed to write token file: {}", temp_path.display()))?;
        temp_file
            .sync_all()
            .with_context(|| format!("failed to sync token file: {}", temp_path.display()))?;
        drop(temp_file);

        replace_file_atomically(&temp_path, token_path)?;
        repair_private_permissions(token_path)?;
        File::open(token_path)
            .and_then(|file| file.sync_all())
            .with_context(|| format!("failed to sync token file: {}", token_path.display()))?;
        sync_parent_directory(data_dir)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

/// Read the existing token from `<data_dir>/token` or generate a new 32-byte
/// hex token, persist it, and return it.  Prints the token path to stdout.
pub fn load_or_create_token(data_dir: &Path) -> anyhow::Result<String> {
    std::fs::create_dir_all(data_dir)?;
    let token_path = data_dir.join("token");
    let lock_path = data_dir.join(".token.lock");
    let lock_file = secure_open_options()
        .open(&lock_path)
        .with_context(|| format!("failed to open token lock: {}", lock_path.display()))?;
    repair_private_permissions(&lock_path)?;
    lock_file
        .lock()
        .with_context(|| format!("failed to lock token file: {}", token_path.display()))?;

    if token_path.exists() {
        let token = std::fs::read_to_string(&token_path)?;
        if is_valid_token(&token) {
            repair_private_permissions(&token_path)?;
            eprintln!("[margins-server] auth token: {}", token_path.display());
            return Ok(token);
        }
        eprintln!(
            "[margins-server] replacing invalid auth token: {}",
            token_path.display()
        );
    }

    // Generate 32 random bytes → 64-char hex string
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();

    persist_token_atomically(data_dir, &token_path, &token)?;

    eprintln!(
        "[margins-server] generated auth token at: {}",
        token_path.display()
    );
    Ok(token)
}

/// Hosted tokens are exactly 32 bytes encoded as lowercase or uppercase hex.
/// Keeping this boundary narrow also makes JavaScript serialization inert.
pub fn is_valid_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn test_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "margins-auth-{label}-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ))
    }

    #[test]
    fn token_validation_accepts_only_exact_ascii_hex() {
        assert!(is_valid_token(&"a".repeat(64)));
        assert!(is_valid_token(&"A1".repeat(32)));
        assert!(!is_valid_token(&"a".repeat(63)));
        assert!(!is_valid_token(&"a".repeat(65)));
        assert!(!is_valid_token(&format!("{}g", "a".repeat(63))));
        assert!(!is_valid_token(&format!("{}é", "a".repeat(62))));
    }

    #[test]
    fn failed_atomic_replace_does_not_remove_the_existing_file() {
        let data_dir = test_dir("failed-replace");
        std::fs::create_dir_all(&data_dir).unwrap();
        let temp_path = data_dir.join("token.tmp");
        let token_path = data_dir.join("token");
        std::fs::write(&temp_path, "replacement").unwrap();
        std::fs::write(&token_path, "existing").unwrap();

        let error = replace_file_with(&temp_path, &token_path, |_, _| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected replacement failure",
            ))
        })
        .unwrap_err();

        assert!(error.to_string().contains("failed to install token file"));
        assert_eq!(std::fs::read_to_string(&token_path).unwrap(), "existing");
        assert_eq!(std::fs::read_to_string(&temp_path).unwrap(), "replacement");
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn invalid_persisted_token_is_replaced_safely() {
        let data_dir = test_dir("replace");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(
            data_dir.join("token"),
            "</script><script>globalThis.pwned=true</script>",
        )
        .unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(
            data_dir.join("token"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        let token = load_or_create_token(&data_dir).unwrap();
        assert!(is_valid_token(&token));
        assert_eq!(
            std::fs::read_to_string(data_dir.join("token")).unwrap(),
            token
        );
        #[cfg(unix)]
        assert_eq!(mode(&data_dir.join("token")), 0o600);
        assert!(std::fs::read_dir(&data_dir).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".token.tmp-")));
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn persisted_token_must_match_the_exact_hex_format() {
        let data_dir = test_dir("exact");
        std::fs::create_dir_all(&data_dir).unwrap();
        let token_path = data_dir.join("token");
        let invalid = format!("{}\n", "a".repeat(64));
        std::fs::write(&token_path, &invalid).unwrap();

        let token = load_or_create_token(&data_dir).unwrap();
        assert!(is_valid_token(&token));
        assert_ne!(token, invalid);
        assert_eq!(std::fs::read_to_string(&token_path).unwrap(), token);
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn valid_existing_token_permissions_are_repaired_before_return() {
        let data_dir = test_dir("repair");
        std::fs::create_dir_all(&data_dir).unwrap();
        let token_path = data_dir.join("token");
        let expected = "A1".repeat(32);
        std::fs::write(&token_path, &expected).unwrap();
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(load_or_create_token(&data_dir).unwrap(), expected);
        assert_eq!(mode(&token_path), 0o600);
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_initialization_returns_one_private_persisted_token() {
        let data_dir = test_dir("concurrent");
        let start = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let data_dir = data_dir.clone();
                let start = start.clone();
                std::thread::spawn(move || {
                    start.wait();
                    load_or_create_token(&data_dir).unwrap()
                })
            })
            .collect();
        let tokens: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();

        assert!(tokens.iter().all(|token| token == &tokens[0]));
        assert!(is_valid_token(&tokens[0]));
        assert_eq!(
            std::fs::read_to_string(data_dir.join("token")).unwrap(),
            tokens[0]
        );
        assert_eq!(mode(&data_dir.join("token")), 0o600);
        assert_eq!(mode(&data_dir.join(".token.lock")), 0o600);
        std::fs::remove_dir_all(data_dir).unwrap();
    }
}
