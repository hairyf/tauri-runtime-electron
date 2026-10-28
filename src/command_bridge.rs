//! Access the authenticated client of an already running Electron main process.

/// The current runtime's shared client, not a second Electron process.
///
/// With the `tauri` feature, register `tauri_runtime_electron::init()` to inject
/// this type as managed state.
pub type ElectronClient = std::sync::Arc<electron_rs::Electron>;

#[cfg(feature = "tauri")]
thread_local! {
  static CLIENT: std::cell::RefCell<Option<ElectronClient>> = const { std::cell::RefCell::new(None) };
}

#[cfg(feature = "tauri")]
pub(crate) fn with_client<R>(client: ElectronClient, task: impl FnOnce() -> R) -> R {
  struct Restore(Option<ElectronClient>);
  impl Drop for Restore {
    fn drop(&mut self) {
      CLIENT.with(|slot| slot.replace(self.0.take()));
    }
  }
  let _restore = Restore(CLIENT.with(|slot| slot.replace(Some(client))));
  task()
}

/// Register this app's existing Electron client before application setup.
///
/// No window is needed. This plugin adds only Rust managed state: it registers no
/// renderer commands and does not change Tauri permissions. Registration fails if
/// an `ElectronClient` has already been managed, rather than using another client.
///
/// ```no_run
/// use tauri_runtime_electron::{ElectronClient, ElectronRuntime};
///
/// #[tauri::command]
/// async fn run_task(electron: tauri::State<'_, ElectronClient>) -> Result<String, String> {
///   let client = electron.inner().clone();
///   let output = client.run("return 'ready';").await.map_err(|error| error.to_string())?;
///   Ok(String::from_utf8_lossy(&output.stdout).into_owned())
/// }
///
/// let builder = tauri::Builder::<ElectronRuntime>::new()
///   .plugin(tauri_runtime_electron::init())
///   .invoke_handler(tauri::generate_handler![run_task]);
/// ```
#[cfg(feature = "tauri")]
pub fn init() -> tauri::plugin::TauriPlugin<crate::ElectronRuntime> {
  use tauri::Manager;

  tauri::plugin::Builder::new("electron-client")
    .setup(|app, _api| {
      let (sender, receiver) = std::sync::mpsc::sync_channel(1);
      // The public app API reaches this exact runtime handle, which scopes CLIENT
      // to this task. Runtime UI-thread tasks execute inline, including in setup.
      app.run_on_main_thread(move || {
        let _ = sender.send(CLIENT.with(|slot| slot.borrow().clone()));
      })?;
      let client = receiver
        .recv_timeout(crate::runtime::IPC_TIMEOUT)
        .map_err(|error| {
          let kind = match error {
            std::sync::mpsc::RecvTimeoutError::Timeout => std::io::ErrorKind::TimedOut,
            std::sync::mpsc::RecvTimeoutError::Disconnected => std::io::ErrorKind::BrokenPipe,
          };
          std::io::Error::new(
            kind,
            format!("Electron runtime client route failed: {error}"),
          )
        })?
        .ok_or_else(|| std::io::Error::other("Electron runtime client route is unavailable"))?;
      if !app.manage(client) {
        return Err(
          std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "ElectronClient is already managed by this app",
          )
          .into(),
        );
      }
      Ok(())
    })
    .build()
}

#[cfg(all(test, feature = "tauri", windows))]
mod tests {
  use super::*;
  use std::sync::Arc;
  use tauri::Manager;

  #[test]
  fn managed_clients_are_app_scoped_without_windows_and_routes_restore() {
    let first = tauri::Builder::<crate::ElectronRuntime>::new()
      .any_thread()
      .plugin(init())
      .build(tauri::generate_context!(
        "tests/assets/tauri-new-window/tauri.conf.json",
        test = true
      ))
      .expect("build first windowless app");
    let second = tauri::Builder::<crate::ElectronRuntime>::new()
      .any_thread()
      .plugin(init())
      .build(tauri::generate_context!(
        "tests/assets/tauri-new-window/tauri.conf.json",
        test = true
      ))
      .expect("build second windowless app");
    let first_state: tauri::State<'_, ElectronClient> = first.state();
    let second_state: tauri::State<'_, ElectronClient> = second.state();
    let first_client = first_state.inner().clone();
    let second_client = second_state.inner().clone();
    assert!(first.webviews().is_empty() && second.webviews().is_empty());
    assert!(!Arc::ptr_eq(&first_client, &second_client));
    assert!(CLIENT.with(|slot| slot.borrow().is_none()));

    let outer = first_client.clone();
    let inner = second_client.clone();
    let second_app = second.handle().clone();
    first
      .run_on_main_thread(move || {
        assert!(CLIENT.with(|slot| Arc::ptr_eq(slot.borrow().as_ref().unwrap(), &outer)));
        second_app
          .run_on_main_thread(move || {
            assert!(CLIENT.with(|slot| Arc::ptr_eq(slot.borrow().as_ref().unwrap(), &inner)));
          })
          .unwrap();
        assert!(CLIENT.with(|slot| Arc::ptr_eq(slot.borrow().as_ref().unwrap(), &outer)));
        assert!(
          std::thread::spawn(|| CLIENT.with(|slot| slot.borrow().is_none()))
            .join()
            .unwrap()
        );
      })
      .unwrap();
    assert!(CLIENT.with(|slot| slot.borrow().is_none()));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      with_client(first_client.clone(), || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
          with_client(second_client.clone(), || panic!("test scoped cleanup"));
        }));
        assert!(result.is_err());
        assert!(CLIENT.with(|slot| Arc::ptr_eq(slot.borrow().as_ref().unwrap(), &first_client)));
        panic!("test outer cleanup");
      });
    }));
    assert!(result.is_err());
    assert!(CLIENT.with(|slot| slot.borrow().is_none()));

    let error = first.handle().plugin(init()).unwrap_err();
    assert!(
      error
        .to_string()
        .contains("ElectronClient is already managed"),
      "{error}"
    );
    assert!(Arc::ptr_eq(
      first.state::<ElectronClient>().inner(),
      &first_client
    ));
    assert!(Arc::ptr_eq(
      second.state::<ElectronClient>().inner(),
      &second_client
    ));
  }
}
