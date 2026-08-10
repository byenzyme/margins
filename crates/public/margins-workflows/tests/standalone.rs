use std::path::{Path, PathBuf};

fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn crate_builds_from_an_isolated_public_tree() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let public_dir = manifest_dir.parent().unwrap();
    let temp = tempfile::tempdir().unwrap();
    for name in [
        "margins-core",
        "margins-media",
        "margins-store",
        "margins-workflows",
        "margins-meeting-protocol",
    ] {
        copy_tree(&public_dir.join(name), &temp.path().join(name));
    }
    let manifest = temp.path().join("margins-workflows/Cargo.toml");
    let output = std::process::Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(&manifest)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = String::from_utf8(output.stdout).unwrap();
    assert!(!metadata.contains("desktop/src-tauri"));
    assert!(!metadata.contains("crates/private"));

    let check = std::process::Command::new("cargo")
        .args(["check", "--lib", "--offline", "--manifest-path"])
        .arg(&manifest)
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}
