//! Read-only reporting of the machine-level catalyst selection.
//!
//! This reports only mode and a stable reason code. It never returns credential
//! material, contacts the hosted broker, discovers models, or changes setup.

use serde::Serialize;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CatalystMode {
    Hosted,
    Local,
    None,
}

impl CatalystMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hosted => "hosted",
            Self::Local => "local",
            Self::None => "none",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CatalystStatus {
    pub mode: CatalystMode,
    pub reason: &'static str,
}

impl CatalystStatus {
    const fn new(mode: CatalystMode, reason: &'static str) -> Self {
        Self { mode, reason }
    }
}

/// Report the setup-selected generator without resolving credentials. Explicit
/// environment credentials retain their documented precedence.
pub fn selected_status(margins_home: &Path) -> CatalystStatus {
    if std::env::var_os("OPENAI_API_KEY").is_some()
        || std::env::var_os("OPENROUTER_API_KEY").is_some()
    {
        return CatalystStatus::new(CatalystMode::Hosted, "explicit_env");
    }

    let config = match std::fs::read_to_string(margins_home.join("config.toml")) {
        Ok(contents) => match contents.parse::<toml::Value>() {
            Ok(config) => config,
            Err(_) => return CatalystStatus::new(CatalystMode::None, "invalid_config"),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CatalystStatus::new(CatalystMode::None, "setup_required");
        }
        Err(_) => return CatalystStatus::new(CatalystMode::None, "config_unreadable"),
    };

    match config
        .get("llm")
        .and_then(|llm| llm.get("mode"))
        .and_then(toml::Value::as_str)
        .unwrap_or("auto")
    {
        "local" => CatalystStatus::new(CatalystMode::Local, "local_selected"),
        "hosted" => hosted_bundle_status(margins_home),
        "auto" => CatalystStatus::new(CatalystMode::None, "setup_required"),
        _ => CatalystStatus::new(CatalystMode::None, "invalid_config"),
    }
}

fn hosted_bundle_status(margins_home: &Path) -> CatalystStatus {
    let contents = match std::fs::read_to_string(margins_home.join("llm-config-cache.json")) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CatalystStatus::new(CatalystMode::None, "hosted_bundle_missing");
        }
        Err(_) => return CatalystStatus::new(CatalystMode::None, "hosted_bundle_unreadable"),
    };
    let bundle: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(bundle) => bundle,
        Err(_) => return CatalystStatus::new(CatalystMode::None, "hosted_bundle_invalid"),
    };
    let required_string = |field: &str| {
        bundle
            .get(field)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    };
    if !required_string("api_key")
        || !required_string("base_url")
        || !required_string("model")
        || !required_string("profile")
    {
        return CatalystStatus::new(CatalystMode::None, "hosted_bundle_invalid");
    }
    let expires_at = match bundle.get("expires_at") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => match value.as_i64() {
            Some(value) => Some(value),
            None => return CatalystStatus::new(CatalystMode::None, "hosted_bundle_invalid"),
        },
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    if expires_at.is_some_and(|expiry| expiry <= now) {
        return CatalystStatus::new(CatalystMode::None, "hosted_bundle_expired");
    }
    CatalystStatus::new(CatalystMode::Hosted, "hosted_bundle_ready")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestore(Vec<(&'static str, Option<std::ffi::OsString>)>);

    impl EnvRestore {
        fn without_explicit_keys() -> Self {
            let names = ["OPENAI_API_KEY", "OPENROUTER_API_KEY"];
            let values = names
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect();
            for name in names {
                std::env::remove_var(name);
            }
            Self(values)
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    fn auto_requires_setup_and_hosted_requires_a_usable_bundle() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _env = EnvRestore::without_explicit_keys();
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            selected_status(home.path()),
            CatalystStatus::new(CatalystMode::None, "setup_required")
        );

        std::fs::write(
            home.path().join("config.toml"),
            "[llm]\nmode = \"hosted\"\n",
        )
        .unwrap();
        assert_eq!(
            selected_status(home.path()),
            CatalystStatus::new(CatalystMode::None, "hosted_bundle_missing")
        );
        std::fs::write(
            home.path().join("llm-config-cache.json"),
            r#"{"api_key":"fixture-key","base_url":"https://fixture.invalid/v1","model":"fixture-model","expires_at":4102444800,"cached_at":1,"profile":"fixture-profile"}"#,
        )
        .unwrap();
        assert_eq!(
            selected_status(home.path()),
            CatalystStatus::new(CatalystMode::Hosted, "hosted_bundle_ready")
        );
        std::fs::write(
            home.path().join("llm-config-cache.json"),
            r#"{"api_key":"fixture-key","base_url":"https://fixture.invalid/v1","model":"fixture-model","expires_at":1,"cached_at":1,"profile":"fixture-profile"}"#,
        )
        .unwrap();
        let expired = selected_status(home.path());
        assert_eq!(
            expired,
            CatalystStatus::new(CatalystMode::None, "hosted_bundle_expired")
        );
        assert_eq!(
            serde_json::to_value(expired).unwrap(),
            serde_json::json!({"mode": "none", "reason": "hosted_bundle_expired"})
        );
    }

    #[test]
    fn local_selection_is_reported_without_model_discovery() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _env = EnvRestore::without_explicit_keys();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), "[llm]\nmode = \"local\"\n").unwrap();
        assert_eq!(
            selected_status(home.path()),
            CatalystStatus::new(CatalystMode::Local, "local_selected")
        );
    }

    #[test]
    fn explicit_environment_key_has_visible_precedence() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _env = EnvRestore::without_explicit_keys();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("OPENROUTER_API_KEY", "fixture-key");
        assert_eq!(
            selected_status(home.path()),
            CatalystStatus::new(CatalystMode::Hosted, "explicit_env")
        );
    }
}
