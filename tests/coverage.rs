use std::{fs, path::Path};

mod common;

#[test]
fn generated_coverage_matches_committed_document() {
  let root = Path::new(env!("CARGO_MANIFEST_DIR"));
  let generated = common::node(root.join("tests/checks/coverage.mjs"), &["--md"]);
  let committed =
    fs::read_to_string(root.join("docs/coverage.md")).expect("read coverage document");
  assert_eq!(
    committed.replace("\r\n", "\n"),
    generated,
    "docs/coverage.md is stale; run: node tests/checks/coverage.mjs --write",
  );
}
