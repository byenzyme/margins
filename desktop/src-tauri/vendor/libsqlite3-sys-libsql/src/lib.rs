//! Compatibility surface for consumers expecting `libsqlite3-sys`.
//!
//! libSQL exports SQLite's C ABI. Re-exporting its generated bindings lets
//! adapters use that ABI without compiling and linking a second SQLite copy.

pub use libsql_ffi::*;

use std::ffi::{c_char, c_int};

type AutoExtensionEntry = unsafe extern "C" fn(
    *mut sqlite3,
    *mut *mut c_char,
    *const sqlite3_api_routines,
) -> c_int;

/// Adapt rusqlite's upstream callback signature to libSQL's const-corrected
/// generated binding. SQLite owns the callback invocation and ABI; only the
/// pointee mutability in the Rust type differs.
pub unsafe fn sqlite3_auto_extension(entry: Option<AutoExtensionEntry>) -> c_int {
    let entry = std::mem::transmute(entry);
    libsql_ffi::sqlite3_auto_extension(entry)
}

/// See [`sqlite3_auto_extension`].
pub unsafe fn sqlite3_cancel_auto_extension(entry: Option<AutoExtensionEntry>) -> c_int {
    let entry = std::mem::transmute(entry);
    libsql_ffi::sqlite3_cancel_auto_extension(entry)
}
