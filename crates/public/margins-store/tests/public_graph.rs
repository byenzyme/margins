use margins_core::SessionRepository;
use margins_store::SqliteSessionRepository;

fn assert_send_sync<T: ?Sized + Send + Sync>() {}

#[test]
fn repository_is_a_send_sync_core_port_implementation() {
    assert_send_sync::<SqliteSessionRepository>();
    assert_send_sync::<dyn SessionRepository>();
}

#[test]
fn manifest_and_source_have_no_private_or_native_edge() {
    let manifest = include_str!("../Cargo.toml").to_ascii_lowercase();
    let source = [
        include_str!("../src/lib.rs"),
        include_str!("../src/index.rs"),
        include_str!("../src/legacy.rs"),
        include_str!("../src/sqlite.rs"),
    ]
    .join("\n")
    .to_ascii_lowercase();
    for forbidden in [
        "margins-capture-native",
        "margins-desktop",
        "tauri",
        "cpal",
        "cidre",
        "pi_agent_rust",
        "windows =",
        "desktop/",
        "crates/private",
    ] {
        assert!(
            !manifest.contains(forbidden),
            "manifest contains {forbidden}"
        );
        assert!(!source.contains(forbidden), "source contains {forbidden}");
    }
}
