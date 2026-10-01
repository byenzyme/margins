use std::path::PathBuf;

fn main() {
    let plist = PathBuf::from("src/cli/MarginsNativeBridge-Info.plist");
    println!("cargo:rerun-if-changed={}", plist.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos")
        && std::env::var_os("CARGO_FEATURE_AUDIO_CAPTURE").is_some()
    {
        let absolute = std::env::current_dir()
            .expect("package directory")
            .join(plist);
        assert!(
            absolute.is_file(),
            "native bridge permission plist is missing"
        );
        println!(
            "cargo:rustc-link-arg-bin=margins-private=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            absolute.display()
        );
    }
}
