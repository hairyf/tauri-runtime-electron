//! Newline-delimited JSON RPC over a loopback TCP socket.
//!
//! The Rust side owns the listener and spawns the Electron main process with the
//! chosen port as an argument. Electron connects back, sends a `ready` event, and
//! from then on both sides exchange `{"id": n, "method": ..., "params": ...}` /
//! `{"id": n, "result": ...}` (or `{"id": n, "error": "..."}`) plus
//! `{"event": ..., "params": ...}` notifications.

use std::{
  collections::HashMap,
  io::{BufRead, BufReader, Write},
  net::{TcpListener, TcpStream},
  path::PathBuf,
  process::{Child, Command},
  sync::{
    atomic::{AtomicU64, Ordering},
    mpsc::{channel, Receiver, Sender},
    Arc, Mutex,
  },
  time::{Duration, Instant},
};

use serde_json::{json, Value};

/// Error message returned by the Electron side.
pub(crate) type IpcResult = std::result::Result<Value, String>;

pub(crate) struct Ipc {
  writer: Mutex<TcpStream>,
  pending: Mutex<HashMap<u64, Sender<IpcResult>>>,
  next_id: AtomicU64,
  child: Mutex<Option<Child>>,
  /// Version reported by the Electron process during the handshake.
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
  /// Spawns `electron <main_script> --port <port>` and waits for the handshake.
  ///
  /// The returned receiver yields every notification the Electron side sends.
  pub(crate) fn launch(
    binary: &PathBuf,
    main_script: &PathBuf,
    extra_args: &[String],
    timeout: Duration,
  ) -> std::result::Result<(Arc<Ipc>, Receiver<Value>), String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
      .map_err(|e| format!("failed to bind IPC listener: {e}"))?;
    let port = listener
      .local_addr()
      .map_err(|e| format!("failed to read IPC listener address: {e}"))?
      .port();

    let mut command = Command::new(binary);
    command
      .arg(main_script)
      .arg("--port")
      .arg(port.to_string())
      .args(extra_args);
    #[cfg(windows)]
    {
      use std::os::windows::process::CommandExt;
      // Keep the Rust console clean: Electron is a GUI process, no extra window needed.
      command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| {
      format!(
        "failed to spawn electron binary {}: {e}",
        binary.display()
      )
    })?;

    listener
      .set_nonblocking(true)
      .map_err(|e| format!("failed to configure IPC listener: {e}"))?;
    let deadline = Instant::now() + timeout;
    let stream = loop {
      match listener.accept() {
        Ok((stream, _)) => {
          // On Windows the accepted socket inherits the listener's non-blocking mode.
          stream
            .set_nonblocking(false)
            .map_err(|e| format!("failed to configure IPC stream: {e}"))?;
          break stream;
        }
        Err(e) if Instant::now() >= deadline => {
          let _ = child.kill();
          return Err(format!(
            "electron did not connect within {timeout:?}: {e} (is the electron binary at {} correct?)",
            binary.display()
          ));
        }
        Err(_) => std::thread::sleep(Duration::from_millis(20)),
      }
    };

    let handshake = read_line(&stream, timeout);
    let handshake = match handshake {
      Ok(line) => line,
      Err(e) => {
        let _ = child.kill();
        return Err(format!("electron handshake failed: {e}"));
      }
    };
    let handshake: Value = serde_json::from_str(&handshake)
      .map_err(|e| format!("invalid electron handshake: {e}"))?;
    if handshake.get("event").and_then(Value::as_str) != Some("ready") {
      let _ = child.kill();
      return Err(format!("unexpected electron handshake: {handshake}"));
    }
    let version = handshake
      .pointer("/params/electron")
      .and_then(Value::as_str)
      .unwrap_or("unknown")
      .to_string();

    let writer = stream
      .try_clone()
      .map_err(|e| format!("failed to clone IPC stream: {e}"))?;
    let (event_tx, event_rx) = channel();
    let ipc = Arc::new(Ipc {
      writer: Mutex::new(writer),
      pending: Mutex::new(HashMap::new()),
      next_id: AtomicU64::new(0),
      child: Mutex::new(Some(child)),
      version,
    });

    let reader_ipc = ipc.clone();
    std::thread::Builder::new()
      .name("electron-ipc".into())
      .spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
          line.clear();
          match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
          }
          let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
          };
          reader_ipc.dispatch(message, &event_tx);
        }
        // The Electron process is gone; let the event loop wind down.
        let _ = event_tx.send(json!({ "event": "ipc.closed" }));
      })
      .map_err(|e| format!("failed to start IPC reader thread: {e}"))?;

    Ok((ipc, event_rx))
  }

  /// Performs a synchronous RPC. `timeout` guards against a wedged Electron process.
  pub(crate) fn call(
    &self,
    method: &str,
    params: Value,
    timeout: Duration,
  ) -> IpcResult {
    let id = self.next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = channel();
    self.pending.lock().unwrap().insert(id, tx);

    let request = json!({ "id": id, "method": method, "params": params });
    let mut line = serde_json::to_string(&request).map_err(|e| e.to_string())?;
    line.push('\n');
    {
      let mut writer = self.writer.lock().unwrap();
      if let Err(e) = writer
        .write_all(line.as_bytes())
        .and_then(|()| writer.flush())
      {
        self.pending.lock().unwrap().remove(&id);
        return Err(format!("failed to write to electron ({method}): {e}"));
      }
    }

    match rx.recv_timeout(timeout) {
      Ok(result) => result,
      Err(_) => {
        self.pending.lock().unwrap().remove(&id);
        Err(format!("electron did not answer `{method}` in {timeout:?}"))
      }
    }
  }

  fn dispatch(&self, message: Value, events: &Sender<Value>) {
    let Some(id) = message.get("id").and_then(Value::as_u64) else {
      let _ = events.send(message);
      return;
    };
    let Some(waiter) = self.pending.lock().unwrap().remove(&id) else {
      return;
    };
    let result = if let Some(error) = message.get("error").and_then(Value::as_str) {
      Err(error.to_string())
    } else {
      Ok(message.get("result").cloned().unwrap_or(Value::Null))
    };
    let _ = waiter.send(result);
  }

  /// Closes the socket and terminates the Electron process.
  pub(crate) fn shutdown(&self) {
    if let Ok(writer) = self.writer.lock() {
      let _ = writer.shutdown(std::net::Shutdown::Both);
    }
    if let Some(mut child) = self.child.lock().unwrap().take() {
      let _ = child.kill();
      let _ = child.wait();
    }
  }
}

fn read_line(stream: &TcpStream, timeout: Duration) -> std::result::Result<String, String> {
  stream
    .set_read_timeout(Some(timeout))
    .map_err(|e| e.to_string())?;
  let mut line = String::new();
  let mut reader = BufReader::new(stream);
  reader
    .read_line(&mut line)
    .map_err(|e| e.to_string())?;
  if line.trim().is_empty() {
    return Err("electron closed the socket during the handshake".into());
  }
  Ok(line)
}
