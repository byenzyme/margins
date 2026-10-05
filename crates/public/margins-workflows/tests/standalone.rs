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

/// `enzyme-spec` is a draft path dependency on a sibling checkout until its
/// public repository exists. Point the isolated copy at the same crate so the
/// check still proves this tree needs nothing else from the repository.
fn pin_sibling_path_dependencies(original_crate: &Path, copied_manifest: &Path) {
    let manifest = std::fs::read_to_string(copied_manifest).unwrap();
    let rewritten = manifest
        .lines()
        .map(|line| {
            let Some(rest) = line.strip_prefix("enzyme-spec = { path = \"") else {
                return line.to_string();
            };
            let relative = rest.split('"').next().unwrap();
            let absolute = original_crate.join(relative).canonicalize().unwrap();
            format!("enzyme-spec = {{ path = {:?} }}", absolute)
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(copied_manifest, rewritten + "\n").unwrap();
}

#[test]
fn crate_builds_from_an_isolated_public_tree() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let public_dir = manifest_dir.parent().unwrap();
    let temp = tempfile::tempdir().unwrap();
    for name in [
        "margins-core",
        "margins-meeting-protocol",
        "margins-meeting-runtime",
        "margins-media",
        "margins-store",
        "margins-workflows",
    ] {
        copy_tree(&public_dir.join(name), &temp.path().join(name));
    }
    // Keep the dependency versions under test when checking an isolated tree.
    // Fresh offline resolution can reject a yanked transitive version that is
    // valid in the repository's existing lockfile.
    let repo_root = public_dir.parent().unwrap().parent().unwrap();
    std::fs::copy(
        repo_root.join("Cargo.lock"),
        temp.path().join("margins-workflows/Cargo.lock"),
    )
    .unwrap();
    let manifest = temp.path().join("margins-workflows/Cargo.toml");
    pin_sibling_path_dependencies(&public_dir.join("margins-workflows"), &manifest);
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
        .env("CARGO_BUILD_BUILD_DIR", temp.path().join("target"))
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}
