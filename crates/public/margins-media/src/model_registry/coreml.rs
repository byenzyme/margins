use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const TREE_URL: &str = "https://huggingface.co/api/models/FluidInference/parakeet-tdt-0.6b-v2-coreml/tree/main?recursive=true";
const FILE_URL: &str =
    "https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v2-coreml/resolve/main";
const REQUIRED_DIRS: &[&str] = &[
    "Preprocessor.mlmodelc",
    "Encoder.mlmodelc",
    "Decoder.mlmodelc",
    "JointDecision.mlmodelc",
];
// Leave bandwidth and one execution lane available for the Polyvoice speaker
// models, which `margins setup` provisions concurrently.
const MAX_DOWNLOAD_WORKERS: usize = 6;

#[derive(Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    size: Option<u64>,
    lfs: Option<LfsEntry>,
}

#[derive(Deserialize)]
struct LfsEntry {
    size: u64,
}

impl TreeEntry {
    fn size(&self) -> u64 {
        self.lfs
            .as_ref()
            .map(|lfs| lfs.size)
            .or(self.size)
            .unwrap_or(0)
    }
}

pub fn find_installed_model() -> Option<PathBuf> {
    resolve_installed_model_from(
        super::env_path("MARGINS_FLUID_COREML_MODEL_DIR"),
        dirs::home_dir(),
        std::env::var("MARGINS_FLUID_COREML_VERSION")
            .ok()
            .as_deref(),
    )
}

fn resolve_installed_model_from(
    configured: Option<PathBuf>,
    home: Option<PathBuf>,
    version: Option<&str>,
) -> Option<PathBuf> {
    if let Some(configured) = configured {
        return valid_model(&configured).then_some(configured);
    }
    let root = home?.join("Library/Application Support/FluidAudio/Models");
    let preferred = match version.map(str::to_ascii_lowercase).as_deref() {
        Some("v3") | Some("3") => "parakeet-tdt-0.6b-v3",
        _ => "parakeet-tdt-0.6b-v2",
    };
    [
        root.join(preferred),
        root.join("parakeet-tdt-0.6b-v2"),
        root.join("parakeet-tdt-0.6b-v3"),
    ]
    .into_iter()
    .find(|path| valid_model(path))
}

pub fn download_model(mut progress: impl FnMut(u64, u64)) -> Result<PathBuf> {
    download_model_from(TREE_URL, FILE_URL, download_target()?, &mut progress)
}

fn download_model_from(
    tree_url: &str,
    file_url: &str,
    final_dir: PathBuf,
    progress: &mut impl FnMut(u64, u64),
) -> Result<PathBuf> {
    let parent = final_dir
        .parent()
        .context("model directory has no parent")?;
    fs::create_dir_all(parent)?;
    let tree = Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--retry",
            "2",
            tree_url,
        ])
        .output()
        .context("could not start curl")?;
    if !tree.status.success() {
        bail!("could not fetch the model file list");
    }
    let entries: Vec<TreeEntry> =
        serde_json::from_slice(&tree.stdout).context("could not parse the model file list")?;
    let entries: Vec<_> = entries
        .into_iter()
        .filter(|entry| entry.kind == "file" && required_path(&entry.path))
        .collect();
    if entries.is_empty() {
        bail!("the model repository did not contain the required assets");
    }
    let total = entries.iter().map(TreeEntry::size).sum();
    let target_name = final_dir
        .file_name()
        .context("model directory has no final component")?
        .to_string_lossy();
    // Keep a stable staging directory so an interrupted setup can resume its
    // verified partial files on the next run.
    let temp = parent.join(format!(".{target_name}.downloading"));
    fs::create_dir_all(&temp)?;

    let result = (|| -> Result<()> {
        let mut pending = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            let destination = temp.join(&entry.path);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let present = fs::metadata(&destination)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            if entry.size() > 0 && present == entry.size() {
                continue;
            }
            if entry.size() > 0 && present > entry.size() {
                fs::remove_file(&destination)?;
            }
            pending.push(index);
        }

        let staged = entries
            .iter()
            .map(|entry| {
                fs::metadata(temp.join(&entry.path))
                    .map(|metadata| metadata.len().min(entry.size()))
                    .unwrap_or(0)
            })
            .sum();
        progress(staged, total);

        let mut active = Vec::new();
        let mut next = 0;
        loop {
            while active.len() < MAX_DOWNLOAD_WORKERS && next < pending.len() {
                let index = pending[next];
                next += 1;
                let entry = &entries[index];
                let destination = temp.join(&entry.path);
                let url = format!("{file_url}/{}", entry.path);
                let child = match Command::new("/usr/bin/curl")
                    .args([
                        "--fail",
                        "--silent",
                        "--show-error",
                        "--location",
                        "--retry",
                        "2",
                        "--continue-at",
                        "-",
                        "--output",
                    ])
                    .arg(&destination)
                    .arg(&url)
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .spawn()
                {
                    Ok(child) => child,
                    Err(error) => {
                        stop_downloads(&mut active);
                        return Err(error)
                            .with_context(|| format!("could not download {}", entry.path));
                    }
                };
                active.push((index, child));
            }

            let downloaded = entries
                .iter()
                .map(|entry| {
                    fs::metadata(temp.join(&entry.path))
                        .map(|metadata| metadata.len().min(entry.size()))
                        .unwrap_or(0)
                })
                .sum();
            progress(downloaded, total);

            let mut index = active.len();
            while index > 0 {
                index -= 1;
                let finished = active[index].1.try_wait()?;
                if let Some(status) = finished {
                    let (entry_index, mut child) = active.swap_remove(index);
                    if !status.success() {
                        let path = entries[entry_index].path.clone();
                        let mut detail = String::new();
                        if let Some(mut stderr) = child.stderr.take() {
                            let _ = stderr.read_to_string(&mut detail);
                        }
                        let _ = child.wait();
                        stop_downloads(&mut active);
                        let detail = detail.trim();
                        if detail.is_empty() {
                            bail!("could not download {path}");
                        }
                        bail!("could not download {path}: {detail}");
                    }
                }
            }

            if next == pending.len() && active.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        for entry in &entries {
            let file_bytes = fs::metadata(temp.join(&entry.path))?.len();
            if entry.size() > 0 && file_bytes != entry.size() {
                bail!(
                    "incomplete download for {} ({file_bytes} of {} bytes)",
                    entry.path,
                    entry.size()
                );
            }
        }
        if !valid_model(&temp) {
            bail!("downloaded model assets are incomplete");
        }
        Ok(())
    })();
    if let Err(error) = result {
        return Err(error).context("could not install local transcription");
    }
    let old = parent.join(format!(
        ".parakeet-tdt-0.6b-v2.previous-{}",
        std::process::id()
    ));
    if final_dir.exists() {
        fs::rename(&final_dir, &old).context("could not stage the previous model directory")?;
    }
    if let Err(error) = fs::rename(&temp, &final_dir) {
        if old.exists() {
            let _ = fs::rename(&old, &final_dir);
        }
        return Err(error).context("could not activate the downloaded model");
    }
    if old.exists() {
        let _ = fs::remove_dir_all(old);
    }
    progress(total, total);
    Ok(final_dir)
}

