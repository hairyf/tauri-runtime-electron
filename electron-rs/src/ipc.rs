use std::{
  collections::HashMap,
  io::{self, BufRead, BufReader, Write},
  net::{Shutdown, TcpListener, TcpStream},
  path::{Path, PathBuf},
  process::{Child, Command},
  sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, Sender},
    Arc, Mutex,
  },
  time::{Duration, Instant},
};

use serde_json::{json, Value};

/// An unsolicited notification from Electron.
pub type Event = Value;
type RpcResult = Result<Value, String>;

/// A connected Electron process with synchronous RPC and event subscriptions.
pub struct Electron {
  binary: PathBuf,
  writer: Mutex<TcpStream>,
  pending: Mutex<HashMap<u64, Sender<RpcResult>>>,
  events: Mutex<Option<Receiver<Event>>>,
  subscribers: Mutex<Vec<Sender<Event>>>,
  next_id: AtomicU64,
  closed: AtomicBool,
  child: Mutex<Option<Child>>,
  /// Version reported by Electron during the handshake.
  pub version: String,
}

impl std::fmt::Debug for Electron {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Electron")
      .field("binary", &self.binary)
      .field("version", &self.version)
      .finish()
  }
}

impl Electron {
  /// Launch Electron using its staged main.cjs; port 0 selects a free port.
  pub fn new(binary: impl AsRef<Path>, port: u16) -> Result<Arc<Self>, String> {
    Self::with_args(binary, port, &[], Duration::from_secs(15))
  }

