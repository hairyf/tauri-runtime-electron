use std::{
  collections::HashMap,
  io::{self, BufRead, BufReader, Write},
  net::{Shutdown, TcpListener, TcpStream},
  path::{Path, PathBuf},
  process::{Child, Command, Stdio},
  sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, SyncSender, TrySendError},
    Arc, Mutex,
  },
  time::{Duration, Instant},
};

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::{command::{CommandRoute, Mode}, Error};

const EVENT_CAPACITY: usize = 1024;

/// An unsolicited notification from Electron.
pub type Event = Value;
type RpcResult = Result<Value, Error>;

/// A typed Electron request and its response.
pub trait RpcRequest: Serialize + Send + 'static {
  type Response: DeserializeOwned + Send + 'static;
  const METHOD: &'static str;

  fn method(&self) -> &'static str { Self::METHOD }
  fn timeout(&self) -> Duration { Duration::from_secs(5) }
  fn deferred(&self) -> bool { false }
  fn notification(&self) -> bool { false }
}

#[cfg(test)]
struct TestRequest {
  method: &'static str,
  params: Value,
  timeout: Duration,
  deferred: bool,
  notification: bool,
}

#[cfg(test)]
impl Serialize for TestRequest {
  fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    self.params.serialize(serializer)
  }
}

#[cfg(test)]
impl RpcRequest for TestRequest {
  type Response = Value;
  const METHOD: &'static str = "test";
  fn method(&self) -> &'static str { self.method }
  fn timeout(&self) -> Duration { self.timeout }
  fn deferred(&self) -> bool { self.deferred }
  fn notification(&self) -> bool { self.notification }
}

#[cfg(test)]
fn call(electron: &Electron, method: &'static str, params: Value, timeout: Duration, notification: bool) -> Result<Value, Error> {
  let request = TestRequest { method, params, timeout, deferred: false, notification };
  tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(electron.call(request))
}

type PendingSender = (String, Option<String>, oneshot::Sender<RpcResult>);

struct Subscribers {
  initial: Option<SyncSender<Event>>,
  future: Vec<SyncSender<Event>>,
}

/// A connected Electron process with synchronous RPC and event subscriptions.
pub struct Electron {
  binary: PathBuf,
  writer: Arc<Mutex<TcpStream>>,
  writer_order: Arc<tokio::sync::Mutex<()>>,
  shutdown_stream: TcpStream,
  pending: Mutex<HashMap<u64, PendingSender>>,
  command_routes: Mutex<HashMap<String, CommandRoute>>,
  events: Mutex<Option<Receiver<Event>>>,
  subscribers: Mutex<Subscribers>,
  next_id: AtomicU64,
  closed: AtomicBool,
  child: Mutex<Option<Child>>,
  /// Version reported by Electron during the handshake.
  pub version: String,
}

struct PendingGuard<'a> {
  electron: &'a Electron,
  id: u64,
}