fn stop_downloads(active: &mut Vec<(usize, std::process::Child)>) {
    for (_, child) in active.iter_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    active.clear();
}

fn download_target() -> Result<PathBuf> {
    if let Some(path) = super::env_path("MARGINS_FLUID_COREML_MODEL_DIR") {
        return Ok(path);
    }
    Ok(dirs::home_dir()
        .context("could not resolve the home directory")?
        .join("Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v2"))
}

fn required_path(path: &str) -> bool {
    path == "parakeet_vocab.json"
        || REQUIRED_DIRS.contains(&path.split('/').next().unwrap_or_default())
}

pub fn valid_model(path: &Path) -> bool {
    REQUIRED_DIRS.iter().all(|dir| path.join(dir).is_dir())
        && path.join("parakeet_vocab.json").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn required_assets_exclude_alternate_bundles() {
        assert!(required_path("Encoder.mlmodelc/model.mil"));
        assert!(required_path("parakeet_vocab.json"));
        assert!(!required_path("ParakeetEncoder.mlmodelc/model.mil"));
        assert!(!required_path("README.md"));
    }

    #[test]
    fn validation_requires_every_asset() {
        let temp = tempfile::tempdir().unwrap();
        for directory in REQUIRED_DIRS {
            fs::create_dir(temp.path().join(directory)).unwrap();
        }
        assert!(!valid_model(temp.path()));
        fs::File::create(temp.path().join("parakeet_vocab.json")).unwrap();
        assert!(valid_model(temp.path()));
    }

    #[test]
    fn explicit_missing_dir_is_authoritative_and_version_selects_installed_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .join("Library/Application Support/FluidAudio/Models");
        let v2 = root.join("parakeet-tdt-0.6b-v2");
        let v3 = root.join("parakeet-tdt-0.6b-v3");
        for dir in [&v2, &v3] {
            for asset in REQUIRED_DIRS {
                fs::create_dir_all(dir.join(asset)).unwrap();
            }
            fs::write(dir.join("parakeet_vocab.json"), b"{}").unwrap();
        }
        let home = Some(temp.path().to_path_buf());
        assert_eq!(
            resolve_installed_model_from(None, home.clone(), None),
            Some(v2)
        );
        assert_eq!(
            resolve_installed_model_from(None, home.clone(), Some("v3")),
            Some(v3)
        );
        assert_eq!(
            resolve_installed_model_from(Some(root.join("missing")), home, None),
            None
        );
    }

    #[test]
    fn downloads_required_assets_from_fake_model_host() {
        let temp = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = stop.clone();
        let server = std::thread::spawn(move || {
            let files = [
                "Preprocessor.mlmodelc/coremldata.bin",
                "Encoder.mlmodelc/coremldata.bin",
                "Decoder.mlmodelc/coremldata.bin",
                "JointDecision.mlmodelc/coremldata.bin",
                "parakeet_vocab.json",
            ];
            while !server_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0u8; 2048];
                        let count = stream.read(&mut request).unwrap();
                        let line = String::from_utf8_lossy(&request[..count]);
                        let path = line.split_whitespace().nth(1).unwrap_or("/");
                        let (status, body) = if path == "/tree" {
                            ("200 OK", serde_json::to_vec(&files.iter().map(|path| serde_json::json!({"path": path, "type": "file", "size": 1})).collect::<Vec<_>>()).unwrap())
                        } else if files.iter().any(|file| path == format!("/files/{file}")) {
                            ("200 OK", vec![b'x'])
                        } else {
                            ("404 Not Found", Vec::new())
                        };
                        let header = format!(
                            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        stream.write_all(header.as_bytes()).unwrap();
                        stream.write_all(&body).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5))
                    }
                    Err(error) => panic!("fake model host failed: {error}"),
                }
            }
        });
        let target = temp.path().join("parakeet-tdt-0.6b-v2");
        let result = download_model_from(
            &format!("http://{address}/tree"),
            &format!("http://{address}/files"),
            target.clone(),
            &mut |_, _| {},
        );
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
        assert_eq!(result.unwrap(), target);
        assert!(valid_model(&target));
        assert_eq!(
            fs::read(target.join("Preprocessor.mlmodelc/coremldata.bin")).unwrap(),
            b"x"
        );
    }
}
