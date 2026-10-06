use chrono::{SecondsFormat, Utc};
use std::collections::BTreeSet;
use std::env;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_path(repo: &Path, path: &str) -> Option<String> {
    git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-path", path],
    )
    .filter(|value| !value.is_empty())
}

fn emit_git_rerun_paths(repo: &Path) {
    // Cargo may reuse this build-script output across linked-worktree checkouts.
    // Track both the per-worktree pointers and the common ref storage so the
    // embedded identity follows detach/checkout, branch advances, and staging.
    let mut paths = BTreeSet::new();
    for path in ["HEAD", "index", "packed-refs"] {
        if let Some(path) = git_path(repo, path) {
            paths.insert(path);
        }
    }
    if let Some(reference) = git(repo, &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git_path(repo, &reference) {
            paths.insert(path);
        }
    }
    for path in paths {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=MARGINS_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=MARGINS_BUILD_DIRTY");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let repo = Path::new(&manifest_dir);
    emit_git_rerun_paths(repo);
    let git_commit = git(repo, &["rev-parse", "HEAD"]).filter(|value| !value.is_empty());
    let override_commit = env::var("MARGINS_BUILD_COMMIT")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let commit = override_commit
        .as_deref()
        .or(git_commit.as_deref())
        .unwrap_or("unknown");
    let short = if commit == "unknown" {
        "unknown".to_string()
    } else {
        commit.chars().take(12).collect()
    };
    let git_available = git_commit.is_some();
    // Like MARGINS_BUILD_COMMIT, an explicit "true"/"false" overrides the
    // checkout's own `git status`.
    let dirty_override =
        env::var("MARGINS_BUILD_DIRTY")
            .ok()
            .and_then(|value| match value.trim() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            });
    let dirty = if let Some(dirty) = dirty_override {
        dirty
    } else if git_available {
        git(repo, &["status", "--porcelain"]).is_some_and(|status| !status.is_empty())
    } else {
        commit == "unknown"
    };
    let branch = git(repo, &["symbolic-ref", "--short", "HEAD"]).unwrap_or_default();
    let built_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let profile = env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=MARGINS_BUILD_COMMIT_VALUE={commit}");
    println!("cargo:rustc-env=MARGINS_BUILD_SHORT_VALUE={short}");
    println!("cargo:rustc-env=MARGINS_BUILD_DIRTY_VALUE={dirty}");
    println!("cargo:rustc-env=MARGINS_BUILD_BRANCH_VALUE={branch}");
    println!("cargo:rustc-env=MARGINS_BUILD_AT_VALUE={built_at}");
    println!("cargo:rustc-env=MARGINS_BUILD_PROFILE_VALUE={profile}");
}
