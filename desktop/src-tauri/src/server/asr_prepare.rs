//! Machine-level, pinned Linux ASR assets for the project recording service.
//! Downloads stage beside the final cache and become visible only after checks.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::Command,
};

struct InstallLock(fs::File);

impl InstallLock {
    fn acquire(root: &Path) -> Result<Self> {
        fs::create_dir_all(root)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(root.join("install.lock"))?;
        anyhow::ensure!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0,
            "could not lock the shared speech cache"
        );
        Ok(Self(file))
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn install_dir(staged: &Path, final_dir: &Path) -> Result<()> {
    let backup = final_dir.with_extension(format!("old-{}", std::process::id()));
    if backup.exists() {
        if final_dir.exists() {
            fs::remove_dir_all(&backup)?;
        } else {
            fs::rename(&backup, final_dir)?;
        }
    }
    let had_previous = final_dir.exists();
    if had_previous {
        fs::rename(final_dir, &backup)?;
    }
    if let Err(error) = fs::rename(staged, final_dir) {
        if had_previous {
            let _ = fs::rename(&backup, final_dir);
        }
        return Err(error.into());
    }
    if had_previous {
        let _ = fs::remove_dir_all(backup);
    }
    Ok(())
}

const MODEL_REVISION: &str = "d64884b484b919e9656d0b70cb95dfdc98852bef";
const MODEL_FILES: [(&str, u64, &str); 3] = [
    (
        "encoder-model.int8.onnx",
        652_282_300,
        "464e6ba9e9ee681361b52801d90f13be3cf01a9252a584f14023dab6197aedf9",
    ),
    (
        "decoder_joint-model.int8.onnx",
        8_998_557,
        "93fbb1856f8f897004a15793e7b78953879003e03fdac56dc76b76ccc2ce6505",
    ),
    (
        "vocab.txt",
        9_384,
        "ec182b70dd42113aff6c5372c75cac58c952443eb22322f57bbd7f53977d497d",
    ),
];

fn cache_root() -> Result<PathBuf> {
    Ok(dirs::cache_dir()
        .context("cannot locate the machine model cache")?
        .join("margins/asr"))
}

fn model_dir() -> Result<PathBuf> {
    Ok(cache_root()?.join("parakeet-tdt-0.6b-v2-int8"))
}

fn ort_name_and_digest() -> Result<(&'static str, &'static str)> {
    match std::env::consts::ARCH {
        "x86_64" => Ok((
            "onnxruntime-linux-x64-1.24.2",
            "43725474ba5663642e17684717946693850e2005efbd724ac72da278fead25e6",
        )),
        "aarch64" => Ok((
            "onnxruntime-linux-aarch64-1.24.2",
            "6715b3d19965a2a6981e78ed4ba24f17a8c30d2d26420dbed10aac7ceca0085e",
        )),
        other => bail!("no pinned ONNX Runtime package for {other}"),
    }
}

fn ort_path() -> Result<PathBuf> {
    let (name, _) = ort_name_and_digest()?;
    Ok(cache_root()?
        .join(name)
        .join("lib/libonnxruntime.so.1.24.2"))
}

/// Configure default locations before spawning threads or loading ONNX Runtime.
pub fn configure_env() -> Result<()> {
    if std::env::var_os("MARGINS_PARAKEET_MODEL_DIR").is_none() {
        std::env::set_var("MARGINS_PARAKEET_MODEL_DIR", model_dir()?);
        std::env::set_var("MARGINS_MANAGED_ASR_MODEL", "1");
    }
    if std::env::var_os("ORT_DYLIB_PATH").is_none() {
        std::env::set_var("ORT_DYLIB_PATH", ort_path()?);
        std::env::set_var("MARGINS_MANAGED_ASR_RUNTIME", "1");
    }
    if std::env::var_os("MARGINS_MANAGED_ASR_MODEL").is_some()
        && std::env::var_os("MARGINS_PARAKEET_MODEL_KIND").is_none()
    {
        std::env::set_var("MARGINS_PARAKEET_MODEL_KIND", "tdt-v2");
    }
    Ok(())
}

fn digest_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn checked_download(
    client: &reqwest::blocking::Client,
    url: &str,
    target: &Path,
    size: u64,
    sha256: &str,
    progress: &(dyn Fn(String, Option<f32>) + Send + Sync),
    label: &str,
    completed_bytes: u64,
    total_bytes: u64,
) -> Result<()> {
    let mut response = client.get(url).send()?.error_for_status()?;
    let mut output = fs::File::create(target)?;
    let mut bytes = 0;
    let mut last_reported = 0;
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let count = response.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        bytes += count as u64;
        if bytes - last_reported >= 1024 * 1024 {
            last_reported = bytes;
            progress(
                format!("Downloading {label}"),
                Some(((completed_bytes + bytes) as f32 / total_bytes as f32).min(0.98)),
            );
        }
    }
    output.sync_all()?;
    anyhow::ensure!(
        bytes == size,
        "incomplete ASR download: {bytes} of {size} bytes"
    );
    anyhow::ensure!(
        digest_file(target)? == sha256,
        "ASR download digest mismatch"
    );
    Ok(())
}

