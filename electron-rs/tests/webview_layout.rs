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
fn display_containment_uses_physical_windows_points_and_logical_unix_points() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["monitors"],
    ))
    .unwrap();

    let contained = json!(["primary", "primary", "secondary", "secondary", null, null, null, null]);
    assert_eq!(observed["physical"], contained);
    assert_eq!(observed["logical"]["darwin"], contained);
    assert_eq!(observed["logical"]["linux"], contained);
    assert_eq!(observed["secondary"]["name"], "secondary");
    assert_eq!(observed["secondary"]["position"]["x"], 1920);
    assert_eq!(observed["secondary"]["size"], json!({ "width": 2560, "height": 1440 }));
    assert_eq!(observed["invalid"], "invalid monitor point");
}

#[test]
fn native_button_getters_ignore_request_mirrors_and_each_setter_calls_native_once() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["native-state"],
    ))
    .unwrap();

    let disabled = json!({ "maximizable": false, "minimizable": false, "closable": false });
    assert_eq!(observed["contrary"], disabled);
    assert_eq!(observed["afterNativeChange"], disabled);
    let requests = [false, true, false, true];
    let expected: Vec<_> = requests.iter().map(|value| json!({
        "maximizable": value, "minimizable": value, "closable": value,
    })).collect();
    assert_eq!(observed["states"], json!(expected));
    let calls: Vec<_> = requests.into_iter().flat_map(|value| [
        json!({ "method": "setClosable", "value": value }),
        json!({ "method": "setMaximizable", "value": value }),
        json!({ "method": "setMinimizable", "value": value }),
    ]).collect();
    assert_eq!(observed["calls"], json!(calls));
}

#[test]
fn four_native_children_keep_url_ownership_and_resize_without_gaps_or_drift() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["layout"],
    ))
    .unwrap();

    assert_eq!(observed["hostUrl"], Value::Null);
    assert_eq!(observed["hostHasWebContents"], false);
    assert_eq!(observed["contentsCount"], 4, "BaseWindow must not create an unused renderer");
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
                "nodeIntegrationInSubFrames": true, "webviewTag": false, "preload": "preload.cjs",
                "javascript": true, "devTools": true, "backgroundThrottling": true
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
        assert_eq!(observation["host"], json!([]));
        assert_eq!(
            observation["children"],
            json!(vec![
                json!([{ "css": format!("* {{ cursor: {cursor} !important }}"), "cssOrigin": "user" }]);
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
    assert_eq!(observed["script"], "main-script");
    assert_eq!(
        observed["cursor"],
        json!([{ "css": "* { cursor: crosshair !important }", "cssOrigin": "user" }])
    );
    assert_eq!(
        observed["unrelatedCursor"],
        json!([{ "css": "* { cursor: default !important }", "cssOrigin": "user" }])
    );
    assert_eq!(observed["visible"], false);
    assert_eq!(observed["hostVisible"], true, "hiding the main view must not hide its BaseWindow");
    assert_eq!(observed["closeRequested"], false, "closing a view must not close its host window");
    assert_eq!(observed["contentsDestroyed"], true);
    assert_eq!(observed["mainMapped"], false);
    assert_eq!(observed["hostAlive"], true);
    assert_eq!(observed["hostHasWebContents"], false);
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
fn native_client_background_preserves_alpha_and_independent_webview_colors() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["backgrounds"],
    ))
    .unwrap();
    assert_eq!(observed, json!({ "independent": true, "opaque": true }));
}

#[test]
fn creation_uses_nested_config_and_applies_webview_preferences_before_navigation() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["configuration"],
    ))
    .unwrap();

    assert_eq!(
        observed["nested"],
        json!({
            "title": "Nested window", "width": 640, "height": 480,
            "useContentSize": true, "acceptFirstMouse": true,
            "backgroundColor": "rgba(10, 20, 30, 0.502)", "titleBarStyle": "hiddenInset"
        })
    );
    for (target, agent, url) in [
        ("main", "main-agent", "tauri://localhost/main"),
        ("child", "child-agent", "tauri://localhost/child"),
        ("fallback", "fallback-agent", "tauri://localhost/fallback"),
    ] {
        assert_eq!(
            observed[target]["preferences"],
            json!({ "javascript": false, "devTools": false, "backgroundThrottling": false, "userAgent": null, "webviewTag": false }),
            "preferences for {target}"
        );
        assert_eq!(
            observed[target]["calls"],
            json!([
                { "method": "setUserAgent", "value": agent },
                { "method": "loadURL", "value": url }
            ]),
            "user agent must be set before the first load for {target}"
        );
    }
    assert_eq!(
        observed["defaults"],
        json!({ "javascript": true, "devTools": true, "backgroundThrottling": true, "userAgent": null, "webviewTag": false })
    );
    assert_eq!(
        observed["flat"],
        json!({ "title": "Flat window", "width": 360, "height": 240,
            "acceptFirstMouse": true, "backgroundColor": "#123456", "titleBarStyle": "hidden" })
    );
    assert_eq!(
        observed["titleStyles"],
        json!(["hidden", "hidden", "default"])
    );
}

