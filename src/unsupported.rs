// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Makes unimplemented runtime APIs visible instead of silently succeeding.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

static REPORTED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();

/// Reports `api` the first time it is called, then stays quiet.
///
/// Call this before returning a successful default from something this runtime
/// does not implement yet, so a caller never thinks the request worked.
pub(crate) fn report(api: &'static str) {
  let reported = REPORTED.get_or_init(|| Mutex::new(HashSet::new()));
  let fresh = match reported.lock() {
    Ok(mut reported) => reported.insert(api),
    Err(poisoned) => poisoned.into_inner().insert(api),
  };
  if fresh {
    eprintln!("[electron] unsupported, ignored: {api}");
  }
}
