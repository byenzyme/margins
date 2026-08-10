use margins_core::{
    AsrBackend, CaptureHandle, CaptureObserver, CaptureProvider, DiarizationBackend, EventSink,
    SessionRepository,
};

fn assert_send_sync<T: ?Sized + Send + Sync>() {}

#[test]
fn public_ports_are_object_safe_send_and_sync() {
    assert_send_sync::<dyn CaptureProvider>();
    assert_send_sync::<dyn CaptureHandle>();
    assert_send_sync::<dyn CaptureObserver>();
    assert_send_sync::<dyn SessionRepository>();
    assert_send_sync::<dyn AsrBackend>();
    assert_send_sync::<dyn DiarizationBackend>();
    assert_send_sync::<dyn EventSink>();
}

#[test]
fn direct_dependency_graph_is_platform_neutral() {
    let manifest = include_str!("../Cargo.toml");
    for forbidden in [
        "anyhow",
        "cpal",
        "cidre",
        "tauri",
        "rusqlite",
        "coreaudio",
        "windows",
        "objc2",
        "tokio",
    ] {
        assert!(
            !manifest.to_ascii_lowercase().contains(forbidden),
            "public manifest contains forbidden dependency or platform binding: {forbidden}"
        );
    }

    let public_source = [
        include_str!("../src/audio.rs"),
        include_str!("../src/capture.rs"),
        include_str!("../src/event.rs"),
        include_str!("../src/ids.rs"),
        include_str!("../src/memo.rs"),
        include_str!("../src/session.rs"),
        include_str!("../src/transcript.rs"),
    ]
    .join("\n")
    .to_ascii_lowercase();

    for forbidden in [
        "cpal::",
        "cidre::",
        "tauri::",
        "rusqlite::",
        "anyhow::",
        "std::os::",
        "rawfd",
        "rawhandle",
    ] {
        assert!(
            !public_source.contains(forbidden),
            "public source exposes forbidden implementation type: {forbidden}"
        );
    }
}
