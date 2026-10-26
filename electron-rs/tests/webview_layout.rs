mod support;

use serde_json::{json, Value};
use std::{collections::HashSet, path::Path};

#[test]
fn geometry_uses_native_mixed_dpi_screen_coordinates() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["geometry"],
    ))
    .unwrap();

    assert_eq!(observed["useContentSize"], true);
    assert_eq!(observed["cursor"], json!({ "x": 2280, "y": 80 }));
    assert_eq!(
        observed["nativePosition"],
        json!({ "x": 2100, "y": 40, "width": 800, "height": 600 })
    );
    assert_eq!(
        observed["geometry"]["bounds"],
        json!({ "x": 2280, "y": 80, "width": 1600, "height": 1200 })
    );
    assert_eq!(observed["monitor"]["position"]["x"], 1920);
    assert_eq!(observed["monitor"]["size"]["width"], 2560);
    assert_eq!(observed["monitor"]["workArea"]["size"]["height"], 1360);
}

#[test]
fn four_native_children_keep_url_ownership_and_resize_without_gaps_or_drift() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["layout"],
    ))
    .unwrap();

    assert_eq!(observed["hostUrl"], Value::Null);
    let children = observed["children"].as_array().unwrap();
    assert_eq!(children.len(), 4);
    let contents_ids: HashSet<_> = children
        .iter()
        .map(|child| child["contentsId"].as_u64().unwrap())
        .collect();
    assert_eq!(contents_ids.len(), 4);
    let urls = [
        "tauri://localhost/",
        "https://github.com/tauri-apps/tauri",
        "https://tauri.app",
        "https://twitter.com/TauriApps",
    ];
    let physical_bounds = json!([
        { "x": 0, "y": 0, "width": 800, "height": 600 },
        { "x": 800, "y": 0, "width": 800, "height": 600 },
        { "x": 0, "y": 600, "width": 800, "height": 600 },
        { "x": 800, "y": 600, "width": 800, "height": 600 }
    ]);
    for (index, child) in children.iter().enumerate() {
        assert_eq!(child["url"], urls[index]);
        assert_eq!(child["owner"], [10, 11, 12, 13][index]);
        assert_eq!(child["bounds"], physical_bounds[index]);
        assert_eq!(
            child["preferences"],
            json!({
                "contextIsolation": true, "sandbox": true, "nodeIntegration": false,
                "nodeIntegrationInSubFrames": true, "preload": "preload.cjs"
            })
        );
    }
    assert_eq!(
        observed["oddBounds"],
        json!([
            { "x": 0, "y": 0, "width": 501, "height": 351 },
            { "x": 501, "y": 0, "width": 500, "height": 351 },
            { "x": 0, "y": 351, "width": 501, "height": 350 },
            { "x": 501, "y": 351, "width": 500, "height": 350 }
        ])
    );
    assert_eq!(
        observed["resetBounds"],
        json!([
            { "x": 0, "y": 0, "width": 400, "height": 300 },
            { "x": 400, "y": 0, "width": 400, "height": 300 },
            { "x": 0, "y": 300, "width": 400, "height": 300 },
            { "x": 400, "y": 300, "width": 400, "height": 300 }
        ])
    );
}

#[test]
fn partial_mixed_unit_bounds_validate_and_reset_resize_proportions() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["bounds"],
    ))
    .unwrap();

    assert_eq!(
        observed["steps"],
        json!([
            { "x": -30, "y": 20, "width": 200, "height": 100 },
            { "x": -30, "y": 20, "width": 310, "height": 190 },
            { "x": 60, "y": 40, "width": 310, "height": 190 },
            { "x": 100, "y": 50, "width": 200, "height": 100 }
        ])
    );
    assert_eq!(
        observed["physical"],
        json!({ "x": 200, "y": 100, "width": 400, "height": 200 })
    );
    assert_eq!(
        observed["errors"],
        json!([
            "invalid child webview bounds",
            "invalid child webview bounds"
        ])
    );
    assert_eq!(
        observed["manualResize"],
        json!({ "x": 200, "y": 100, "width": 400, "height": 200 })
    );
    assert_eq!(observed["disabledResize"], observed["manualResize"]);
    assert_eq!(
        observed["enabledResize"],
        json!({ "x": 400, "y": 200, "width": 800, "height": 400 })
    );
}

