fn main() {
    embed_live_runtime_info_plist();

    #[cfg(feature = "tauri-app")]
    {
        ensure_sidecar("margins-cli", "scripts/build-margins-sidecar.sh");
        tauri_build::build()
    }
}

/// A command-line executable has no app bundle from which macOS can read its
/// privacy purpose strings. Put the live runtime's plist in the Mach-O itself
/// so microphone and system-audio permission prompts work when bb launches the
/// standalone binary.
fn embed_live_runtime_info_plist() {
    println!("cargo:rerun-if-changed=MarginsLive-Info.plist");

    let targets_macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    let builds_live_runtime = std::env::var_os("CARGO_FEATURE_LIVE_RUNTIME").is_some();
    if !targets_macos || !builds_live_runtime {
        return;
    }

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let plist = std::path::Path::new(&manifest_dir).join("MarginsLive-Info.plist");
    assert!(
        plist.is_file(),
        "margins-live information property list is missing: {}",
        plist.display()
    );
    println!(
        "cargo:rustc-link-arg-bin=margins-live=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );
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