#[test]
fn content_constraints_keep_each_axis_and_reapply_frame_and_physical_units() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["constraints"],
    ))
    .unwrap();

    assert_eq!(
        observed["creation"],
        json!({ "min": [240, 180], "max": [1000, 800] })
    );
    assert_eq!(
        observed["framed"],
        json!({ "min": [256, 218], "max": [1016, 838] })
    );
    assert_eq!(observed["clamped"], json!([240, 800]));
    assert_eq!(
        observed["heightOnlyMin"],
        json!({ "min": [0, 338], "max": [1016, 838] })
    );
    assert_eq!(
        observed["heightOnlyMax"],
        json!({ "min": [0, 338], "max": [0, 488] })
    );
    let framed = json!({ "min": [316, 238], "max": [516, 438] });
    assert_eq!(observed["atomic"], framed);
    assert_eq!(
        observed["nativeConstraints"],
        json!({ "minWidth": 300, "minHeight": 200, "maxWidth": 500, "maxHeight": 400 })
    );
    assert_eq!(observed["physicalClamped"], json!([500, 200]));
    assert_eq!(
        observed["menu"],
        json!({ "min": [316, 258], "max": [516, 458] })
    );
    assert_eq!(observed["menuRemoved"], framed);
    assert_eq!(
        observed["undecorated"],
        json!({ "min": [300, 200], "max": [500, 400] })
    );
    assert_eq!(observed["decorated"], framed);
    assert_eq!(
        observed["dpi"],
        json!({ "min": [312, 350], "max": [812, 670] })
    );
    assert_eq!(
        observed["nativeDpi"],
        json!({ "scaleFactor": 1.25, "constraints": {
            "minWidth": 300, "minHeight": 320, "maxWidth": 800, "maxHeight": 640
        } })
    );
    assert_eq!(
        observed["nativeCleared"],
        json!({ "minWidth": 0, "minHeight": 0, "maxWidth": 0, "maxHeight": 0 })
    );
    assert_eq!(observed["cleared"], json!({ "min": [0, 0], "max": [0, 0] }));
    assert_eq!(
        observed["restoredHeightOnly"],
        json!({ "min": [0, 150], "max": [0, 330] })
    );
    assert_eq!(observed["legacyCleared"], observed["cleared"]);
}

#[test]
fn cursor_visibility_preserves_the_icon_across_navigation_and_new_children() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["cursors"],
    ))
    .unwrap();

    let crosshair = json!([{ "css": "* { cursor: crosshair !important }", "cssOrigin": "user" }]);
    let hidden = json!([{ "css": "* { cursor: none !important }", "cssOrigin": "user" }]);
    let pointer = json!([{ "css": "* { cursor: pointer !important }", "cssOrigin": "user" }]);
    let wait = json!([{ "css": "* { cursor: wait !important }", "cssOrigin": "user" }]);
    assert_eq!(observed["chosen"], json!(vec![crosshair; 4]));
    assert_eq!(observed["hidden"], json!(vec![&hidden; 4]));
    assert_eq!(observed["chosenWhileHidden"], observed["hidden"]);
    assert_eq!(observed["inheritedHidden"], hidden);
    assert_eq!(observed["visible"], json!(vec![&pointer; 5]));
    assert_eq!(observed["restored"], pointer);
    assert_eq!(observed["inheritedVisible"], pointer);
    assert_eq!(observed["legacy"], json!(vec![wait; 6]));
    assert_eq!(observed["host"], json!([]));
    assert_eq!(observed["other"], json!([]));
    assert!(observed["removed"].as_u64().unwrap() >= 3);
}