impl Drop for PendingGuard<'_> {
  fn drop(&mut self) {
    self.electron.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&self.id);
  }
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
  pub async fn new(binary: impl AsRef<Path>, port: u16) -> Result<Arc<Self>, Error> {
    Self::with_args(binary, port, &[], Duration::from_secs(15)).await
  }

  /// Launch with app-specific switches and a deadline covering connection and handshake.
  /// The crate-owned main.cjs and preload.cjs must already be staged with the distribution.
  pub async fn with_args(
    binary: impl AsRef<Path>,
    port: u16,
    args: &[String],
    timeout: Duration,
  ) -> Result<Arc<Self>, Error> {
    let binary = binary.as_ref().to_path_buf();
    let args = args.to_vec();
    tokio::task::spawn_blocking(move || Self::start(binary, port, &args, timeout))
      .await
      .map_err(|error| Error::with_source("Electron startup task failed", error))?
  }

  fn start(
    binary: impl AsRef<Path>,
    port: u16,
    args: &[String],
    timeout: Duration,
  ) -> Result<Arc<Self>, Error> {
    if timeout.is_zero() {
      return Err("Electron startup timeout must be positive".into());
    }
    let binary = std::fs::canonicalize(binary.as_ref()).map_err(|error| {
      Error::with_source(
        format!("Electron binary {}", binary.as_ref().display()),
        error,
      )
    })?;
    let script = main_script(&binary)?;
    let identity = staged_identity(&binary, &script)?;
    let listener = TcpListener::bind(("127.0.0.1", port))
      .map_err(|error| Error::with_source("bind IPC", error))?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let token = handshake_token()?;
    let mut command = Command::new(&binary);
    command
      .stdin(Stdio::piped())
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
      .map_err(|error| Error::with_source(format!("spawn {}", binary.display()), error))?;
    let startup = (|| {
      // Only the child inherits this pipe. Never expose the credential in argv,
      // environment variables, files or diagnostics readable by another app.
      writeln!(child.stdin.take().ok_or("Electron stdin unavailable")?, "{token}")?;
      loop {
        if Instant::now() >= deadline {
          return Err(format!("Electron did not authenticate within {timeout:?}").into());
        }
        match listener.accept() {
          Ok((stream, _)) => {
            // Accepted sockets inherit nonblocking mode on Windows. An invalid
            // first client must neither become Electron nor prevent a real one.
            stream.set_nonblocking(false)?;
            let mut reader = BufReader::new(stream);
            let peer_deadline = deadline.min(Instant::now() + Duration::from_millis(500));
            if let Ok(version) = read_handshake(&mut reader, peer_deadline, &token) {
              let writer = reader.get_ref().try_clone()?;
              return Ok((reader, writer, version));
            }
          }
          Err(error)
            if matches!(
              error.kind(),
              io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) => {}
          Err(error) => return Err(Error::with_source("accept Electron IPC", error)),
        }
        if let Some(status) = child
          .try_wait()
          .map_err(|error| Error::with_source("poll Electron", error))?
        {
          return Err(
            format!(
              "Electron exited before authenticating ({status}): {}",
              binary.display()
            )
            .into(),
          );
        }
        std::thread::sleep(
          Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
        );
      }
    })();
    let (reader, writer, version) = match startup {
      Ok(connection) => connection,
      Err(error) => {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
      }
    };
    let shutdown_stream = match reader.get_ref().try_clone() {
      Ok(stream) => stream,
      Err(error) => {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
      }
    };
    let (event_tx, event_rx) = mpsc::sync_channel(EVENT_CAPACITY);
    let electron = Arc::new(Self {
      binary,
      writer: Arc::new(Mutex::new(writer)),
      writer_order: Arc::new(tokio::sync::Mutex::new(())),
      shutdown_stream,
      pending: Mutex::new(HashMap::new()),
      command_routes: Mutex::new(HashMap::new()),
      events: Mutex::new(Some(event_rx)),
      subscribers: Mutex::new(Subscribers {
        initial: Some(event_tx),
        future: Vec::new(),
      }),
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
  ///
  /// At most 1,024 events are buffered, even before this receiver is taken. An
  /// event arriving while full shuts Electron down to make protocol loss visible.
  /// Drain buffered events and handle channel disconnection: `ipc.closed` is a
  /// best-effort final notification and cannot be queued when the buffer is full.
  pub fn events(&self) -> Result<Receiver<Event>, Error> {
    self
      .events
      .lock()
      .map_err(|_| "events lock poisoned")?
      .take()
      .ok_or_else(|| "Electron events receiver already taken".into())
  }

  /// Subscribe to future events using a callback on a dedicated thread.
  ///
  /// Uses the same 1,024-event bound and overflow policy as [`Self::subscribe`].
  /// After disconnect or overflow, queued events drain before the callback is
  /// dropped. A blocked handler never blocks the RPC reader or shutdown.
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
  ///
  /// Each subscription buffers at most 1,024 events. If full, only that
  /// subscription closes, with a diagnostic on stderr; its buffered events
  /// remain available before `recv` reports disconnection. No overflow marker
  /// fits in a full queue, so consumers must handle channel closure. Other
  /// subscriptions and RPC continue. Normal `ipc.closed` is best effort.
  pub fn subscribe(&self) -> Receiver<Event> {
    let (sender, receiver) = mpsc::sync_channel(EVENT_CAPACITY);
    let mut subscribers = self
      .subscribers
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !self.closed.load(Ordering::SeqCst) {
      subscribers.future.push(sender);
    }
    receiver
  }

  /// Execute a typed RPC without blocking the calling executor.
  pub async fn call<R: RpcRequest>(&self, request: R) -> Result<R::Response, Error> {
    let method = request.method();
    let timeout = request.timeout();
    let deferred = request.deferred();
    let notification = request.notification();
    let params = serde_json::to_value(request)?;
    let value = self.rpc_value(method, params, timeout, deferred, notification).await?;
    Ok(serde_json::from_value(value)?)
  }

  async fn rpc_value(
    &self,
    method: &str,
    params: Value,
    timeout: Duration,
    deferred: bool,
    notification: bool,
  ) -> RpcResult {
    let started = Instant::now();
    let id = self.next_id.fetch_add(1, Ordering::Relaxed);
    let command_task = if method == "command.execute" { params["taskId"].as_str().map(str::to_owned) } else { None };
    let mut request = json!({"id": id, "method": method, "params": params});
    if deferred { request["asynchronous"] = json!(true); }
    if notification { request["notification"] = json!(true); }
    let mut line = serde_json::to_vec(&request)?;
    line.push(b'\n');
    if self.closed.load(Ordering::SeqCst) { return Err(Error::ChannelDisconnected); }
    if timeout.is_zero() { return Err(Error::IpcTimeout { timeout_duration: timeout }); }
    let deadline = started.checked_add(timeout).ok_or(Error::IpcTimeout { timeout_duration: timeout })?;
    if notification {
      self.write_line(line, deadline).await?;
      return Ok(Value::Null);
    }
    let (sender, receiver) = oneshot::channel();
    {
      let mut pending = self.pending.lock().map_err(|_| "pending lock poisoned")?;
      if self.closed.load(Ordering::SeqCst) { return Err(Error::ChannelDisconnected); }
      pending.insert(id, (method.to_owned(), command_task, sender));
    }
    let guard = PendingGuard { electron: self, id };
    if let Err(error) = self.write_line(line, deadline).await {
      drop(guard);
      self.shutdown();
      return Err(error);
    }
    let result = tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), receiver).await;
    drop(guard);
    match result {
      Ok(Ok(result)) => result,
      Ok(Err(_)) => Err(Error::ChannelDisconnected),
      Err(_) => Err(Error::IpcTimeout { timeout_duration: timeout }),
    }
  }

  async fn write_line(&self, line: Vec<u8>, deadline: Instant) -> Result<(), Error> {
    let timeout = deadline.saturating_duration_since(Instant::now());
    let order = tokio::time::timeout(timeout, Arc::clone(&self.writer_order).lock_owned()).await
      .map_err(|_| Error::IpcTimeout { timeout_duration: timeout })?;
    let timeout = deadline.saturating_duration_since(Instant::now());
    if timeout.is_zero() { return Err(Error::IpcTimeout { timeout_duration: timeout }); }
    let writer = Arc::clone(&self.writer);
    let task = tokio::task::spawn_blocking(move || {
      let _order = order;
      writer.lock().map_err(|_| Error::from("writer lock poisoned"))
        .and_then(|mut writer| writer.set_write_timeout(Some(timeout)).and_then(|()| writer.write_all(&line)).map_err(Error::from))
    });
    tokio::time::timeout(timeout, task).await.map_err(|_| Error::IpcTimeout { timeout_duration: timeout })?
      .map_err(|error| Error::with_source("Electron RPC writer failed", error))?
  }

  /// Run this Electron binary in Node mode.
  ///
  /// Nonzero exits return `Ok(Output)`, preserving status, stdout, and stderr.
  /// Only process creation or output collection failures return an I/O error.
  pub fn node(&self, args: &[&str]) -> io::Result<std::process::Output> {
    crate::node(&self.binary, args)
  }

  /// Run this binary in Electron mode in a separate process, without an RPC bridge.
  /// Clears `ELECTRON_RUN_AS_NODE`; nonzero exits return `Ok(Output)`.
  pub fn electron(&self, args: &[&str]) -> io::Result<std::process::Output> {
    crate::electron(&self.binary, args)
  }

  /// Run an owned async source body in this connection's shared main process.
  /// The body receives `args` and `signal`; await the builder to execute it.
  pub fn node_in_main(self: &Arc<Self>, script: &str, args: &[&str]) -> crate::Command {
    crate::Command::new(Arc::clone(self), Mode::Node, script, args)
  }

  /// Run an owned async source body with Electron access in the shared main process.
  /// The body receives `args` and `signal`; await the builder to execute it.
  pub fn electron_in_main(self: &Arc<Self>, script: &str, args: &[&str]) -> crate::Command {
    crate::Command::new(Arc::clone(self), Mode::Electron, script, args)
  }

  pub(crate) fn command_task_id(&self) -> Result<String, Error> {
    let id = self.next_id.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
      .map_err(|_| Error::from("command task ID space exhausted"))?;
    Ok(format!("command-{id}"))
  }

  pub(crate) fn register_command(&self, task_id: String, route: CommandRoute) -> Result<(), Error> {
    let mut routes = self.command_routes.lock().map_err(|_| "command routes lock poisoned")?;
    if self.closed.load(Ordering::SeqCst) { return Err(Error::ChannelDisconnected); }
    match routes.entry(task_id) {
      std::collections::hash_map::Entry::Vacant(entry) => { entry.insert(route); Ok(()) }
      std::collections::hash_map::Entry::Occupied(_) => Err("duplicate command task ID".into()),
    }
  }

  pub(crate) fn remove_command(&self, task_id: &str) {
    self.command_routes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(task_id);
  }

  fn command_output(&self, params: Value) {
    let mut routes = self.command_routes.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(task_id) = params["taskId"].as_str().map(str::to_owned) else {
      for (_, mut route) in routes.drain() {
        route.fail(Error::Rpc { method: "command.execute".into(), message: "command.output missing string taskId".into() });
      }
      return;
    };
    // Unknown IDs are late output from cancelled/completed commands, never events.
    if let Some(route) = routes.get_mut(&task_id) {
      if !route.dispatch(params) { routes.remove(&task_id); }
    }
  }

  /// Close IPC, wake pending calls, close subscriptions, and terminate/reap Electron.
  pub fn shutdown(&self) {
    self.disconnect();
    let _ = self.shutdown_stream.shutdown(Shutdown::Both);
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

  fn start_reader(self: &Arc<Self>, mut reader: BufReader<TcpStream>) -> Result<(), Error> {
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
      .map_err(|error| Error::with_source("start Electron IPC reader", error))
  }

  fn disconnect(&self) {
    if self.closed.swap(true, Ordering::SeqCst) {
      return;
    }
    for (_, mut route) in self.command_routes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).drain() {
      route.fail(Error::ChannelDisconnected);
    }
    let pending = std::mem::take(&mut *self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
    for (_, (_, _, sender)) in pending { let _ = sender.send(Err(Error::ChannelDisconnected)); }
    let mut subscribers = self
      .subscribers
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());
    for sender in subscribers
      .initial
      .take()
      .into_iter()
      .chain(subscribers.future.drain(..))
    {
      let _ = sender.try_send(json!({"event":"ipc.closed"}));
    }
  }

  fn dispatch(&self, message: Value) {
    if message["event"] == "command.output" || message["method"] == "command.output" {
      self.command_output(message.get("params").cloned().unwrap_or(Value::Null));
      return;
    }
    let Some(id) = message["id"].as_u64() else {
      self.publish(message);
      return;
    };
    let Some((method, command_task, waiter)) = self
      .pending
      .lock()
      .ok()
      .and_then(|mut pending| pending.remove(&id))
    else {
      return;
    };
    // The wire guarantees output is flushed before this reply. Closing the
    // route now seals its queue and prevents a later disconnect overriding it.
    if let Some(task_id) = command_task { self.remove_command(&task_id); }
    let result = message["error"].as_str().map_or_else(
      || Ok(message.get("result").cloned().unwrap_or(Value::Null)),
      |error| Err(Error::Rpc { method, message: error.into() }),
    );
    let _ = waiter.send(result);
  }

  fn publish(&self, event: Event) {
    let mut subscribers = self
      .subscribers
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());
    if self.closed.load(Ordering::SeqCst) {
      return;
    }
    if let Some(initial) = &subscribers.initial {
      match initial.try_send(event.clone()) {
        Ok(()) => {}
        Err(TrySendError::Disconnected(_)) => subscribers.initial = None,
        Err(TrySendError::Full(_)) => {
          // Shutdown takes this lock again; release it before closing IPC.
          drop(subscribers);
          self.shutdown();
          eprintln!("[electron] initial event queue exceeded {EVENT_CAPACITY} events; closed IPC");
          return;
        }
      }
    }
    let mut overflowed = 0;
    subscribers
      .future
      .retain(|sender| match sender.try_send(event.clone()) {
        Ok(()) => true,
        Err(TrySendError::Disconnected(_)) => false,
        Err(TrySendError::Full(_)) => {
          overflowed += 1;
          false
        }
      });
    drop(subscribers);
    if overflowed > 0 {
      eprintln!(
        "[electron] closed {overflowed} event subscriptions exceeding {EVENT_CAPACITY} queued events"
      );
    }
  }
}