  /// Launch with app-specific switches and a deadline covering connection and handshake.
  /// The crate-owned main.cjs and preload.cjs must already be staged with the distribution.
  pub fn with_args(
    binary: impl AsRef<Path>,
    port: u16,
    args: &[String],
    timeout: Duration,
  ) -> Result<Arc<Self>, String> {
    if timeout.is_zero() {
      return Err("Electron startup timeout must be positive".into());
    }
    let binary = std::fs::canonicalize(binary.as_ref())
      .map_err(|error| format!("Electron binary {}: {error}", binary.as_ref().display()))?;
    let script = main_script(&binary)?;
    let identity = staged_identity(&binary, &script)?;
    let listener =
      TcpListener::bind(("127.0.0.1", port)).map_err(|error| format!("bind IPC: {error}"))?;
    let port = listener
      .local_addr()
      .map_err(|error| error.to_string())?
      .port();
    listener
      .set_nonblocking(true)
      .map_err(|error| error.to_string())?;
    let mut command = Command::new(&binary);
    command
      .arg(script)
      .arg("--port")
      .arg(port.to_string())
      .args(args)
      .env_remove("ELECTRON_RUN_AS_NODE")
      .env_remove("ELECTRON_RS_APP_NAME")
      .env_remove("ELECTRON_RS_IDENTIFIER");
    if let Some(name) = &identity.app_name {
      command.env("ELECTRON_RS_APP_NAME", name);
    }
    if let Some(identifier) = &identity.identifier {
      command.env("ELECTRON_RS_IDENTIFIER", identifier);
    }
    let owner = std::env::var_os("TAURI_RUNTIME_ELECTRON_OWNER")
      .or_else(|| {
        identity
          .identifier
          .as_ref()
          .or(identity.app_name.as_ref())
          .map(Into::into)
      })
      .or_else(|| {
        std::env::current_exe()
          .ok()
          .and_then(|exe| exe.file_stem().map(ToOwned::to_owned))
      })
      .ok_or("cannot determine the Electron profile owner")?;
    command.env("TAURI_RUNTIME_ELECTRON_OWNER", owner);
    #[cfg(windows)]
    {
      use std::os::windows::process::CommandExt;
      command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let deadline = Instant::now()
      .checked_add(timeout)
      .ok_or("Electron startup timeout is too large")?;
    let mut child = command
      .spawn()
      .map_err(|error| format!("spawn {}: {error}", binary.display()))?;
    let startup = (|| {
      let stream = loop {
        if Instant::now() >= deadline {
          return Err(format!("Electron did not connect within {timeout:?}"));
        }
        match listener.accept() {
          Ok((stream, _)) => break stream,
          Err(error)
            if matches!(
              error.kind(),
              io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) => {}
          Err(error) => return Err(format!("accept Electron IPC: {error}")),
        }
        if let Some(status) = child
          .try_wait()
          .map_err(|error| format!("poll Electron: {error}"))?
        {
          return Err(format!(
            "Electron exited before connecting ({status}): {}",
            binary.display()
          ));
        }
        std::thread::sleep(
          Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
        );
      };
      // Accepted sockets inherit the listener's nonblocking mode on Windows.
      stream
        .set_nonblocking(false)
        .map_err(|error| error.to_string())?;
      let mut reader = BufReader::new(stream);
      let version = read_handshake(&mut reader, deadline)?;
      let writer = reader
        .get_ref()
        .try_clone()
        .map_err(|error| error.to_string())?;
      Ok((reader, writer, version))
    })();
    let (reader, writer, version) = match startup {
      Ok(connection) => connection,
      Err(error) => {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
      }
    };
    let (event_tx, event_rx) = mpsc::channel();
    let electron = Arc::new(Self {
      binary,
      writer: Mutex::new(writer),
      pending: Mutex::new(HashMap::new()),
      events: Mutex::new(Some(event_rx)),
      subscribers: Mutex::new(vec![event_tx]),
      next_id: AtomicU64::new(0),
      closed: AtomicBool::new(false),
      child: Mutex::new(Some(child)),
      version,
    });
    // Drop also reaps the child if starting the reader thread fails.
    electron.start_reader(reader)?;
    Ok(electron)
  }

  /// Take the initial event receiver once, including events buffered since launch.
  pub fn events(&self) -> Result<Receiver<Event>, String> {
    self
      .events
      .lock()
      .map_err(|_| "events lock poisoned")?
      .take()
      .ok_or_else(|| "Electron events receiver already taken".into())
  }

  /// Subscribe to future events using a callback on a dedicated thread.
  /// The callback is dropped when Electron disconnects or shuts down.
  pub fn on(&self, handler: impl Fn(Event) + Send + 'static) {
    let receiver = self.subscribe();
    let _ = std::thread::Builder::new()
      .name("electron-events".into())
      .spawn(move || {
        while let Ok(event) = receiver.recv() {
          handler(event);
        }
      });
  }

  /// Subscribe to future events; subscriptions made after disconnect are closed.
  pub fn subscribe(&self) -> Receiver<Event> {
    let (sender, receiver) = mpsc::channel();
    if let Ok(mut subscribers) = self.subscribers.lock() {
      if !self.closed.load(Ordering::SeqCst) {
        subscribers.push(sender);
      }
    }
    receiver
  }

  /// Execute a synchronous RPC with a response timeout.
  pub fn call(&self, method: &str, params: Value, timeout: Duration) -> RpcResult {
    let id = self.next_id.fetch_add(1, Ordering::Relaxed);
    let mut line = serde_json::to_vec(&json!({"id":id,"method":method,"params":params}))
      .map_err(|error| error.to_string())?;
    line.push(b'\n');
    let (sender, receiver) = mpsc::channel();
    {
      let mut pending = self.pending.lock().map_err(|_| "pending lock poisoned")?;
      // Check while holding the same lock disconnect uses to drain waiters.
      if self.closed.load(Ordering::SeqCst) {
        return Err(format!("Electron is gone ({method})"));
      }
      if timeout.is_zero() {
        return Err(format!("Electron did not answer `{method}` in {timeout:?}"));
      }
      pending.insert(id, sender);
    }
    let written = self
      .writer
      .lock()
      .map_err(|_| "writer lock poisoned".to_string())
      .and_then(|mut writer| {
        writer
          .set_write_timeout(Some(timeout))
          .and_then(|()| writer.write_all(&line))
          .map_err(|error| error.to_string())
      });
    if let Err(error) = written {
      self.shutdown();
      return Err(format!("write Electron RPC `{method}`: {error}"));
    }
    match receiver.recv_timeout(timeout) {
      Ok(result) => result,
      Err(error) => {
        self
          .pending
          .lock()
          .unwrap_or_else(|poisoned| poisoned.into_inner())
          .remove(&id);
        match error {
          RecvTimeoutError::Timeout => {
            Err(format!("Electron did not answer `{method}` in {timeout:?}"))
          }
          RecvTimeoutError::Disconnected => Err(format!("Electron is gone ({method})")),
        }
      }
    }
  }

  /// Run this Electron binary in Node mode.
  pub fn node(&self, args: &[&str]) -> Result<std::process::Output, String> {
    let mut command = Command::new(&self.binary);
    command.args(args).env("ELECTRON_RUN_AS_NODE", "1");
    #[cfg(windows)]
    {
      use std::os::windows::process::CommandExt;
      command.creation_flags(0x0800_0000);
    }
    let output = command.output().map_err(|error| error.to_string())?;
    if output.status.success() {
      Ok(output)
    } else {
      Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
  }

  /// Close IPC, wake pending calls, close subscriptions, and terminate/reap Electron.
  pub fn shutdown(&self) {
    self.disconnect();
    let _ = self
      .writer
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .shutdown(Shutdown::Both);
    if let Some(mut child) = self
      .child
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .take()
    {
      let _ = child.kill();
      let _ = child.wait();
    }
  }

  fn start_reader(self: &Arc<Self>, mut reader: BufReader<TcpStream>) -> Result<(), String> {
    let electron = Arc::downgrade(self);
    std::thread::Builder::new()
      .name("electron-rpc".into())
      .spawn(move || {
        let mut line = String::new();
        while reader
          .read_line(&mut line)
          .ok()
          .is_some_and(|size| size > 0)
        {
          // Never retain an Electron owner across the blocking socket read.
          let Some(electron) = electron.upgrade() else {
            return;
          };
          if let Ok(message) = serde_json::from_str::<Value>(&line) {
            electron.dispatch(message);
          }
          line.clear();
        }
        if let Some(electron) = electron.upgrade() {
          electron.shutdown();
        }
      })
      .map(|_| ())
      .map_err(|error| format!("start Electron IPC reader: {error}"))
  }

  fn disconnect(&self) {
    if self.closed.swap(true, Ordering::SeqCst) {
      return;
    }
    self
      .pending
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .clear();
    for sender in self
      .subscribers
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .drain(..)
    {
      let _ = sender.send(json!({"event":"ipc.closed"}));
    }
  }

  fn dispatch(&self, message: Value) {
    let Some(id) = message["id"].as_u64() else {
      self.publish(message);
      return;
    };
    let Some(waiter) = self
      .pending
      .lock()
      .ok()
      .and_then(|mut pending| pending.remove(&id))
    else {
      return;
    };
    let result = message["error"].as_str().map_or_else(
      || Ok(message.get("result").cloned().unwrap_or(Value::Null)),
      |error| Err(error.into()),
    );
    let _ = waiter.send(result);
  }

  fn publish(&self, event: Event) {
    if let Ok(mut subscribers) = self.subscribers.lock() {
      if !self.closed.load(Ordering::SeqCst) {
        subscribers.retain(|sender| sender.send(event.clone()).is_ok());
      }
    }
  }
}

impl Drop for Electron {
  fn drop(&mut self) {
    self.shutdown();
  }
}

fn main_script(binary: &Path) -> Result<PathBuf, String> {
  if let Some(dir) = binary.parent() {
    let script = dir.join("main.cjs");
    if script.is_file() {
      return Ok(script);
    }
    #[cfg(target_os = "macos")]
    if let Some(app) = dir.parent().and_then(Path::parent) {
      if dir.file_name().is_some_and(|name| name == "MacOS")
        && dir
          .parent()
          .and_then(Path::file_name)
          .is_some_and(|name| name == "Contents")
        && app.extension().is_some_and(|extension| extension == "app")
      {
        if let Some(dist) = app.parent() {
          let script = dist.join("main.cjs");
          if script.is_file() {
            return Ok(script);
          }
        }
      }
    }
  }
  Err(format!(
    "staged Electron main.cjs not found beside {} or its .app distribution",
    binary.display()
  ))
}

fn staged_identity(binary: &Path, script: &Path) -> Result<crate::Metadata, String> {
  let root = script
    .parent()
    .ok_or("Electron main.cjs has no distribution directory")?;
  let marker = root.join("electron-rs.json");
  let bytes = match std::fs::read(&marker) {
    Ok(bytes) => bytes,
    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(crate::Metadata::default()),
    Err(error) => return Err(format!("{}: {error}", marker.display())),
  };
  let value: Value = serde_json::from_slice(&bytes)
    .map_err(|error| format!("invalid {}: {error}", marker.display()))?;
  // Reuse the staged marker's version, relative-path and canonical containment checks.
  let staged_binary = crate::metadata::binary(root, None)?;
  if std::fs::canonicalize(staged_binary).map_err(|error| error.to_string())? != binary {
    return Err(format!(
      "{} belongs to a different Electron binary",
      marker.display()
    ));
  }
  for key in ["appName", "identifier"] {
    if value
      .get(key)
      .is_some_and(|value| !value.is_null() && !value.is_string())
    {
      return Err(format!(
        "{}: {key} must be a string or null",
        marker.display()
      ));
    }
  }
  let identity = crate::Metadata {
    app_name: value["appName"].as_str().map(str::to_owned),
    identifier: value["identifier"].as_str().map(str::to_owned),
    ..crate::Metadata::default()
  };
  crate::metadata::validate(&identity)?;
  Ok(identity)
}

fn read_handshake(reader: &mut BufReader<TcpStream>, deadline: Instant) -> Result<String, String> {
  let mut line = Vec::new();
  loop {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
      return Err("Electron handshake timed out".into());
    }
    reader
      .get_ref()
      .set_read_timeout(Some(remaining))
      .map_err(|error| error.to_string())?;
    let buffer = reader
      .fill_buf()
      .map_err(|error| format!("Electron handshake: {error}"))?;
    if buffer.is_empty() {
      return Err("Electron closed the handshake socket".into());
    }
    let newline = buffer.iter().position(|byte| *byte == b'\n');
    let consumed = newline.map_or(buffer.len(), |index| index + 1);
    if line.len() + consumed > 64 * 1024 {
      return Err("Electron handshake exceeds 64 KiB".into());
    }
    line.extend_from_slice(&buffer[..consumed]);
    reader.consume(consumed);
    if newline.is_some() {
      break;
    }
  }
  let handshake: Value = serde_json::from_slice(&line)
    .map_err(|error| format!("invalid Electron handshake: {error}"))?;
  if handshake["event"].as_str() != Some("ready") {
    return Err(format!("unexpected Electron handshake: {handshake}"));
  }
  reader
    .get_ref()
    .set_read_timeout(None)
    .map_err(|error| error.to_string())?;
  Ok(
    handshake
      .pointer("/params/electron")
      .and_then(Value::as_str)
      .unwrap_or("unknown")
      .into(),
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn pipelined_handshake_preserves_events_and_cleans_up_rpc_waiters() {
    let timeout = Duration::from_secs(5);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    peer.set_read_timeout(Some(timeout)).unwrap();
    let stream = listener.accept().unwrap().0;
    peer
      .write_all(
        b"{\"event\":\"ready\",\"params\":{\"electron\":\"test\"}}\n{\"event\":\"early\"}\n",
      )
      .unwrap();
    let mut reader = BufReader::new(stream);
    let version = read_handshake(&mut reader, Instant::now() + timeout).unwrap();
    let (sender, receiver) = mpsc::channel();
    let electron = Arc::new(Electron {
      binary: PathBuf::new(),
      writer: Mutex::new(reader.get_ref().try_clone().unwrap()),
      pending: Mutex::new(HashMap::new()),
      events: Mutex::new(Some(receiver)),
      subscribers: Mutex::new(vec![sender]),
      next_id: AtomicU64::new(0),
      closed: AtomicBool::new(false),
      child: Mutex::new(None),
      version,
    });
    let (callback_tx, callback_rx) = mpsc::channel();
    electron.on(move |event| {
      let _ = callback_tx.send(event);
    });
    electron.start_reader(reader).unwrap();
    // Wait until the early event was dispatched before taking the initial receiver.
    assert_eq!(callback_rx.recv_timeout(timeout).unwrap()["event"], "early");
    let events = electron.events().unwrap();
    assert_eq!(events.recv_timeout(timeout).unwrap()["event"], "early");
    assert!(electron.events().is_err());
    assert_eq!(electron.version, "test");
    let future = electron.subscribe();
    assert!(future.try_recv().is_err());
    let peer = std::thread::spawn(move || {
      let mut reader = BufReader::new(peer);
      let mut line = String::new();
      for method in ["echo", "timeout", "disconnect"] {
        line.clear();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], method);
        if method == "echo" {
          writeln!(
            reader.get_mut(),
            "{}",
            json!({"id":request["id"],"result":42})
          )
          .unwrap();
          writeln!(reader.get_mut(), "{}", json!({"event":"future"})).unwrap();
        }
      }
      reader.get_ref().shutdown(Shutdown::Both).unwrap();
    });
    assert_eq!(electron.call("echo", Value::Null, timeout).unwrap(), 42);
    assert_eq!(future.recv_timeout(timeout).unwrap()["event"], "future");
    assert_eq!(
      callback_rx.recv_timeout(timeout).unwrap()["event"],
      "future"
    );
    assert!(electron
      .call("timeout", Value::Null, Duration::from_millis(30))
      .unwrap_err()
      .contains("did not answer"));
    assert!(electron.pending.lock().unwrap().is_empty());
    let deadline = Instant::now() + timeout;
    while Arc::strong_count(&electron) > 1 && Instant::now() < deadline {
      std::thread::yield_now();
    }
    assert_eq!(
      Arc::strong_count(&electron),
      1,
      "the blocked reader must not own Electron"
    );
    assert!(electron
      .call("disconnect", Value::Null, timeout)
      .unwrap_err()
      .contains("is gone"));
    peer.join().unwrap();
    assert!(electron.pending.lock().unwrap().is_empty());
    assert_eq!(events.recv_timeout(timeout).unwrap()["event"], "future");
    assert_eq!(events.recv_timeout(timeout).unwrap()["event"], "ipc.closed");
    assert!(matches!(
      events.recv_timeout(timeout),
      Err(RecvTimeoutError::Disconnected)
    ));
    assert_eq!(future.recv_timeout(timeout).unwrap()["event"], "ipc.closed");
    assert_eq!(
      callback_rx.recv_timeout(timeout).unwrap()["event"],
      "ipc.closed"
    );
    assert!(matches!(
      callback_rx.recv_timeout(timeout),
      Err(RecvTimeoutError::Disconnected)
    ));
    assert!(matches!(
      electron.subscribe().recv_timeout(timeout),
      Err(RecvTimeoutError::Disconnected)
    ));
    assert!(electron
      .call("after-close", Value::Null, timeout)
      .unwrap_err()
      .contains("is gone"));
    electron.shutdown();
  }
}
