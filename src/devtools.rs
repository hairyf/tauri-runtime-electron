// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::sync::atomic::{AtomicU64, Ordering};

/// First identifier of the range reserved for the runtime's own requests.
///
/// Callers allocate below this bound, starting at 1, and the runtime allocates
/// from it upwards. Each counter is bounded by its own range and fails closed
/// there, so neither can ever reach the other one, no matter how many
/// identifiers are allocated.
///
/// The bound is the reference's, and it is kept: it holds every caller-visible
/// identifier inside the positive 32-bit range the DevTools agent accepts, which
/// is what a caller forwarding one over `--remote-debugging-port` needs, while
/// the reserved range above it is free to run to the top of `u64`.
const RESERVED_RANGE_START: u64 = 1_000_000_000;

static NEXT_MESSAGE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_RESERVED_MESSAGE_ID: AtomicU64 = AtomicU64::new(RESERVED_RANGE_START);

/// The process-local native DevTools request identifier space is exhausted.
#[derive(Clone, Copy, Debug)]
pub struct DevToolsMessageIdExhausted;

impl std::fmt::Display for DevToolsMessageIdExhausted {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    formatter.write_str("native Electron DevTools message identifiers are exhausted")
  }
}

impl std::error::Error for DevToolsMessageIdExhausted {}

/// Allocates one native DevTools request ID for a caller of this crate.
///
/// Electron numbers its own protocol traffic: `webContents.debugger.sendCommand`
/// allocates the request id, matches the response and delivers it without ever
/// exposing the number, so the runtime's own requests need no allocator at all.
/// A caller that speaks the protocol itself does need one — over
/// `--remote-debugging-port`, or over the single `webContents.debugger`
/// attachment this runtime holds a lease on — and every such caller must use
/// this allocator, because observers see the whole browser and two callers
/// picking the same number consume each other's results.
///
/// The runtime allocates its own requests from a reserved range this function
/// never returns, so a caller cannot answer an internal request by picking its
/// number.
///
/// IDs are positive and never reused, including after cancellation or browser
/// teardown. Numeric correlation does not authorize a browser or document.
pub fn allocate_devtools_message_id() -> Result<u64, DevToolsMessageIdExhausted> {
  allocate_from(&NEXT_MESSAGE_ID, RESERVED_RANGE_START - 1)
}

/// Allocates one native DevTools request ID for the runtime itself.
///
/// The identifiers come from the range reserved above every caller-visible one, so internal
/// correlation only ever matches requests the runtime sent, even when a caller ignores
/// [`allocate_devtools_message_id`] and hardcodes an `id`.
///
/// The runtime never allocates from it: Electron numbers the requests the runtime makes
/// through `webContents.debugger`, and one protocol session per `WebContents` means a second
/// consumer has to share this runtime's attachment rather than open its own. The range stays
/// reserved so the two halves cannot drift into each other if that lease is ever exposed.
#[allow(dead_code)]
pub(crate) fn allocate_runtime_devtools_message_id() -> Result<u64, DevToolsMessageIdExhausted> {
  allocate_from(&NEXT_RESERVED_MESSAGE_ID, u64::MAX)
}

/// Allocates the next identifier of `counter`, which hands out values up to
/// `last`. Each range fails closed at its own boundary instead of wrapping or
/// growing into the other one.
fn allocate_from(counter: &AtomicU64, last: u64) -> Result<u64, DevToolsMessageIdExhausted> {
  counter
    .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
      if value > last {
        return None;
      }
      value.checked_add(1)
    })
    .map_err(|_| DevToolsMessageIdExhausted)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::HashSet;

  #[test]
  fn concurrent_native_producers_never_share_a_response_id() {
    let threads = (0..4)
      .map(|_| {
        std::thread::spawn(|| {
          (0..1_000)
            .map(|_| allocate_devtools_message_id().unwrap())
            .collect::<Vec<_>>()
        })
      })
      .collect::<Vec<_>>();
    let ids = threads
      .into_iter()
      .flat_map(|thread| thread.join().unwrap())
      .collect::<Vec<_>>();
    assert!(ids.iter().all(|id| *id > 0));
    assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 4_000);
  }

  #[test]
  fn callers_and_the_runtime_never_share_a_response_id() {
    let caller_ids = (0..1_000)
      .map(|_| allocate_devtools_message_id().unwrap())
      .collect::<HashSet<_>>();
    let runtime_ids = (0..1_000)
      .map(|_| allocate_runtime_devtools_message_id().unwrap())
      .collect::<HashSet<_>>();
    assert!(
      caller_ids
        .iter()
        .all(|id| *id > 0 && *id < RESERVED_RANGE_START)
    );
    assert!(runtime_ids.iter().all(|id| *id >= RESERVED_RANGE_START));
    assert!(caller_ids.is_disjoint(&runtime_ids));
  }

  #[test]
  fn caller_exhaustion_cannot_reach_the_reserved_range() {
    let last = RESERVED_RANGE_START - 1;
    let counter = AtomicU64::new(last);
    assert_eq!(allocate_from(&counter, last).unwrap(), last);
    assert!(allocate_from(&counter, last).is_err());
    assert!(allocate_from(&counter, last).is_err());
    assert_eq!(counter.load(Ordering::Relaxed), RESERVED_RANGE_START);
  }

  #[test]
  fn exhaustion_cannot_reuse_a_late_response_id() {
    let counter = AtomicU64::new(u64::MAX - 1);
    assert_eq!(allocate_from(&counter, u64::MAX).unwrap(), u64::MAX - 1);
    assert!(allocate_from(&counter, u64::MAX).is_err());
    assert!(allocate_from(&counter, u64::MAX).is_err());
    assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
  }
}
