use serde::Serialize;

/// Provenance embedded by `build.rs` in every public and official CLI build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildInfo {
    pub commit: &'static str,
    pub short: &'static str,
    pub dirty: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<&'static str>,
    pub built_at: &'static str,
    pub profile: &'static str,
}

pub fn get() -> BuildInfo {
    let branch = env!("MARGINS_BUILD_BRANCH_VALUE");
    BuildInfo {
        commit: env!("MARGINS_BUILD_COMMIT_VALUE"),
        short: env!("MARGINS_BUILD_SHORT_VALUE"),
        dirty: env!("MARGINS_BUILD_DIRTY_VALUE") == "true",
        branch: (!branch.is_empty()).then_some(branch),
        built_at: env!("MARGINS_BUILD_AT_VALUE"),
        profile: env!("MARGINS_BUILD_PROFILE_VALUE"),
    }
}

/// One-line `--version` report, e.g. `margins 0.4.15 (582d77f61abc, official)`.
/// `version` is the calling binary's package version; `composition` matches
/// the `composition` field of `margins capabilities`.
pub fn version_line(version: &str, composition: &str) -> String {
    let info = get();
    let dirty = if info.dirty { "-dirty" } else { "" };
    format!("margins {version} ({}{dirty}, {composition})", info.short)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_object_has_stable_shape() {
        let value = serde_json::to_value(get()).unwrap();
        let object = value.as_object().unwrap();
        for field in ["commit", "short", "dirty", "built_at", "profile"] {
            assert!(object.contains_key(field), "missing build field {field}");
        }
        assert!(value["commit"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        assert!(value["short"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        assert!(value["dirty"].is_boolean());
        assert!(chrono::DateTime::parse_from_rfc3339(value["built_at"].as_str().unwrap()).is_ok());
        assert!(value["profile"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        if object.contains_key("branch") {
            assert!(value["branch"]
                .as_str()
                .is_some_and(|value| !value.is_empty()));
        }
    }

    #[test]
    fn version_line_names_version_commit_and_composition() {
        let line = version_line(env!("CARGO_PKG_VERSION"), "public");
        let info = get();
        assert!(line.starts_with(&format!("margins {} (", env!("CARGO_PKG_VERSION"))));
        assert!(line.contains(info.short));
        assert!(line.ends_with(", public)"));
    }
}
