use std::path::{Component, Path};

#[test]
fn manifest_has_no_root_private_or_native_dependency_edge() {
    let manifest = include_str!("../Cargo.toml");
    for forbidden in [
        "margins =",
        "margins-capture-native",
        "margins-desktop",
        "cpal",
        "cidre",
        "tauri",
        "windows",
        "objc2",
    ] {
        assert!(
            !manifest.contains(forbidden),
            "forbidden manifest edge: {forbidden}"
        );
    }
    for line in manifest
        .lines()
        .filter(|line| line.contains("path = \"../"))
    {
        let path = line
            .split("path = \"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let components = Path::new(path).components().collect::<Vec<_>>();
        assert_eq!(components.first(), Some(&Component::ParentDir));
        assert!(!components
            .iter()
            .skip(1)
            .any(|component| *component == Component::ParentDir));
    }
}

#[test]
fn parser_and_dispatch_are_exported_with_the_library() {
    let library = include_str!("../src/lib.rs");
    let arguments = include_str!("../src/args.rs");
    assert!(library.contains("pub fn run<I, T>"));
    assert!(arguments.contains("pub enum Command"));
}
