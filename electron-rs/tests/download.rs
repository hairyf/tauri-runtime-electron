mod support;

use serde_json::{json, Value};
use std::path::Path;

fn assert_cleanup_options(observed: &Value, count: usize) {
    let options = observed["cleanupOptions"].as_array().unwrap();
    assert_eq!(options.len(), count);
    for option in options {
        assert_eq!(*option, json!({ "recursive": true, "force": true }));
    }
}

#[test]
fn completion_and_first_host_decision_join_in_either_order() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/download.mjs"),
        &["ordering"],
    ))
    .unwrap();

    assert_eq!(observed["distinctStages"], true);
    assert_eq!(observed["suggestedDestination"], "suggested.bin");
    assert_eq!(observed["stagingExposed"], false);
    assert_eq!(observed["finishedBeforeDecision"], 0);
    assert_eq!(
        observed["lateResult"],
        json!({
            "webviewId": 70, "url": "https://download.invalid/file", "path": "late.bin", "success": true
        })
    );
    assert_eq!(observed["lateBytes"], "download bytes");
    assert_eq!(observed["lateClean"], true);
    assert_eq!(observed["finishedAfterDuplicate"], 1);
    assert_eq!(
        observed["timersAfterAccept"], 0,
        "an accepted long download must not retain its decision timeout"
    );
    assert_eq!(observed["cancelledAfterDuplicate"], false);
    assert_eq!(observed["finishedBeforeDone"], 1);
    assert_eq!(
        observed["earlyResult"],
        json!({
            "webviewId": 70, "url": "https://download.invalid/file", "path": "early.bin", "success": true
        })
    );
    assert_eq!(observed["earlyBytes"], "download bytes");
    assert_eq!(observed["earlyClean"], true);
    assert_eq!(observed["ignoredExists"], false);
    assert_eq!(observed["finishedCount"], 2);
    assert_cleanup_options(&observed, 2);
    assert_eq!(
        observed["copyFlags"],
        json!([1, 1]),
        "publication must use COPYFILE_EXCL"
    );
    assert_eq!(
        observed["renames"], 0,
        "publication must work across volumes without rename"
    );
    assert_eq!(observed["savePathDuringCallback"], json!([true, true]));
    assert_eq!(observed["timerDelays"], json!([30_000, 30_000]));
    assert_eq!(observed["pending"], 0);
    assert_eq!(observed["timers"], 0);
}

#[test]
fn denial_and_unsuccessful_done_clean_staging_without_duplicate_results() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/download.mjs"),
        &["denial-and-failure"],
    ))
    .unwrap();

    let failure = json!({ "webviewId": 70, "url": "https://download.invalid/file", "path": null, "success": false });
    assert_eq!(
        observed["denied"],
        json!([
            { "result": failure, "clean": true, "cancelled": true },
            { "result": failure, "clean": true, "cancelled": null }
        ])
    );
    assert_eq!(
        observed["failures"],
        json!([
            { "state": "interrupted", "pendingBeforeDecision": true, "finishedDelta": 1, "result": failure, "clean": true },
            { "state": "cancelled", "pendingBeforeDecision": true, "finishedDelta": 1, "result": failure, "clean": true }
        ])
    );
    assert_eq!(observed["finishedCount"], 4);
    assert_cleanup_options(&observed, 4);
    assert_eq!(observed["copyFlags"], json!([]));
    assert_eq!(
        observed["savePathDuringCallback"],
        json!([true, true, true, true])
    );
    assert_eq!(
        observed["timerDelays"],
        json!([30_000, 30_000, 30_000, 30_000])
    );
    assert_eq!(observed["pending"], 0);
    assert_eq!(observed["timers"], 0);
}

#[test]
fn exclusive_publication_preserves_existing_data_and_recoverable_staged_bytes() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/download.mjs"),
        &["publication-errors"],
    ))
    .unwrap();

    let failure = json!({ "webviewId": 70, "url": "https://download.invalid/file", "path": null, "success": false });
    assert_eq!(
        observed["collision"],
        json!({
            "requestedDestination": "existing.bin", "result": failure,
            "originalBytes": "original bytes", "stagedBytes": "download bytes", "loggedStage": true
        })
    );
    assert_eq!(
        observed["failed"],
        json!({
            "result": failure, "stagedBytes": "download bytes", "destinationExists": false
        })
    );
    assert_eq!(observed["finishedCount"], 2);
    assert_cleanup_options(&observed, 0);
    assert_eq!(
        observed["copyFlags"],
        json!([1, 1]),
        "COPYFILE_EXCL applies on collision and I/O failure too"
    );
    assert_eq!(observed["renames"], 0);
    assert_eq!(observed["savePathDuringCallback"], json!([true, true]));
    assert_eq!(observed["timerDelays"], json!([30_000, 30_000]));
    assert_eq!(observed["pending"], 0);
    assert_eq!(observed["timers"], 0);
}

