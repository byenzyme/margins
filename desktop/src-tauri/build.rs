fn main() {
    #[cfg(feature = "tauri-app")]
    {
        ensure_sidecar("margins-cli", "scripts/build-margins-sidecar.sh");
        tauri_build::build()
    }
}

/// tauri.conf.json declares externalBin sidecars, so the target-triple-suffixed
/// binaries must exist before tauri_build runs. Stage each one when it's
/// missing so plain `cargo check`/`cargo test` keep working on a fresh
/// checkout. The staging scripts never build into the shared target dir (it is
/// locked by the outer cargo invocation).
fn ensure_sidecar(name: &str, script: &str) {
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let sidecar = std::path::Path::new(&manifest_dir)
        .join("binaries")
        .join(format!("{name}-{target}"));
    if sidecar.is_file() {
        return;
    }
    let script_path = std::path::Path::new(&manifest_dir).join(script);
    let status = std::process::Command::new("bash")
        .arg(&script_path)
        .arg(&target)
        .status()
        .unwrap_or_else(|e| panic!("could not run {script}: {e}"));
    assert!(
        status.success(),
        "{script} failed for {target}; run it manually or place the binary at {}",
        sidecar.display()
    );
}
