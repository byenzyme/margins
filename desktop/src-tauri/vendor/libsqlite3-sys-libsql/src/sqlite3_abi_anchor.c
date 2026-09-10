/*
 * Intentionally empty SQLite archive for sqlmodel-sqlite's hard-coded
 * `#[link(name = "sqlite3", kind = "static")]`. SQLite's actual C symbols
 * are supplied once by libsql-ffi.
 */
void margins_libsql_sqlite_abi_anchor(void) {}