impl Drop for Electron {
  fn drop(&mut self) {
    self.shutdown();
  }
}

fn main_script(binary: &Path) -> Result<PathBuf, Error> {
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
  Err(
    format!(
      "staged Electron main.cjs not found beside {} or its .app distribution",
      binary.display()
    )
    .into(),
  )
}

fn staged_identity(binary: &Path, script: &Path) -> Result<crate::Metadata, Error> {
  let root = script
    .parent()
    .ok_or("Electron main.cjs has no distribution directory")?;
  let marker = root.join("electron-rs.json");
  let bytes = match std::fs::read(&marker) {
    Ok(bytes) => bytes,
    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(crate::Metadata::default()),
    Err(error) => return Err(Error::with_source(marker.display().to_string(), error)),
  };
  let value: Value = serde_json::from_slice(&bytes)
    .map_err(|error| Error::with_source(format!("invalid {}", marker.display()), error))?;
  // Reuse the staged marker's version, relative-path and canonical containment checks.
  let staged_binary = crate::metadata::binary(root, None)?;
  if std::fs::canonicalize(staged_binary)? != binary {
    return Err(
      format!(
        "{} belongs to a different Electron binary",
        marker.display()
      )
      .into(),
    );
  }
  for key in ["appName", "identifier"] {
    if value
      .get(key)
      .is_some_and(|value| !value.is_null() && !value.is_string())
    {
      return Err(format!("{}: {key} must be a string or null", marker.display()).into());
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

fn handshake_token() -> Result<String, Error> {
  let mut bytes = [0u8; 32];
  #[cfg(unix)]
  {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
  }
  #[cfg(windows)]
  unsafe {
    use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG)
      .ok().map_err(|error| Error::with_source("generate Electron IPC credential", error))?;
  }
  Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn read_handshake(reader: &mut BufReader<TcpStream>, deadline: Instant, token: &str) -> Result<String, Error> {
  let mut line = Vec::new();
  loop {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
      return Err("Electron handshake timed out".into());
    }
    reader.get_ref().set_read_timeout(Some(remaining))?;
    let buffer = reader
      .fill_buf()
      .map_err(|error| Error::with_source("Electron handshake", error))?;
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
    .map_err(|error| Error::with_source("invalid Electron handshake", error))?;
  let supplied = handshake.pointer("/params/token").and_then(Value::as_str).unwrap_or("");
  // Fixed-size comparison: neither prefix matching nor an early mismatch exit.
  let authenticated = supplied.len() == token.len() && supplied.bytes().zip(token.bytes()).fold(0, |diff, (a, b)| diff | (a ^ b)) == 0;
  if !authenticated || handshake["event"].as_str() != Some("ready") {
    return Err("unauthenticated Electron handshake".into());
  }
  reader.get_ref().set_read_timeout(None)?;
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
  use std::{error::Error as _, sync::mpsc::RecvTimeoutError};

  const WAIT: Duration = Duration::from_secs(5);
  const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
  const READY: &[u8] = b"{\"event\":\"ready\",\"params\":{\"electron\":\"test\",\"token\":\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"}}\n";

  fn loopback(handshake: &[u8]) -> (Arc<Electron>, TcpStream, BufReader<TcpStream>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    peer.set_read_timeout(Some(WAIT)).unwrap();
    peer.set_write_timeout(Some(WAIT)).unwrap();
    let stream = listener.accept().unwrap().0;
    peer.write_all(handshake).unwrap();
    let mut reader = BufReader::new(stream);
    let version = read_handshake(&mut reader, Instant::now() + WAIT, TOKEN).unwrap();
    let (sender, receiver) = mpsc::sync_channel(EVENT_CAPACITY);
    let electron = Arc::new(Electron {
      binary: PathBuf::new(),
      writer: Arc::new(Mutex::new(reader.get_ref().try_clone().unwrap())),
      writer_order: Arc::new(tokio::sync::Mutex::new(())),
      shutdown_stream: reader.get_ref().try_clone().unwrap(),
      pending: Mutex::new(HashMap::new()),
      command_routes: Mutex::new(HashMap::new()),
      events: Mutex::new(Some(receiver)),
      subscribers: Mutex::new(Subscribers {
        initial: Some(sender),
        future: Vec::new(),
      }),
      next_id: AtomicU64::new(0),
      closed: AtomicBool::new(false),
      child: Mutex::new(None),
      version,
    });
    (electron, peer, reader)
  }

  #[tokio::test]
  async fn shared_main_streams_owned_bytes_before_completion_and_survives_final_disconnect() {
    use std::future::IntoFuture as _;
    let (electron, peer, reader) = loopback(READY);
    electron.start_reader(reader).unwrap();
    let events = electron.events().unwrap();
    let peer = std::thread::spawn(move || {
      let mut reader = BufReader::new(peer);
      let mut line = String::new();
      reader.read_line(&mut line).unwrap();
      let request: Value = serde_json::from_str(&line).unwrap();
      assert_eq!(request["method"], "command.execute");
      let params = &request["params"];
      assert!(params["taskId"].is_string());
      assert_eq!(params["mode"], "node");
      assert_eq!(params["script"], "return args[0]");
      assert_eq!(params["args"], json!(["owned argument"]));
      assert_eq!(params["timeoutMs"], 30_000);
      assert_eq!(params["maxOutputBytes"], 16 * 1024 * 1024);
      assert_eq!(params["stream"], true);
      writeln!(reader.get_mut(), "{}", json!({"event":"command.output","params":{
        "taskId":params["taskId"],"chunks":[
          {"sequence":0,"stream":"stdout","data":"AP8="},
          {"sequence":1,"stream":"stderr","data":"gA=="},
          {"sequence":2,"stream":"stdout","data":"YQ=="}
        ]
      }})).unwrap();
      writeln!(reader.get_mut(), "{}", json!({"id":request["id"],"result":{
        "code":23,"stdout":"AP9h","stderr":"gA==","result":{"answer":42}
      }})).unwrap();
      reader.get_ref().shutdown(Shutdown::Both).unwrap();
    });
    let command = {
      let script = String::from("return args[0]");
      let arg = String::from("owned argument");
      electron.node_in_main(&script, &[&arg])
    };
    let (chunks, received) = mpsc::channel();
    let future = command.on_output(move |chunk| chunks.send(chunk).unwrap()).into_future();
    fn assert_send_static<T: Send + 'static>(_: &T) {}
    assert_send_static(&future);
    let output = future.await.unwrap();
    peer.join().unwrap();
    let received: Vec<_> = received.try_iter().collect();
    assert_eq!(received.iter().map(|chunk| chunk.sequence).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(received[0].data, [0, 255]);
    assert_eq!(output.stdout, [0, 255, b'a']);
    assert_eq!(output.stderr, [128]);
    assert_eq!(output.result, json!({"answer":42}));
    assert_eq!(output.code, 23);
    assert!(electron.command_routes.lock().unwrap().is_empty());
    assert!(events.try_iter().all(|event| event["event"] == "ipc.closed"));
  }

  #[test]
  fn command_output_bypasses_all_generic_event_queues_even_after_cancellation() {
    let (electron, _peer, _reader) = loopback(READY);
    let events = electron.events().unwrap();
    let subscriber = electron.subscribe();
    for _ in 0..EVENT_CAPACITY + 1 {
      for envelope in ["event", "method"] {
        let mut event = json!({"params":{"taskId":"cancelled-command","chunks":[]}});
        event[envelope] = json!("command.output");
        electron.dispatch(event);
      }
    }
    assert!(events.try_recv().is_err());
    assert!(subscriber.try_recv().is_err());
    assert!(!electron.closed.load(Ordering::SeqCst));
  }

  #[tokio::test]
  async fn dropping_pending_command_removes_routes_and_posts_typed_cancel() {
    use std::future::IntoFuture as _;
    let (electron, peer, reader) = loopback(READY);
    electron.start_reader(reader).unwrap();
    let (executed, received) = oneshot::channel();
    let (cancelled, cancellation) = oneshot::channel();
    let peer = std::thread::spawn(move || {
      let mut reader = BufReader::new(peer);
      let mut line = String::new();
      reader.read_line(&mut line).unwrap();
      let request: Value = serde_json::from_str(&line).unwrap();
      assert_eq!(request["method"], "command.execute");
      assert_eq!(request["params"]["mode"], "electron");
      assert_eq!(request["params"]["stream"], false);
      assert_eq!(request["params"]["timeoutMs"], 1000);
      assert_eq!(request["params"]["maxOutputBytes"], 32);
      executed.send(()).unwrap();
      line.clear();
      reader.read_line(&mut line).unwrap();
      let cancel: Value = serde_json::from_str(&line).unwrap();
      assert_eq!(cancel["method"], "command.cancel");
      assert_eq!(cancel["notification"], true);
      assert_eq!(cancel["params"]["taskId"], request["params"]["taskId"]);
      cancelled.send(()).unwrap();
      reader
    });
    let future = electron.electron_in_main("await new Promise(() => {})", &[])
      .timeout(Duration::from_secs(1)).max_output_bytes(32).into_future();
    let mut future = Box::pin(future);
    tokio::select! {
      result = &mut future => panic!("command unexpectedly completed: {result:?}"),
      received = received => received.unwrap(),
    }
    assert_eq!(electron.command_routes.lock().unwrap().len(), 1);
    drop(future);
    assert!(electron.command_routes.lock().unwrap().is_empty());
    assert!(electron.pending.lock().unwrap().is_empty());
    tokio::time::timeout(WAIT, cancellation).await.unwrap().unwrap();
    let _peer = peer.join().unwrap();
  }

  #[tokio::test]
  async fn shared_main_validates_limits_and_rejects_incomplete_streams() {
    let (electron, peer, reader) = loopback(READY);
    assert!(electron.node_in_main("", &[]).timeout(Duration::ZERO).await.is_err());
    assert!(electron.node_in_main("", &[]).max_output_bytes(0).await.is_err());
    assert!(electron.node_in_main("", &[]).max_output_bytes(256 * 1024 * 1024 + 1).await.is_err());
    assert!(electron.command_routes.lock().unwrap().is_empty());
    electron.start_reader(reader).unwrap();
    let peer = std::thread::spawn(move || {
      let mut reader = BufReader::new(peer);
      let mut line = String::new();
      reader.read_line(&mut line).unwrap();
      let request: Value = serde_json::from_str(&line).unwrap();
      writeln!(reader.get_mut(), "{}", json!({"id":request["id"],"result":{
        "code":0,"stdout":"YQ==","stderr":"","result":null
      }})).unwrap();
      line.clear();
      reader.read_line(&mut line).unwrap();
      let cancel: Value = serde_json::from_str(&line).unwrap();
      assert_eq!(cancel["method"], "command.cancel");
      assert_eq!(cancel["params"]["taskId"], request["params"]["taskId"]);
      reader
    });
    let error = electron.node_in_main("", &[]).on_output(|_| {}).await.unwrap_err();
    assert!(error.to_string().contains("do not match"));
    assert!(electron.command_routes.lock().unwrap().is_empty());
    // Let the Drop guard's best-effort cancellation run on this current runtime.
    while !peer.is_finished() { tokio::task::yield_now().await; }
    let _peer = peer.join().unwrap();
  }

  #[tokio::test]
  async fn command_disconnect_removes_routes_and_wakes_the_future() {
    let (electron, peer, reader) = loopback(READY);
    electron.start_reader(reader).unwrap();
    let peer = std::thread::spawn(move || {
      let mut reader = BufReader::new(peer);
      let mut line = String::new();
      reader.read_line(&mut line).unwrap();
      reader.get_ref().shutdown(Shutdown::Both).unwrap();
    });
    assert!(matches!(electron.node_in_main("", &[]).await, Err(Error::ChannelDisconnected)));
    peer.join().unwrap();
    assert!(electron.command_routes.lock().unwrap().is_empty());
    assert!(electron.pending.lock().unwrap().is_empty());
  }

  #[tokio::test]
  async fn cancelled_rpc_removes_its_pending_waiter() {
    let (electron, _peer, _reader) = loopback(READY);
    let _order = electron.writer_order.lock().await;
    let mut request = Box::pin(electron.rpc_value("cancel", Value::Null, WAIT, false, false));
    assert!(std::future::poll_fn(|cx| {
      std::task::Poll::Ready(std::future::Future::poll(request.as_mut(), cx))
    }).await.is_pending());
    assert_eq!(electron.pending.lock().unwrap().len(), 1);
    drop(request);
    assert!(electron.pending.lock().unwrap().is_empty());
  }

  #[test]
  fn pipelined_handshake_preserves_events_and_cleans_up_rpc_waiters() {
    let (electron, peer, reader) = loopback(
      &[READY, b"{\"event\":\"early\"}\n"].concat(),
    );
    let early = electron.subscribe();
    electron.start_reader(reader).unwrap();
    // Wait until the early event was dispatched before taking the initial receiver.
    assert_eq!(early.recv_timeout(WAIT).unwrap()["event"], "early");
    drop(early);
    let events = electron.events().unwrap();
    assert_eq!(events.recv_timeout(WAIT).unwrap()["event"], "early");
    assert!(electron.events().is_err());
    assert_eq!(electron.version, "test");
    let future = electron.subscribe();
    let (callback_tx, callback_rx) = mpsc::channel();
    electron.on(move |event| {
      let _ = callback_tx.send(event);
    });
    assert!(future.try_recv().is_err());
    assert!(callback_rx.try_recv().is_err());
    let peer = std::thread::spawn(move || {
      let mut reader = BufReader::new(peer);
      let mut line = String::new();
      for method in ["notify", "echo", "error", "timeout", "disconnect"] {
        line.clear();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], method);
        assert_eq!(request["notification"].as_bool().unwrap_or(false), method == "notify");
        if method == "notify" {
          // A reply to a posted call has no waiter and must be dropped.
          writeln!(
            reader.get_mut(),
            "{}",
            json!({"id":request["id"],"result":null})
          )
          .unwrap();
        }
        if method == "echo" {
          writeln!(
            reader.get_mut(),
            "{}",
            json!({"id":request["id"],"result":42})
          )
          .unwrap();
          writeln!(reader.get_mut(), "{}", json!({"event":"future"})).unwrap();
        }
        if method == "error" {
          writeln!(
            reader.get_mut(),
            "{}",
            json!({"id":request["id"],"error":"remote RPC error"})
          )
          .unwrap();
        }
      }
      reader.get_ref().shutdown(Shutdown::Both).unwrap();
    });
    call(&electron, "notify", Value::Null, WAIT, true).unwrap();
    assert!(
      electron.pending.lock().unwrap().is_empty(),
      "a posted call must not register a waiter"
    );
    assert_eq!(call(&electron, "echo", Value::Null, WAIT, false).unwrap(), 42);
    assert_eq!(future.recv_timeout(WAIT).unwrap()["event"], "future");
    assert_eq!(callback_rx.recv_timeout(WAIT).unwrap()["event"], "future");
    let error = call(&electron, "error", Value::Null, WAIT, false).unwrap_err();
    assert!(matches!(&error, Error::Rpc { method, message } if method == "error" && message == "remote RPC error"));
    assert!(error.source().is_none());
    assert!(matches!(
      call(&electron, "timeout", Value::Null, Duration::from_millis(30), false).unwrap_err(),
      Error::IpcTimeout { timeout_duration } if timeout_duration == Duration::from_millis(30)
    ));
    assert!(electron.pending.lock().unwrap().is_empty());
    let deadline = Instant::now() + WAIT;
    while Arc::strong_count(&electron) > 1 && Instant::now() < deadline {
      std::thread::yield_now();
    }
    assert_eq!(
      Arc::strong_count(&electron),
      1,
      "the blocked reader must not own Electron"
    );
    assert!(matches!(
      call(&electron, "disconnect", Value::Null, WAIT, false).unwrap_err(),
      Error::ChannelDisconnected
    ));
    peer.join().unwrap();
    assert!(electron.pending.lock().unwrap().is_empty());
    assert_eq!(events.recv_timeout(WAIT).unwrap()["event"], "future");
    assert_eq!(events.recv_timeout(WAIT).unwrap()["event"], "ipc.closed");
    assert!(matches!(
      events.recv_timeout(WAIT),
      Err(RecvTimeoutError::Disconnected)
    ));
    assert_eq!(future.recv_timeout(WAIT).unwrap()["event"], "ipc.closed");
    assert_eq!(
      callback_rx.recv_timeout(WAIT).unwrap()["event"],
      "ipc.closed"
    );
    assert!(matches!(
      callback_rx.recv_timeout(WAIT),
      Err(RecvTimeoutError::Disconnected)
    ));
    assert!(matches!(
      electron.subscribe().recv_timeout(WAIT),
      Err(RecvTimeoutError::Disconnected)
    ));
    assert!(matches!(
      call(&electron, "after-close", Value::Null, WAIT, false).unwrap_err(),
      Error::ChannelDisconnected
    ));
    electron.shutdown();
  }

  #[test]
  fn unclaimed_initial_queue_overflow_closes_ipc_and_wakes_rpc() {
    let (electron, peer, reader) = loopback(READY);
    electron.start_reader(reader).unwrap();
    let peer = std::thread::spawn(move || {
      let mut reader = BufReader::new(peer);
      let mut line = String::new();
      assert!(reader.read_line(&mut line).unwrap() > 0);
      let mut burst = Vec::new();
      for index in 0..=EVENT_CAPACITY {
        writeln!(&mut burst, "{}", json!({"event":"queued","index":index})).unwrap();
      }
      reader.get_mut().write_all(&burst).unwrap();
      line.clear();
      assert_eq!(reader.read_line(&mut line).unwrap(), 0);
    });
    let error = call(&electron, "overflow", Value::Null, WAIT, false).unwrap_err();
    assert!(matches!(error, Error::ChannelDisconnected), "{error}");
    peer.join().unwrap();
    assert!(electron.closed.load(Ordering::SeqCst));
    assert!(electron.pending.lock().unwrap().is_empty());
    let events = electron.events().unwrap();
    for index in 0..EVENT_CAPACITY {
      assert_eq!(events.recv_timeout(WAIT).unwrap()["index"], index);
    }
    assert!(matches!(
      events.recv_timeout(WAIT),
      Err(RecvTimeoutError::Disconnected)
    ));
  }

  #[test]
  fn slow_subscriptions_close_without_blocking_primary_events_or_rpc() {
    let (electron, peer, reader) = loopback(READY);
    let events = electron.events().unwrap();
    let slow = electron.subscribe();
    let (callback_tx, callback_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    electron.on(move |event| {
      let first = event["index"] == 0;
      callback_tx.send(event).unwrap();
      if first {
        release_rx.recv_timeout(WAIT).unwrap();
      }
    });
    drop(electron.subscribe());
    electron.start_reader(reader).unwrap();
    let peer = std::thread::spawn(move || {
      let mut reader = BufReader::new(peer);
      let mut line = String::new();
      for method in ["fill", "overflow", "callback-overflow", "after"] {
        line.clear();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], method);
        let mut burst = Vec::new();
        if method == "fill" {
          for index in 0..EVENT_CAPACITY {
            writeln!(&mut burst, "{}", json!({"event":"queued","index":index})).unwrap();
          }
        } else if method == "overflow" || method == "callback-overflow" {
          let index = if method == "overflow" {
            EVENT_CAPACITY
          } else {
            EVENT_CAPACITY + 1
          };
          writeln!(&mut burst, "{}", json!({"event":"queued","index":index})).unwrap();
        }
        reader.get_mut().write_all(&burst).unwrap();
        writeln!(
          reader.get_mut(),
          "{}",
          json!({"id":request["id"],"result":42})
        )
        .unwrap();
      }
      line.clear();
      assert_eq!(reader.read_line(&mut line).unwrap(), 0);
    });
    assert_eq!(call(&electron, "fill", Value::Null, WAIT, false).unwrap(), 42);
    assert_eq!(callback_rx.recv_timeout(WAIT).unwrap()["index"], 0);
    assert_eq!(electron.subscribers.lock().unwrap().future.len(), 2);
    for index in 0..EVENT_CAPACITY {
      assert_eq!(events.recv_timeout(WAIT).unwrap()["index"], index);
    }
    assert_eq!(call(&electron, "overflow", Value::Null, WAIT, false).unwrap(), 42);
    assert_eq!(events.recv_timeout(WAIT).unwrap()["index"], EVENT_CAPACITY);
    for index in 0..EVENT_CAPACITY {
      assert_eq!(slow.recv_timeout(WAIT).unwrap()["index"], index);
    }
    assert!(matches!(
      slow.recv_timeout(WAIT),
      Err(RecvTimeoutError::Disconnected)
    ));
    assert_eq!(
      call(&electron, "callback-overflow", Value::Null, WAIT, false).unwrap(),
      42
    );
    assert_eq!(
      events.recv_timeout(WAIT).unwrap()["index"],
      EVENT_CAPACITY + 1
    );
    assert!(electron.subscribers.lock().unwrap().future.is_empty());
    release_tx.send(()).unwrap();
    for index in 1..=EVENT_CAPACITY {
      assert_eq!(callback_rx.recv_timeout(WAIT).unwrap()["index"], index);
    }
    assert!(matches!(
      callback_rx.recv_timeout(WAIT),
      Err(RecvTimeoutError::Disconnected)
    ));
    assert!(!electron.closed.load(Ordering::SeqCst));
    assert_eq!(call(&electron, "after", Value::Null, WAIT, false).unwrap(), 42);
    electron.shutdown();
    peer.join().unwrap();
  }

  #[test]
  fn dropped_receivers_are_pruned_without_closing_ipc() {
    let (electron, mut peer, reader) = loopback(READY);
    drop(electron.events().unwrap());
    drop(electron.subscribe());
    let live = electron.subscribe();
    electron.start_reader(reader).unwrap();
    writeln!(peer, "{}", json!({"event":"live"})).unwrap();
    assert_eq!(live.recv_timeout(WAIT).unwrap()["event"], "live");
    {
      let subscribers = electron.subscribers.lock().unwrap();
      assert!(subscribers.initial.is_none());
      assert_eq!(subscribers.future.len(), 1);
    }
    assert!(!electron.closed.load(Ordering::SeqCst));
    electron.shutdown();
  }

  #[test]
  fn shutdown_with_full_queues_drops_senders_without_waiting_for_consumers() {
    let (electron, peer, reader) = loopback(READY);
    let events = electron.events().unwrap();
    let future = electron.subscribe();
    electron.start_reader(reader).unwrap();
    let peer = std::thread::spawn(move || {
      let mut reader = BufReader::new(peer);
      let mut line = String::new();
      assert!(reader.read_line(&mut line).unwrap() > 0);
      let request: Value = serde_json::from_str(&line).unwrap();
      let mut burst = Vec::new();
      for index in 0..EVENT_CAPACITY {
        writeln!(&mut burst, "{}", json!({"event":"queued","index":index})).unwrap();
      }
      writeln!(&mut burst, "{}", json!({"id":request["id"],"result":null})).unwrap();
      reader.get_mut().write_all(&burst).unwrap();
      line.clear();
      assert_eq!(reader.read_line(&mut line).unwrap(), 0);
    });
    // The reply is dispatched after every event, so both queues are now full.
    call(&electron, "barrier", Value::Null, WAIT, false).unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let closing = Arc::clone(&electron);
    let shutdown = std::thread::spawn(move || {
      closing.shutdown();
      let _ = done_tx.send(());
    });
    let completed = done_rx.recv_timeout(WAIT);
    let initial_events = events.try_iter().collect::<Vec<_>>();
    let initial_closed = matches!(events.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    let future_events = future.try_iter().collect::<Vec<_>>();
    let future_closed = matches!(future.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    // Also release a regressed blocking sender before asserting the deadline.
    drop(events);
    drop(future);
    shutdown.join().unwrap();
    peer.join().unwrap();
    assert!(completed.is_ok(), "shutdown waited for full event queues");
    assert_eq!(initial_events.len(), EVENT_CAPACITY);
    assert_eq!(future_events, initial_events);
    assert!(initial_closed && future_closed);
  }

  #[test]
  fn rpc_write_errors_preserve_the_io_source() {
    for posted in [false, true] {
      let (electron, _peer, _reader) = loopback(READY);
      electron
        .writer
        .lock()
        .unwrap()
        .shutdown(Shutdown::Write)
        .unwrap();
      let error = call(&electron, "write", Value::Null, WAIT, posted).unwrap_err();
      assert!(matches!(&error, Error::Io(_)), "{error}");
      assert!(error.source().is_some_and(|cause| cause.is::<io::Error>()));
      assert!(electron.pending.lock().unwrap().is_empty());
    }
  }

  #[test]
  fn handshake_rejects_unauthenticated_clients_and_keeps_the_credential_private() {
    for params in [json!({"electron":"spoof"}), json!({"electron":"spoof","token":"wrong"}), json!({"electron":"spoof","token":format!("{TOKEN}extra")})] {
      let (_electron, mut peer, mut reader) = loopback(READY);
      writeln!(peer, "{}", json!({"event":"ready","params":params})).unwrap();
      let error = read_handshake(&mut reader, Instant::now() + WAIT, TOKEN).unwrap_err();
      assert_eq!(error.to_string(), "unauthenticated Electron handshake");
      assert!(!error.to_string().contains(TOKEN));
    }
    let token = handshake_token().unwrap();
    assert_eq!(token.len(), 64);
    assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_ne!(token, handshake_token().unwrap());
  }

  #[test]
  fn invalid_handshake_preserves_the_json_source() {
    let (_electron, mut peer, mut reader) = loopback(READY);
    peer.write_all(b"invalid JSON\n").unwrap();
    let error = read_handshake(&mut reader, Instant::now() + WAIT, TOKEN).unwrap_err();
    assert!(error.to_string().contains("invalid Electron handshake"));
    assert!(error
      .source()
      .is_some_and(|cause| cause.is::<serde_json::Error>()));
  }
}
