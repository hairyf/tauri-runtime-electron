//! Tauri window effects delegated to this runtime without changing Tauri.

use super::{background_material, EffectsTarget, WindowEffectsConfig, EFFECTS_TARGET};
use crate::{runtime::IPC_TIMEOUT, ElectronRuntime};
use std::{ops::Deref, sync::mpsc::sync_channel};
use tauri::{plugin::TauriPlugin, Manager};

/// An official Tauri window whose effect methods use Electron's compositor.
/// Other methods remain available through `Deref`.
#[derive(Clone)]
pub struct Window(tauri::Window<ElectronRuntime>);

impl From<tauri::Window<ElectronRuntime>> for Window {
  fn from(window: tauri::Window<ElectronRuntime>) -> Self {
    Self(window)
  }
}

impl From<tauri::WebviewWindow<ElectronRuntime>> for Window {
  fn from(window: tauri::WebviewWindow<ElectronRuntime>) -> Self {
    Self(window.as_ref().window())
  }
}

impl Deref for Window {
  type Target = tauri::Window<ElectronRuntime>;

  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl Window {
  /// Applies the first supported effect. Windows 11 22H2+ supports Acrylic,
  /// Mica and Tabbed; other platforms and unsupported lists return an error.
  pub fn set_effects<E: Into<Option<WindowEffectsConfig>>>(&self, effects: E) -> tauri::Result<()> {
    set_effects(&self.0, effects)
  }

  /// Clears effects and restores the configured window background.
  pub fn clear_effects(&self) -> tauri::Result<()> {
    clear_effects(&self.0)
  }
}

/// Applies effects to an existing official Tauri window on this runtime.
/// The scheduled task is scoped to that window and Electron connection.
pub fn set_effects<E: Into<Option<WindowEffectsConfig>>>(
  window: &tauri::Window<ElectronRuntime>,
  effects: E,
) -> tauri::Result<()> {
  let effects = effects.into();
  let material = background_material(effects.as_ref())?;
  let (tx, rx) = sync_channel(1);
  window.run_on_main_thread(move || {
    let target: Option<EffectsTarget> = EFFECTS_TARGET.with(|slot| slot.borrow().clone());
    let result = target
      .ok_or_else(|| std::io::Error::other("Electron window route is unavailable"))
      .and_then(|target| target.apply(material));
    let _ = tx.send(result);
  })?;
  rx.recv_timeout(IPC_TIMEOUT)
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "Electron window task did not finish before timeout"))?
    .map_err(Into::into)
}

/// Clears effects on an existing official Tauri window.
pub fn clear_effects(window: &tauri::Window<ElectronRuntime>) -> tauri::Result<()> {
  set_effects(window, None)
}

mod commands {
  use super::*;

  #[tauri::command]
  pub async fn set_effects(
    window: tauri::Window<ElectronRuntime>,
    label: Option<String>,
    value: Option<WindowEffectsConfig>,
  ) -> Result<(), String> {
    let target = match label.filter(|label| !label.is_empty()) {
      Some(label) => window.get_window(&label).ok_or(tauri::Error::WindowNotFound),
      None => Ok(window),
    }.map_err(|error| error.to_string())?;
    super::set_effects(&target, value).map_err(|error| error.to_string())
  }
}

/// Registers the command used by `tauri-runtime-electron/api/window`.
/// Declare an inlined `electron-window` plugin with command `set_effects` in
/// the application's `tauri-build` attributes, then explicitly grant
/// `electron-window:allow-set-effects` in its capabilities.
pub fn init() -> TauriPlugin<ElectronRuntime> {
  tauri::plugin::Builder::new("electron-window")
    .invoke_handler(tauri::generate_handler![commands::set_effects])
    .build()
}
