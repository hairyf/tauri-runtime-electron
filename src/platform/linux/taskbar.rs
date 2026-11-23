// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The Unity launcher entry, which Electron owns.
//!
//! The reference loads `libunity` itself (through `dlopen2`), finds the launcher entry for
//! the application's `.desktop` file and drives its progress and count properties, because
//! nothing else on the Tauri side does. Electron links `libunity` in its own process and
//! implements exactly this for `window.progressBar` and `window.badgeCount`, so the
//! bookkeeping below is kept — it is what decides whether a request is worth sending — and
//! the library calls report.
//!
//! `dlopen2` is not among this crate's dependencies, so there is no `UnityLib` wrapper to
//! keep: the loading it would do is Electron's.

use std::{cell::RefCell, ffi::CString};
use tauri_runtime::ProgressBarState;

pub(super) fn set_progress_bar(state: ProgressBarState) {
  TASKBAR_INDICATOR.with_borrow_mut(|taskbar| taskbar.update(state));
}

pub(super) fn set_badge_count(count: Option<i64>, desktop_filename: Option<String>) {
  TASKBAR_INDICATOR.with_borrow_mut(|taskbar| taskbar.update_count(count, desktop_filename));
}

struct TaskbarIndicator {
  desktop_filename: Option<String>,
  desktop_filename_c_str: Option<CString>,

  attempted_load: bool,

  unity_entry: Option<*const isize>,
}

thread_local! {
  static TASKBAR_INDICATOR: RefCell<TaskbarIndicator> = RefCell::new(TaskbarIndicator::new());
}

impl TaskbarIndicator {
  fn new() -> Self {
    Self {
      desktop_filename: None,
      desktop_filename_c_str: None,

      attempted_load: false,

      unity_entry: None,
    }
  }

  /// Loads `libunity` and asks it for the inspector.
  ///
  /// Electron performs the load in its own process; nothing here holds a `dlopen` handle.
  fn ensure_lib_load(&mut self) {
    if self.attempted_load {
      return;
    }

    self.attempted_load = true;
    crate::unsupported::report(
      "platform::linux::TaskbarIndicator::ensure_lib_load (Electron links libunity)",
    );
  }

  /// Finds the launcher entry for the recorded `.desktop` file.
  fn ensure_entry_load(&mut self) {
    if self.desktop_filename_c_str.is_some() && self.unity_entry.is_none() {
      crate::unsupported::report(
        "platform::linux::TaskbarIndicator::ensure_entry_load (Electron: window.progressBar)",
      );
    }
  }

  /// Whether a Unity session is running.
  ///
  /// The reference asks the loaded `libunity` inspector. Nothing loads `libunity` here, so
  /// there is never an inspector to ask and the answer is always "no" — which is also what
  /// the reference answers on a desktop without Unity, where it stops before making any
  /// launcher call.
  fn is_unity_running(&self) -> bool {
    false
  }

  fn update(&mut self, progress: ProgressBarState) {
    if let Some(uri) = progress.desktop_filename {
      self.desktop_filename = Some(uri);
      self.desktop_filename_c_str = None;
      self.unity_entry = None;
    }

    self.ensure_lib_load();

    if !self.is_unity_running() {
      return;
    }

    if self.desktop_filename_c_str.is_none()
      && let Some(uri) = &self.desktop_filename
    {
      self.desktop_filename_c_str = Some(CString::new(uri.as_str()).unwrap_or_default());
    }

    if self.unity_entry.is_none() {
      self.ensure_entry_load();
    }

    // `progress.progress` is a percentage and `progress.status` hides the bar when it is
    // `None`; both reach the launcher entry through Electron's own `libunity` calls.
    crate::unsupported::report(
      "platform::linux::TaskbarIndicator::update (Electron: window.progressBar)",
    );
  }

  fn update_count(&mut self, _count: Option<i64>, desktop_filename: Option<String>) {
    if let Some(uri) = desktop_filename {
      self.desktop_filename = Some(uri);
      self.desktop_filename_c_str = None;
      self.unity_entry = None;
    }

    self.ensure_lib_load();

    if !self.is_unity_running() {
      return;
    }

    if self.desktop_filename_c_str.is_none()
      && let Some(uri) = &self.desktop_filename
    {
      self.desktop_filename_c_str = Some(CString::new(uri.as_str()).unwrap_or_default());
    }

    if self.unity_entry.is_none() {
      self.ensure_entry_load();
    }

    // A `None` count hides the badge; Electron applies both through its own launcher entry.
    crate::unsupported::report(
      "platform::linux::TaskbarIndicator::update_count (Electron: window.badgeCount)",
    );
  }
}