#[test]
fn parent_and_recursive_menu_dispatch_validate_ownership_and_ignore_closed_windows() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["parent-menu"],
    ))
    .unwrap();

    assert_eq!(observed["attached"], true);
    assert_eq!(observed["detached"], true);
    assert_eq!(observed["unknown"], "unknown window 999");
    assert!(observed["self"]
        .as_str()
        .is_some_and(|error| !error.is_empty()));
    assert_eq!(observed["parentCalls"], json!(["Parent", null]));
    let file = &observed["template"][0];
    assert_eq!(file["label"], "File");
    assert_eq!(file["enabled"], true);
    assert_eq!(
        file["submenu"][0],
        json!({ "label": "Open", "enabled": true, "accelerator": "CmdOrCtrl+O" })
    );
    let tools = &file["submenu"][1];
    assert_eq!(tools["label"], "Tools");
    assert_eq!(tools["enabled"], false);
    assert_eq!(
        tools["submenu"][0],
        json!({ "label": "Pinned", "enabled": true, "type": "checkbox", "checked": true })
    );
    assert_eq!(tools["submenu"][1], json!({ "type": "separator" }));
    assert_eq!(tools["submenu"][2]["label"], "Disabled");
    assert_eq!(tools["submenu"][2]["enabled"], false);
    assert_eq!(
        observed["clicks"],
        json!([
            { "event": "menu.command", "params": { "windowId": 51, "commandId": 71 } },
            { "event": "menu.command", "params": { "windowId": 51, "commandId": 72 } }
        ])
    );
    assert_eq!(
        observed["visibility"],
        json!({ "autoHide": false, "visible": true })
    );
    assert_eq!(observed["cleared"], true);
    assert_eq!(observed["afterClose"], json!([]));
}

#[test]
fn per_window_theme_emulates_owned_pages_and_cleans_native_listeners() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["themes"],
    ))
    .unwrap();

    let dark = json!({ "method": "Emulation.setEmulatedMedia",
        "params": { "features": [{ "name": "prefers-color-scheme", "value": "dark" }] } });
    let light = json!({ "method": "Emulation.setEmulatedMedia",
        "params": { "features": [{ "name": "prefers-color-scheme", "value": "light" }] } });
    assert_eq!(observed["dark"], json!({ "theme": "dark" }));
    assert_eq!(observed["light"], json!({ "theme": "light" }));
    assert_eq!(observed["children"], json!(vec![&dark; 4]));
    assert_eq!(observed["mainCommand"], light);
    assert_eq!(observed["pageQuery"], json!({ "theme": "dark" }));
    assert!(observed["pageQueries"].as_u64().unwrap() > 0);
    assert_eq!(observed["inherited"], dark);
    assert_eq!(observed["inheritedDark"], "dark");
    assert_eq!(
        observed["cleared"],
        json!(vec![
            json!({ "command": { "method": "Emulation.setEmulatedMedia",
            "params": { "features": [] } }, "attached": false });
            5
        ])
    );
    assert!(observed["invalid"]
        .as_str()
        .is_some_and(|error| !error.is_empty()));
    assert_eq!(observed["system"], json!({ "theme": "dark" }));
    assert_eq!(observed["stillLight"], json!({ "theme": "light" }));
    assert_eq!(observed["nativeSource"], "system");
    assert_eq!(observed["beforeListeners"], 2);
    assert_eq!(observed["afterFirstClose"], 1);
    assert_eq!(observed["afterClose"], 0);
    assert_eq!(observed["afterCloseEvents"], json!([]));
}

#[test]
fn rejected_popup_plans_destroy_shells_restore_nested_context_and_preserve_accepted_guests() {
  let observed: Value = serde_json::from_str(&support::node(
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
    &["popup-rejection"],
  ))
  .unwrap();

  let rejected = observed["rejected"].as_array().unwrap();
  let cases = [
    "deny",
    "invalid-action",
    "missing-window-id",
    "missing-destination",
    "null-response",
    "exception",
  ];
  assert_eq!(rejected.len(), cases.len());
  for (index, name) in cases.into_iter().enumerate() {
    assert_eq!(
      rejected[index],
      json!({
          "name": name, "shellDestroyed": true, "destroys": 1, "restoredOnClose": true,
          "pending": false, "windowMapped": false, "webviewMapped": false, "contentsCreated": 0,
          "events": [{ "event": "window.closed", "params": { "windowId": 80 + index } }],
      })
    );
  }
  assert_eq!(
    observed["accepted"],
    json!({
        "shellDestroyed": false, "guestDestroyed": false, "windowMapped": true, "guestMapped": true,
        "pending": false, "outlivesOpener": true, "navigated": false,
    })
  );
}

