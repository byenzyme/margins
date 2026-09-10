//! Shared contract for the desktop-brokered OpenRouter "included" lease.
//!
//! The Margins desktop app mints a short-lived OpenRouter provider key and
//! stores it in the macOS Keychain. This module keeps the desktop and root crate
//! aligned on the Keychain coordinates and JSON shape. The lease is not an
//! Enzyme account identity or bearer, and recall credential resolution must not
//! treat it as one.
//!
//! Not feature-gated: the desktop consumes it regardless of the `recall` feature.

use serde::{Deserialize, Serialize};

/// Keychain service scope (fed to `keychain_service(scope)`, which prefixes the
/// profile — e.g. `margins.included-openrouter` for the default profile).
pub const KEYCHAIN_SCOPE: &str = "included-openrouter";

/// Keychain account holding the serialized [`IncludedLease`].
pub const KEYCHAIN_ACCOUNT: &str = "lease-v1";

/// The brokered lease as persisted in the Keychain. `expires_at` is epoch
/// seconds (absent means no known expiry).
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct IncludedLease {
    pub api_key: String,
    pub expires_at: Option<i64>,
}

impl IncludedLease {
    /// Whether the lease is past its expiry at `now_epoch_secs` (a lease with no
    /// expiry is never considered expired here).
    pub fn is_expired_at(&self, now_epoch_secs: i64) -> bool {
        self.expires_at
            .is_some_and(|expires_at| expires_at <= now_epoch_secs)
    }

    /// The trimmed key, if non-empty.
    pub fn usable_key(&self) -> Option<&str> {
        let key = self.api_key.trim();
        (!key.is_empty()).then_some(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_values_are_stable() {
        // These strings are a cross-process contract with the desktop Keychain.
        // If either side changes them, both sides must change together.
        assert_eq!(KEYCHAIN_SCOPE, "included-openrouter");
        assert_eq!(KEYCHAIN_ACCOUNT, "lease-v1");
    }

    #[test]
    fn json_shape_round_trips() {
        let lease = IncludedLease {
            api_key: "sk-or-v1-abc".into(),
            expires_at: Some(1_800_000_000),
        };
        let json = serde_json::to_string(&lease).unwrap();
        assert_eq!(json, r#"{"api_key":"sk-or-v1-abc","expires_at":1800000000}"#);
        assert_eq!(serde_json::from_str::<IncludedLease>(&json).unwrap(), lease);
    }

    #[test]
    fn expiry_and_usable_key() {
        let lease = IncludedLease {
            api_key: "  k  ".into(),
            expires_at: Some(100),
        };
        assert!(lease.is_expired_at(100));
        assert!(lease.is_expired_at(200));
        assert!(!lease.is_expired_at(99));
        assert_eq!(lease.usable_key(), Some("k"));

        let no_expiry = IncludedLease {
            api_key: String::new(),
            expires_at: None,
        };
        assert!(!no_expiry.is_expired_at(i64::MAX));
        assert_eq!(no_expiry.usable_key(), None);
    }
}