#[test]
fn empty_non_string_and_nul_destinations_are_rejected_and_cleaned() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/download.mjs"),
        &["invalid-destinations"],
    ))
    .unwrap();

    let failure = json!({ "webviewId": 70, "url": "https://download.invalid/file", "path": null, "success": false });
    assert_eq!(
        observed["invalid"],
        json!([
            { "destination": "", "result": failure, "clean": true },
            { "destination": 42, "result": failure, "clean": true },
            { "destination": "bad\0path", "result": failure, "clean": true }
        ])
    );
    assert_eq!(observed["finishedCount"], 3);
    assert_cleanup_options(&observed, 3);
    assert_eq!(observed["copyFlags"], json!([]));
    assert_eq!(
        observed["savePathDuringCallback"],
        json!([true, true, true])
    );
    assert_eq!(observed["timerDelays"], json!([30_000, 30_000, 30_000]));
    assert_eq!(observed["pending"], 0);
    assert_eq!(observed["timers"], 0);
}

#[test]
fn webview_destruction_cleans_pending_completed_and_late_cancelled_downloads() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/download.mjs"),
        &["destruction"],
    ))
    .unwrap();

    let failure = json!({ "webviewId": 70, "url": "https://download.invalid/file", "path": null, "success": false });
    assert_eq!(
        observed["destroyed"],
        json!([
            { "result": failure, "clean": true }, { "result": failure, "clean": true }
        ])
    );
    assert_eq!(observed["cleanBeforeLateDone"], true);
    assert_eq!(observed["cleanAfterLateDone"], true);
    assert_eq!(observed["lateFinishedDelta"], 0);
    assert_eq!(observed["finishedCount"], 3);
    assert_cleanup_options(&observed, 4);
    assert_eq!(observed["copyFlags"], json!([]));
    assert_eq!(
        observed["savePathDuringCallback"],
        json!([true, true, true])
    );
    assert_eq!(observed["timerDelays"], json!([30_000, 30_000, 30_000]));
    assert_eq!(observed["pending"], 0);
    assert_eq!(observed["timers"], 0);
}

#[test]
fn host_timeout_cleans_completed_download_and_ignores_late_acceptance() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/download.mjs"),
        &["timeout"],
    ))
    .unwrap();

    assert_eq!(observed["pendingBeforeTimeout"], 1);
    assert_eq!(observed["finishedBeforeTimeout"], 0);
    assert_eq!(observed["timerPresent"], true);
    assert_eq!(
        observed["result"],
        json!({
            "webviewId": 70, "url": "https://download.invalid/file", "path": null, "success": false
        })
    );
    assert_eq!(observed["clean"], true);
    assert_eq!(observed["lateDestinationExists"], false);
    assert_eq!(observed["finishedCount"], 1);
    assert_cleanup_options(&observed, 1);
    assert_eq!(observed["copyFlags"], json!([]));
    assert_eq!(observed["savePathDuringCallback"], json!([true]));
    assert_eq!(observed["timerDelays"], json!([30_000]));
    assert_eq!(observed["pending"], 0);
    assert_eq!(observed["timers"], 0);
}

#[test]
fn staging_failure_prevents_native_download_and_leaves_no_pending_request() {
    let observed: Value = serde_json::from_str(&support::node(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/download.mjs"),
        &["staging-error"],
    ))
    .unwrap();

    assert_eq!(observed["prevented"], true);
    assert_eq!(
        observed["result"],
        json!({
            "webviewId": 70, "url": "https://download.invalid/file", "path": null, "success": false
        })
    );
    assert_eq!(observed["requestedCount"], 0);
    assert_eq!(observed["finishedCount"], 1);
    assert_cleanup_options(&observed, 0);
    assert_eq!(observed["copyFlags"], json!([]));
    assert_eq!(observed["savePathDuringCallback"], json!([]));
    assert_eq!(observed["timerDelays"], json!([]));
    assert_eq!(observed["pending"], 0);
    assert_eq!(observed["timers"], 0);
}
