// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The V8 heap limit the bundled runtime really enforces.
//!
//! Electron ships V8 with pointer compression, a shared cage and the V8 sandbox,
//! so every heap object has to live inside a 4 GB cage. A
//! `--max-old-space-size` above that is clamped instead of rejected, and the
//! process that asked for it is never told: a service started with 15000 MiB
//! runs with 4192 MiB while its caller keeps offering more.
//!
//! [`Electron::heap_size_limit`](crate::Electron::heap_size_limit) asks the
//! runtime itself, so a caller that offers a memory limit can offer one the
//! runtime can honour.

use std::{fmt, path::Path, time::Duration};

/// One MiB in bytes, the unit `--max-old-space-size` counts in.
const MIB: u64 = 1024 * 1024;

/// How long one probe may take before the child is killed and reported as a
/// failure. A short script in Node mode answers in well under a second; a runtime
/// that cannot answer at all must not hang the caller that asked.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Prints the limit V8 settled on, in bytes.
///
/// `heap_size_limit` is the number the process itself would read, so it covers
/// the young generation as well as the old space the switch names.
const PROBE: &str =
  "process.stdout.write(String(require('node:v8').getHeapStatistics().heap_size_limit))";

/// What the runtime really enforces for one `--max-old-space-size` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapSizeLimit {
  requested_mib: u64,
  limit_bytes: u64,
}

impl HeapSizeLimit {
  /// The `--max-old-space-size` the caller asked for, in MiB.
  pub fn requested_mib(&self) -> u64 {
    self.requested_mib
  }

  /// `v8.getHeapStatistics().heap_size_limit`, in bytes.
  pub fn limit_bytes(&self) -> u64 {
    self.limit_bytes
  }

  /// [`Self::limit_bytes`] in whole MiB.
  ///
  /// V8 adds its young generation on top of the old space the switch names, so
  /// an honoured request reports slightly more than it asked for.
  pub fn limit_mib(&self) -> u64 {
    self.limit_bytes / MIB
  }

  /// Whether the runtime cannot give even the old-space size that was asked for.
  ///
  /// This is the answer a caller needs before offering a range: `true` means the
  /// request was clamped, and the process will run with less memory than the
  /// caller told it it had.
  pub fn is_clamped(&self) -> bool {
    self.limit_bytes < self.requested_mib.saturating_mul(MIB)
  }
}

impl fmt::Display for HeapSizeLimit {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      formatter,
      "{} MiB requested, {} MiB enforced",
      self.requested_mib,
      self.limit_mib()
    )
  }
}

/// Runs `binary` in Node mode once and reads the heap limit it enforces.
pub(crate) async fn measure(binary: &Path, requested_mib: u64) -> std::io::Result<HeapSizeLimit> {
  let output = tokio::time::timeout(
    PROBE_TIMEOUT,
    electron_rs::node(binary)
      .arg(format!("--max-old-space-size={requested_mib}"))
      .args(["-e", PROBE])
      .output(),
  )
  .await
  .map_err(|_| {
    std::io::Error::new(
      std::io::ErrorKind::TimedOut,
      format!("the runtime did not report its heap limit within {PROBE_TIMEOUT:?}"),
    )
  })??;
  let stdout = String::from_utf8_lossy(&output.stdout);
  let limit_bytes = stdout
    .trim()
    .parse::<u64>()
    .ok()
    .filter(|limit| *limit > 0)
    .ok_or_else(|| {
      std::io::Error::other(format!(
        "the runtime reported no heap limit ({}, stdout {:?}, stderr {:?})",
        output.status,
        stdout.trim(),
        String::from_utf8_lossy(&output.stderr).trim()
      ))
    })?;
  Ok(HeapSizeLimit {
    requested_mib,
    limit_bytes,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  fn limit(requested_mib: u64, limit_bytes: u64) -> HeapSizeLimit {
    HeapSizeLimit {
      requested_mib,
      limit_bytes,
    }
  }

  #[test]
  fn an_honoured_request_reports_more_than_it_asked_for() {
    // V8 adds its young generation to the old space the switch names, so the
    // reported limit of a request the runtime can honour is never below it.
    let honoured = limit(1024, 1120 * MIB);
    assert_eq!(honoured.limit_mib(), 1120);
    assert!(!honoured.is_clamped());
  }

  #[test]
  fn the_cage_ceiling_is_not_a_clamp_for_a_request_it_can_hold() {
    // 4192 MiB is what a caged runtime answers for anything above it; a request
    // at or below the ceiling is served, so only the part above it is a clamp.
    assert!(!limit(4096, 4192 * MIB).is_clamped());
    assert!(!limit(4192, 4192 * MIB).is_clamped());
    assert!(limit(4193, 4192 * MIB).is_clamped());
  }

  #[test]
  fn a_request_nobody_could_honour_does_not_overflow() {
    assert!(limit(u64::MAX, 4192 * MIB).is_clamped());
  }

  #[test]
  fn the_report_says_what_was_asked_for_and_what_was_given() {
    assert_eq!(
      limit(15000, 4192 * MIB).to_string(),
      "15000 MiB requested, 4192 MiB enforced"
    );
  }
}
