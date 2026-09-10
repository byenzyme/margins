#![cfg(feature = "recall")]

#[path = "support/sqlite_runtime_product.rs"]
mod sqlite_runtime_product;

#[test]
fn initialize_then_recall_first_then_store() {
    let root = tempfile::tempdir().unwrap();
    margins::initialize_sqlite_runtime().unwrap();
    sqlite_runtime_product::exercise_recall(root.path());
    sqlite_runtime_product::exercise_store(root.path(), "recall-first");
}
