mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    static PROCESS_ENV_LOCK: Mutex<()> = Mutex::new(());

    include!("tests/capture_remote.rs");
    include!("tests/setup.rs");
    include!("tests/capture_local.rs");
    include!("tests/dispatch.rs");
    include!("tests/journey.rs");
}
#[cfg(test)]
mod bare_capture_decision_tests {
    use super::*;

    #[test]
    fn bare_capture_creates_only_when_current_is_absent_or_stale() {
        assert!(bare_capture_creates(None, false));
        assert!(bare_capture_creates(Some(""), false));
        assert!(bare_capture_creates(Some("stale"), false));
        assert!(!bare_capture_creates(Some("current"), true));
    }
}
