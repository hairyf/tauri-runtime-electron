//! The `electron-rs` Ipc: Electron process ownership, the event stream and
//! synchronous RPC.
//!
//! `electron-rs` owns the Electron main process, the loopback socket, the
//! handshake, the JavaScript command table and the event stream. The
//! `tauri-runtime` traits are synchronous, so this module is the only place
//! that enters the async client: one dedicated thread runs a Tokio runtime,
//! and every call is dispatched to it and awaited on the caller's thread.

use std::{
  collections::HashSet,
  future::Future,
  path::{Path, PathBuf},
  sync::{Arc, Mutex, OnceLock, mpsc},
  time::Duration,
};

use electron_rs::{Electron, RpcRequest};
use serde_json::Value;
use tokio::runtime::Runtime;

/// The error text the Electron side reports for a failed request.
pub(crate) type IpcResult = std::result::Result<Value, String>;

type Task = Box<dyn FnOnce(&Runtime) + Send>;

/// Interns the method names so [`Raw`] can hand `electron-rs` a `&'static str`.
///
/// The set of method names is fixed by the command table, so the leak is bounded
/// by the number of distinct methods the runtime ever calls.
fn intern(method: &str) -> &'static str {
  static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
  let mut names = NAMES
    .get_or_init(Default::default)
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner());
  if let Some(name) = names.get(method) {
    return name;
  }
  let leaked: &'static str = Box::leak(method.to_owned().into_boxed_str());
  names.insert(leaked);
  leaked
}

/// A request whose method and payload the runtime already built.
///
/// The runtime keeps its wire-shaped call sites; `electron-rs` still owns the
/// transport, the id allocation and the response matching.
struct Raw {
  method: &'static str,
  params: Value,
  timeout: Duration,
}

impl serde::Serialize for Raw {
  fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
    self.params.serialize(serializer)
  }
}

impl RpcRequest for Raw {
  type Response = Value;
  const METHOD: &'static str = "";
  fn method(&self) -> &'static str {
    self.method
  }
  fn timeout(&self) -> Duration {
    self.timeout
  }
}

/// A connected Electron process plus the thread that drives its async client.
pub(crate) struct Ipc {
  electron: Arc<Electron>,
  tasks: mpsc::SyncSender<Task>,
  /// Version reported by Electron during the handshake.
  pub(crate) version: String,
}

impl std::fmt::Debug for Ipc {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Ipc")
      .field("version", &self.version)
      .finish()
  }
}

impl Ipc {
  /// Launches the prepared Electron distribution and waits for the handshake.
  ///
  /// The returned receiver yields every notification the Electron side sends,
  /// including the final `ipc.closed`.
  pub(crate) fn launch(
    binary: &Path,
    args: &[String],
    timeout: Duration,
  ) -> std::result::Result<(Arc<Self>, mpsc::Receiver<Value>), String> {
    let binary: PathBuf = binary.to_path_buf();
    let args = args.to_vec();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (tasks, receiver) = mpsc::sync_channel::<Task>(1024);
    std::thread::Builder::new()
      .name("electron-rpc-adapter".into())
      .spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
          .enable_time()
          .build()
        {
          Ok(runtime) => runtime,
          Err(error) => {
            let _ = ready_tx.send(Err(format!("create the Electron RPC runtime: {error}")));
            return;
          }
        };
        let electron = runtime.block_on(Electron::with_args(&binary, 0, &args, timeout));
        match electron {
          Ok(electron) => {
            if ready_tx.send(Ok(electron)).is_err() {
              return;
            }
          }
          Err(error) => {
            let _ = ready_tx.send(Err(error.to_string()));
            return;
          }
        }
        while let Ok(task) = receiver.recv() {
          task(&runtime);
        }
      })
      .map_err(|error| format!("start the Electron RPC adapter: {error}"))?;

    let electron = ready_rx.recv().map_err(|_| {
      "the Electron RPC adapter stopped before the handshake completed".to_string()
    })??;
    let version = electron.version.clone();
    let events = electron
      .events()
      .map_err(|error| format!("subscribe to Electron events: {error}"))?;
    Ok((
      Arc::new(Self {
        electron,
        tasks,
        version,
      }),
      events,
    ))
  }

  /// Performs a synchronous RPC. `timeout` guards against a wedged Electron process.
  pub(crate) fn call(&self, method: &str, params: Value, timeout: Duration) -> IpcResult {
    let (sender, receiver) = mpsc::sync_channel(1);
    self.dispatch(
      Raw {
        method: intern(method),
        params,
        timeout,
      },
      move |result| {
        let _ = sender.send(result);
      },
    )?;
    // `recv` reports the channel closing separately from the request's own answer,
    // so the two results are flattened into one.
    let receive = || -> IpcResult {
      receiver
        .recv()
        .map_err(|_| format!("the Electron RPC adapter stopped answering `{method}`"))?
    };
    // A blocking receive inside a Tokio worker would stall the reactor, so the
    // worker hands the thread back before waiting.
    let on_worker = tokio::runtime::Handle::try_current()
      .is_ok_and(|handle| handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread);
    if on_worker {
      tokio::task::block_in_place(receive)
    } else {
      receive()
    }
  }

  /// Runs `request` on the adapter thread and feeds the result to `callback`.
  fn dispatch<R: RpcRequest>(
    &self,
    request: R,
    callback: impl FnOnce(std::result::Result<R::Response, String>) + Send + 'static,
  ) -> std::result::Result<(), String> {
    let electron = Arc::clone(&self.electron);
    self
      .tasks
      .try_send(Box::new(move |runtime| {
        let mut task = Box::pin(async move {
          callback(
            electron
              .call(request)
              .await
              .map_err(|error| error.to_string()),
          );
        });
        // Reserve the client's writer order before accepting the next request.
        let complete = runtime.block_on(std::future::poll_fn(|cx| {
          std::task::Poll::Ready(task.as_mut().poll(cx).is_ready())
        }));
        if !complete {
          runtime.spawn(task);
        }
      }))
      .map_err(|error| match error {
        mpsc::TrySendError::Disconnected(_) => "the Electron RPC adapter has stopped".to_string(),
        mpsc::TrySendError::Full(_) => "the Electron RPC adapter task queue is full".to_string(),
      })
  }

  /// Closes the socket and terminates the Electron process.
  pub(crate) fn shutdown(&self) {
    self.electron.shutdown();
  }
}
