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
}