#[test]
fn native_policy_covers_preferences_popup_cleanup_explicit_close_and_disconnect() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["native-policy"],
    ))
    .unwrap();

    assert_eq!(
        observed["roots"],
        json!({
            "mainOptions": false, "mainView": false, "child": false,
            "popupOptions": false, "popupView": false,
        })
    );
    assert_eq!(
        observed["nativeDestroy"],
        json!({
            "guestDestroyed": true, "viewContentsCleared": true, "shellDestroyed": true,
            "windowMapped": false, "childMapped": false, "webviewMapped": false, "mainMapped": false,
            "events": [
                { "event": "webview.closed", "params": { "webviewId": 30 } },
                { "event": "window.closed", "params": { "windowId": 3 } },
            ],
        })
    );
    assert_eq!(
        observed["explicitClose"],
        json!({
            "guestDestroyed": true, "viewContentsCleared": true, "shellDestroyed": false,
            "windowMapped": true, "childMapped": false, "webviewMapped": false, "mainMapped": false,
            "events": [{ "event": "webview.closed", "params": { "webviewId": 31 } }],
        })
    );
    assert_eq!(
        observed["noSocketClose"],
        json!({ "prevented": false, "destroyed": true, "events": [] })
    );
    assert_eq!(
        observed["closedSocketClose"],
        json!({ "prevented": false, "destroyed": true, "events": [] })
    );
}

#[test]
fn linux_native_button_state_is_not_a_requested_flag_mirror_and_close_policy_still_vetoes() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["linux-state"],
    ))
    .unwrap();

    let disabled = json!({ "maximizable": false, "minimizable": false, "closable": false });
    assert_eq!(observed["initial"], disabled);
    assert_eq!(
        observed["vetoed"],
        json!({ "prevented": true, "events": [] })
    );
    assert_eq!(
        observed["enabled"],
        json!({ "maximizable": false, "minimizable": false, "closable": true })
    );
    assert_eq!(
        observed["requested"],
        json!({ "prevented": true, "events": [
            { "event": "window.closeRequested", "params": { "windowId": 60 } }
        ] })
    );
    assert_eq!(observed["disabled"], disabled);
    assert_eq!(observed["vetoedAgain"], observed["vetoed"]);
    assert_eq!(observed["destroyed"], true);
    assert_eq!(observed["disconnected"], json!({ "prevented": false, "destroyed": true }));
    assert_eq!(
        observed["closedEvents"],
        json!([{ "event": "window.closed", "params": { "windowId": 60 } }])
    );
}

#[test]
fn linux_focusable_is_applied_before_show_without_an_irreversible_constructor_flag() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["linux-focus"],
    ))
    .unwrap();
    assert_eq!(observed, json!({
        "configured": { "constructor": true, "setterCalls": [{ "method": "setFocusable", "value": false }], "focused": false },
        "blocked": false, "setterDidNotFocus": true, "enabled": true, "disabled": false,
    }));
}

#[test]
fn macos_center_uses_current_work_area_and_focus_activates_app_before_window() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["macos-center-focus"],
    ))
    .unwrap();
    assert_eq!(observed, json!({
        "builder": { "x": 2240, "y": 140, "width": 640, "height": 400, "scale": 2 },
        "centered": { "x": 2320, "y": 170, "width": 480, "height": 340, "scale": 2 },
        "focus": [{ "app": { "steal": true } }, "window"], "nativeCenters": 1,
        "fullscreen": false, "fullscreenable": true,
        "trafficLights": [{ "x": 42, "y": 34 }, { "x": 26, "y": 22 }],
    }));
}

#[test]
fn navigation_acknowledges_before_load_and_native_fullscreen_waits_for_real_events() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-layout.mjs"),
        &["lifecycle"],
    ))
    .unwrap();

    assert_eq!(
        observed["initialBounds"],
        json!({ "x": 0, "y": 0, "width": 800, "height": 600 })
    );
    assert_eq!(observed["resizedBounds"], observed["initialBounds"]);
    assert_eq!(
        observed["navigation"],
        json!({ "acknowledged": true, "loaded": false,
            "url": "tauri://localhost/next", "hostUrl": null })
    );
    assert_eq!(
        observed["beforeEnter"],
        json!({ "acknowledged": false, "fullscreen": false, "simpleCalls": [] })
    );
    assert_eq!(observed["beforeLeave"], json!({ "acknowledged": false, "fullscreen": true }));
    assert_eq!(observed["entered"], true);
    assert_eq!(observed["exited"], false);
    assert_eq!(observed["nativeFullscreen"], true);
    assert_eq!(observed["nativeCalls"], json!([true, false, true, true, true]));
    assert_eq!(observed["simpleCalls"], json!([]), "failed native fullscreen must not change fullscreen modes");
    assert_eq!(observed["timeoutDelay"], 10000);
    assert_eq!(observed["timeoutError"], "native fullscreen transition timed out");
    assert_eq!(observed["closedError"], "window closed during fullscreen transition");
    assert_eq!(observed["transitionListeners"], json!([0, 0]));
    assert_eq!(observed["timers"], 0);
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
