fn main() {
    // sqlmodel-sqlite 0.2.2 names a static `sqlite3` archive directly even
    // though its symbols are satisfied by libsql-ffi. Provide an empty archive
    // under that expected name; the single real SQLite implementation remains
    // libsql-ffi.
    cc::Build::new().file("src/sqlite3_abi_anchor.c").compile("sqlite3");
}
