use std::path::Path;

use serde_json::{json, Value};

mod common;

#[test]
fn window_api_preserves_global_and_peer_contracts() {
  let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/checks/window-api.mjs");
  let observed: Value =
    serde_json::from_str(&common::node(fixture, &[])).expect("window API observations");
  let labels = [
    "main", "main", "main", "focused", "main", "focused", "main", "focused",
  ];
  let factories: Vec<_> = labels
    .iter()
    .map(|label| {
      json!({
        "label": label,
        "libraryInstance": true,
        "upstreamInstance": true,
        "options": { "skip": true },
      })
    })
    .collect();
  let expected = json!({
    "effectIdentity": true,
    "effectStateIdentity": true,
    "factories": factories,
    "missing": null,
    "focused": "focused",
    "inherited": "inherited-minimize",
    "calls": [
      {
        "command": "plugin:electron-window|set_effects",
        "args": { "label": "main", "value": {
          "effects": ["acrylic"], "state": "active", "radius": 12, "color": [10, 20, 30, 40],
        } },
      },
      { "command": "plugin:electron-window|set_effects", "args": { "label": "main", "value": null } },
      { "command": "plugin:electron-window|set_effects", "args": { "label": "main", "value": { "effects": ["blur"] } } },
    ],
    "rejected": "unsupported effect: blur",
    "upstreamMethodUnchanged": true,
  });
  for mode in ["global", "peer"] {
    assert_eq!(observed[mode], expected, "{mode} window API contract");
  }
}
