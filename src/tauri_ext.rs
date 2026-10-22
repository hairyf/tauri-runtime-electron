// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use crate::runtime::Electron;
use tauri_runtime::UserEvent;

impl<T: UserEvent> From<Electron> for tauri_runtime::dynamic::DynRuntimeInitAttrs<T> {
  fn from(attrs: Electron) -> Self {
    Self::new(attrs)
  }
}