pub fn prepare(progress: &(dyn Fn(String, Option<f32>) + Send + Sync)) -> Result<()> {
    let root = cache_root()?;
    let _lock = InstallLock::acquire(&root)?;
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()?;

    if std::env::var_os("MARGINS_MANAGED_ASR_MODEL").is_some() {
        let final_dir = model_dir()?;
        let ready = MODEL_FILES.iter().all(|(name, size, digest)| {
            let path = final_dir.join(name);
            path.metadata().is_ok_and(|meta| meta.len() == *size)
                && digest_file(&path).is_ok_and(|actual| actual == *digest)
        });
        if !ready {
            let parent = final_dir.parent().context("model cache has no parent")?;
            fs::create_dir_all(parent)?;
            let stage = tempfile::tempdir_in(parent)?;
            let total_bytes = MODEL_FILES.iter().map(|(_, size, _)| *size).sum::<u64>()
                + if std::env::consts::ARCH == "x86_64" {
                    8_123_282
                } else {
                    7_135_756
                };
            let mut completed_bytes = 0;
            for (name, size, digest) in MODEL_FILES {
                eprintln!("[margins-server] downloading ASR model {name}");
                let url = format!("https://huggingface.co/smcleod/parakeet-tdt-0.6b-v2-int8/resolve/{MODEL_REVISION}/{name}");
                checked_download(
                    &client,
                    &url,
                    &stage.path().join(name),
                    size,
                    digest,
                    progress,
                    "transcription model",
                    completed_bytes,
                    total_bytes,
                )?;
                completed_bytes += size;
            }
            install_dir(stage.path(), &final_dir)?;
        }
    }

    if std::env::var_os("MARGINS_MANAGED_ASR_RUNTIME").is_some() {
        let final_path = ort_path()?;
        if !final_path.is_file() {
            let (name, digest) = ort_name_and_digest()?;
            let stage = tempfile::tempdir_in(&root)?;
            let archive = stage.path().join("onnxruntime.tgz");
            let url = format!(
                "https://github.com/microsoft/onnxruntime/releases/download/v1.24.2/{name}.tgz"
            );
            eprintln!("[margins-server] downloading ONNX Runtime {name}");
            let size = if std::env::consts::ARCH == "x86_64" {
                8_123_282
            } else {
                7_135_756
            };
            let model_bytes = MODEL_FILES.iter().map(|(_, size, _)| *size).sum::<u64>();
            progress(
                "Downloading transcription runtime".into(),
                Some(model_bytes as f32 / (model_bytes + size) as f32),
            );
            checked_download(
                &client,
                &url,
                &archive,
                size,
                digest,
                progress,
                "transcription runtime",
                model_bytes,
                model_bytes + size,
            )?;
            let status = Command::new("tar")
                .args([
                    "-xzf",
                    archive.to_str().context("invalid archive path")?,
                    "-C",
                    stage.path().to_str().context("invalid staging path")?,
                ])
                .status()?;
            anyhow::ensure!(status.success(), "could not unpack ONNX Runtime");
            let unpacked = stage.path().join(name);
            anyhow::ensure!(
                unpacked.join("lib/libonnxruntime.so.1.24.2").is_file(),
                "ONNX Runtime library is missing"
            );
            let final_dir = root.join(name);
            install_dir(&unpacked, &final_dir)?;
        }
    }
    progress("Verifying transcription runtime".into(), Some(0.99));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_cache_keeps_complete_new_directory() {
        let root = tempfile::tempdir().unwrap();
        let final_dir = root.path().join("model");
        let staged = root.path().join("staged");
        fs::create_dir(&final_dir).unwrap();
        fs::create_dir(&staged).unwrap();
        fs::write(final_dir.join("version"), "old").unwrap();
        fs::write(staged.join("version"), "new").unwrap();

        let _lock = InstallLock::acquire(root.path()).unwrap();
        install_dir(&staged, &final_dir).unwrap();

        assert_eq!(
            fs::read_to_string(final_dir.join("version")).unwrap(),
            "new"
        );
        assert!(!staged.exists());
    }
}
