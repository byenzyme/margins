#[test]
fn no_default_manifest_has_only_the_allowed_first_party_edge() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("default = []"));
    assert!(manifest.contains("margins-core"));

    for forbidden in [
        "margins-capture-native",
        "margins-desktop",
        "cpal",
        "cidre",
        "tauri",
        "windows =",
        "rusqlite",
        "tokio",
        "pi_agent_rust",
    ] {
        assert!(
            !manifest.to_ascii_lowercase().contains(forbidden),
            "no-default media manifest contains forbidden edge: {forbidden}"
        );
    }
}

#[test]
fn portable_sources_do_not_reference_private_capture_or_desktop_types() {
    let source = [
        include_str!("../src/audio.rs"),
        include_str!("../src/diarization.rs"),
        include_str!("../src/info.rs"),
        include_str!("../src/lib.rs"),
        include_str!("../src/providers/coreml.rs"),
        include_str!("../src/providers/mod.rs"),
        include_str!("../src/providers/parakeet.rs"),
        include_str!("../src/providers/polyvoice.rs"),
        include_str!("../src/timeline.rs"),
        include_str!("../src/transcript.rs"),
    ]
    .join("\n");

    for forbidden in [
        "SegmentWriter",
        "LaneMsg",
        "CaptureSink",
        "PacketDesc",
        "cpal::",
        "cidre::",
        "tauri::",
        "AudioDeviceCreateIOProcID",
        "CAPTURE_DEVICE_CHANGED_EVENT",
    ] {
        assert!(
            !source.contains(forbidden),
            "portable media source references private symbol: {forbidden}"
        );
    }
}

#[test]
fn model_feature_graph_is_exact() {
    let media = include_str!("../Cargo.toml");
    for edge in [
        "parakeet-onnx = [\"dep:libloading\", \"dep:ndarray\", \"dep:ort\", \"dep:realfft\"]",
        "parakeet-onnx-dynamic = [\"parakeet-onnx\", \"ort/load-dynamic\"]",
        "coreml-asr = [\"dep:block2\", \"dep:objc2\", \"dep:objc2-core-ml\", \"dep:objc2-foundation\"]",
        "polyvoice-diarization = [\"dep:polyvoice\"]",
        "polyvoice-coreml = [\"polyvoice-diarization\", \"polyvoice/coreml\"]",
    ] {
        assert!(media.contains(edge), "missing public media feature edge: {edge}");
    }
}

#[test]
fn public_module_exports_are_allowlisted() {
    let source = include_str!("../src/lib.rs");
    let modules = source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub mod "))
        .filter_map(|line| line.strip_suffix(';'))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        modules,
        [
            "audio",
            "diarization",
            "info",
            "providers",
            "timeline",
            "transcript"
        ]
        .into_iter()
        .collect()
    );
}
