// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Makes unimplemented runtime APIs visible instead of silently succeeding.
//!
//! The reference runtime answers every `tauri-runtime` trait method, because CEF exposes a
//! hook for all of them. Electron does not, and a trait method that returns `Ok(())`
//! without doing anything is the worst possible answer: the caller believes the request
//! worked. Every such body calls [`report`] instead, which names the API once per process
//! and then stays quiet — a method called once per window must not repeat the warning for
//! every window.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// The APIs already reported, so each one is logged once and not once per call.
static REPORTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// Reports `api` the first time it is called, then stays quiet.
///
/// Call this before returning a successful default from something this runtime does not
/// implement, so a caller never thinks the request worked.
pub(crate) fn report(api: &str) {
  let fresh = {
    let mut reported = match REPORTED.get_or_init(Default::default).lock() {
      Ok(reported) => reported,
      // A previous caller panicked while holding the lock; the set is still usable.
      Err(poisoned) => poisoned.into_inner(),
    };
    if reported.contains(api) {
      false
    } else {
      reported.insert(api.to_owned());
      true
    }
  };

  // Outside the lock: a logger that itself calls back into this module must not deadlock.
  if fresh {
    log::warn!("[electron] unsupported, ignored: {api}");
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::{AtomicUsize, Ordering};

  /// Counts the lines this module writes, and ignores everything else the test binary logs.
  struct Counter;

  static COUNTER: Counter = Counter;
  static REPORTED_LINES: AtomicUsize = AtomicUsize::new(0);

  impl log::Log for Counter {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
      true
    }

    fn log(&self, record: &log::Record<'_>) {
      let message = record.args().to_string();
      if message.starts_with("[electron] unsupported, ignored: unsupported::tests::") {
        REPORTED_LINES.fetch_add(1, Ordering::SeqCst);
      }
    }

    fn flush(&self) {}
  }

  #[test]
  fn each_api_is_reported_once_and_the_message_names_it() {
    // A logger can only be installed once per process, so a run in which another test got
    // there first cannot assert anything.
    if log::set_logger(&COUNTER).is_err() {
      return;
    }
    log::set_max_level(log::LevelFilter::Warn);

    report("unsupported::tests::once");
    report("unsupported::tests::once");
    report("unsupported::tests::twice");

    assert_eq!(
      REPORTED_LINES.load(Ordering::SeqCst),
      2,
      "one line per distinct API, not one per call"
    );
  }
}
