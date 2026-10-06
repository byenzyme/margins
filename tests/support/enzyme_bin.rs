/// The `enzyme` binary Margins runs in these tests. Set `MARGINS_ENZYME_BIN`,
/// for example `MARGINS_ENZYME_BIN=$(scripts/cargo-lane disposable --
/// scripts/enzyme-bin)`; `scripts/local-gate linux` sets it.
#[allow(dead_code)]
pub fn enzyme_bin() -> std::path::PathBuf {
    let bin = std::env::var_os("MARGINS_ENZYME_BIN")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .expect("set MARGINS_ENZYME_BIN to the enzyme binary (see scripts/enzyme-bin)");
    assert!(bin.is_file(), "MARGINS_ENZYME_BIN is not a file: {}", bin.display());
    bin
}
