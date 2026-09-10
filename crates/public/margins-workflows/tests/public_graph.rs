#[test]
fn no_default_graph_has_only_public_portable_edges() {
    let manifest = include_str!("../Cargo.toml").to_ascii_lowercase();
    for required in [
        "margins-core",
        "margins-media",
        "margins-store",
        "default = []",
    ] {
        assert!(manifest.contains(required), "missing {required}");
    }
    for forbidden in [
        "margins =",
        "margins-desktop",
        "margins-capture-native",
        "tauri",
        "cpal",
        "cidre",
        "windows =",
        "objc2",
        "pi_agent_rust",
        "desktop/",
        "crates/private",
    ] {
        assert!(
            !manifest.contains(forbidden),
            "forbidden manifest edge: {forbidden}"
        );
    }
}

#[test]
fn public_sources_and_resources_do_not_reach_into_desktop() {
    fn rust_sources(dir: &std::path::Path, out: &mut String) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                rust_sources(&path, out);
            } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
                out.push_str(&std::fs::read_to_string(path).unwrap());
                out.push('\n');
            }
        }
    }
    let mut source = String::new();
    rust_sources(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut source,
    );
    let source = source.to_ascii_lowercase();
    for forbidden in ["desktop/", "src-tauri", "cpal::", "cidre::", "tauri::"] {
        assert!(
            !source.contains(forbidden),
            "public source contains {forbidden}"
        );
    }
}