#[test]
fn child_dispatch_and_close_never_target_the_unloaded_host() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["dispatch"],
    ))
    .unwrap();

    assert_eq!(
        observed["evalScripts"],
        json!({ "child": "child-script", "host": null })
    );
    let cursors = observed["cursors"].as_array().unwrap();
    assert_eq!(cursors.len(), 3);
    for (observation, cursor) in cursors.iter().zip(["pointer", "none", "default"]) {
        assert_eq!(observation["host"], Value::Null);
        assert_eq!(
            observation["children"],
            json!(vec![
                format!(
                    "document.documentElement.style.cursor = {cursor:?}"
                );
                4
            ])
        );
    }
    assert_eq!(observed["hidden"], json!({ "child": false, "host": true }));
    assert_eq!(
        observed["child"],
        json!({ "reloaded": true, "zoom": 1.25, "focused": true, "visible": true, "color": "#123456" })
    );
    assert_eq!(
        observed["closed"],
        json!({ "destroyed": true, "children": 3, "mapped": false, "error": "unknown webview 10" })
    );
    assert_eq!(observed["remainingDestroyed"], json!([true, true, true]));
    assert_eq!(observed["childWebviews"], 0);
    assert_eq!(observed["webviews"], 0);
    assert_eq!(observed["policies"], 0);
}

#[test]
fn devtools_open_reuses_pending_operation_until_the_native_ready_event() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["devtools"],
    ))
    .unwrap();

    assert_eq!(
        observed["before"],
        json!({ "samePending": true, "visible": true, "opened": false, "openCalls": 1 })
    );
    assert_eq!(
        observed["after"],
        json!({ "opened": true, "openCalls": 1, "listeners": [0, 0, 0] })
    );
    assert_eq!(
        observed["destroyedError"],
        "webview destroyed while opening DevTools"
    );
    assert_eq!(observed["destroyedPending"], false);
    assert_eq!(observed["timers"], 0);
}

#[test]
fn rpc_starts_native_calls_synchronously_but_drains_socket_frames_first() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["rpc"],
    ))
    .unwrap();

    assert_eq!(
        observed["closedSynchronously"], false,
        "DevTools must already be closed before the RPC reaches a microtask"
    );
    assert_eq!(
        observed["directReplies"],
        json!([
            { "id": 1, "result": null },
            { "id": 2, "result": false },
            { "id": 3, "error": "unknown method missing" },
            { "id": 4, "error": "eval rejected" },
            { "id": 5, "result": observed["contentsId"] }
        ])
    );
    assert_eq!(observed["invalidReplies"], json!([]));
    assert_eq!(
        observed["beforeTasks"],
        json!({ "closeCalls": 0, "replies": 0 })
    );
    assert_eq!(observed["closeCalls"], 1);
    assert_eq!(observed["closeDuringRead"], json!([false]));
    assert_eq!(
        observed["framedReplies"],
        json!([
            { "id": 6, "result": null }, { "id": 7, "result": false }, { "id": 8, "result": false }
        ])
    );
}

#[test]
fn main_webview_dispatch_survives_failed_child_attach_and_stays_window_local() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["main-window"],
    ))
    .unwrap();

    assert_eq!(observed["attachError"], "attach failed");
    assert_eq!(observed["failedContentsDestroyed"], true);
    assert_eq!(observed["failedChildMapped"], false);
    assert_eq!(observed["children"], 0);
    assert_eq!(observed["url"], "tauri://localhost/main");
    assert_eq!(observed["evalResult"], observed["contentsId"]);
    assert!(observed["contentsId"].as_u64().is_some());
    assert_eq!(
        observed["script"],
        "document.documentElement.style.cursor = \"crosshair\""
    );
    assert_eq!(
        observed["unrelatedScript"],
        "document.documentElement.style.cursor = \"default\""
    );
    assert_eq!(observed["visible"], false);
    assert_eq!(observed["closeRequested"], true);
}

#[test]
fn clearing_background_effects_restores_transparency_and_updated_color() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["effects"],
    ))
    .unwrap();

    assert_eq!(observed["initialClear"], "#00000000");
    assert_eq!(
        observed["materials"],
        json!([
            { "material": "acrylic", "color": "#00000000" },
            { "material": "mica", "color": "#00000000" },
            { "material": "tabbed", "color": "#00000000" }
        ])
    );
    assert_eq!(observed["cleared"], "#00000000");
    assert_eq!(observed["changed"], "#123456");
    assert_eq!(observed["invalid"], "invalid background material");
    assert_eq!(
        observed["unsupported"],
        "Electron background materials require Windows 11 22H2 or later"
    );
}

#[test]
fn macos_restore_waits_for_native_completion_before_hide() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["restore"],
    ))
    .unwrap();

    assert_eq!(
        observed["beforeEvent"],
        json!({ "restored": false, "minimized": false })
    );
    assert_eq!(observed["restored"], true);
    assert_eq!(observed["restoreListeners"], 0);
    assert_eq!(observed["hidden"], false);
    assert_eq!(observed["shown"], true);
    assert_eq!(observed["timers"], 0);
}
