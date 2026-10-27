use std::{
  future::{Future, IntoFuture},
  panic::{catch_unwind, AssertUnwindSafe},
  pin::Pin,
  sync::Arc,
  time::Duration,
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::{Electron, Error, RpcRequest};

const DEFAULT_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 256 * 1024 * 1024;
const OUTPUT_QUEUE_CAPACITY: usize = 256;
const RESPONSE_GRACE: Duration = Duration::from_secs(1);

/// Captured raw output and the JSON value returned by a shared-main script.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandOutput {
  pub code: i32,
  pub stdout: Vec<u8>,
  pub stderr: Vec<u8>,
  pub result: Value,
}

impl CommandOutput {
  /// Whether the script exited with code zero.
  pub fn success(&self) -> bool { self.code == 0 }
}

/// The raw stream to which a command wrote a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputStream {
  Stdout,
  Stderr,
}

/// One raw output chunk, ordered across both streams starting at sequence zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandChunk {
  pub sequence: u64,
  pub stream: OutputStream,
  pub data: Vec<u8>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode { Node, Electron }

type OutputCallback = Box<dyn FnMut(CommandChunk) + Send + 'static>;

/// An owned shared-main script invocation. Awaiting starts execution.
///
/// The async source body receives `args` and `signal`. Defaults are 30 seconds
/// and 16 MiB of combined stdout/stderr. Dropping its pending future cancels it
/// best effort on the current Tokio runtime; the server deadline is the fallback.
#[must_use = "await the command to execute it"]
pub struct Command {
  electron: Arc<Electron>,
  mode: Mode,
  script: String,
  args: Vec<String>,
  timeout: Duration,
  max_output_bytes: usize,
  on_output: Option<OutputCallback>,
}

impl Command {
  pub(crate) fn new(electron: Arc<Electron>, mode: Mode, script: &str, args: &[&str]) -> Self {
    Self {
      electron,
      mode,
      script: script.to_owned(),
      args: args.iter().map(|arg| (*arg).to_owned()).collect(),
      timeout: Duration::from_secs(30),
      max_output_bytes: DEFAULT_OUTPUT_BYTES,
      on_output: None,
    }
  }

  /// Receive raw chunks on the awaiting task, before command completion.
  /// Slow callbacks can exhaust the bounded queue and fail/cancel the command.
  pub fn on_output(mut self, callback: impl FnMut(CommandChunk) + Send + 'static) -> Self {
    self.on_output = Some(Box::new(callback));
    self
  }

  /// Set a deadline of 1..=2,147,483,647 whole milliseconds, validated on await.
  pub fn timeout(mut self, timeout: Duration) -> Self { self.timeout = timeout; self }

  /// Limit combined raw stdout/stderr to 1..=256 MiB, validated on await.
  pub fn max_output_bytes(mut self, bytes: usize) -> Self { self.max_output_bytes = bytes; self }

  async fn execute(self) -> Result<CommandOutput, Error> {
    let Self { electron, mode, script, args, timeout, max_output_bytes, mut on_output } = self;
    let timeout_ms = timeout_millis(timeout)?;
    if !(1..=MAX_OUTPUT_BYTES).contains(&max_output_bytes) {
      return Err("command max_output_bytes must be between 1 byte and 256 MiB".into());
    }
    tokio::runtime::Handle::try_current()
      .map_err(|error| Error::with_source("shared-main command requires a Tokio runtime", error))?;
    let task_id = electron.command_task_id()?;
    let stream = on_output.is_some();
    let (route, mut chunks, mut failure) = CommandRoute::new(max_output_bytes);
    electron.register_command(task_id.clone(), route)?;
    let mut guard = CommandGuard { electron: Arc::clone(&electron), task_id: task_id.clone(), cancel: true };
    let response = electron.call(Execute {
      task_id, mode, script, args, timeout_ms, max_output_bytes, stream,
      rpc_timeout: Duration::from_millis(timeout_ms) + RESPONSE_GRACE,
    });
    tokio::pin!(response);
    let mut captured = CapturedOutput::default();
    let mut failure_open = true;
    let mut chunks_open = true;
    let wire = loop {
      tokio::select! {
        biased;
        error = &mut failure, if failure_open => {
          match error {
            Ok(error) => return Err(error),
            Err(_) => failure_open = false,
          }
        }
        chunk = chunks.recv(), if chunks_open => {
          match chunk {
            Some(chunk) => captured.deliver(chunk, &mut on_output)?,
            None => chunks_open = false,
          }
        }
        response = &mut response => break response?,
      }
    };
    // The reader routes every preceding output notification before waking RPC.
    if let Ok(error) = failure.try_recv() { return Err(error); }
    while let Ok(chunk) = chunks.try_recv() { captured.deliver(chunk, &mut on_output)?; }
    let output = wire.decode(max_output_bytes)?;
    if stream && (captured.stdout != output.stdout || captured.stderr != output.stderr) {
      return Err(protocol_error("streamed stdout/stderr do not match the final command output"));
    }
    guard.cancel = false;
    Ok(output)
  }
}

impl IntoFuture for Command {
  type Output = Result<CommandOutput, Error>;
  type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'static>>;

  fn into_future(self) -> Self::IntoFuture { Box::pin(self.execute()) }
}

fn timeout_millis(timeout: Duration) -> Result<u64, Error> {
  let millis = timeout.as_millis();
  if !(1..=2_147_483_647).contains(&millis) || timeout.subsec_nanos() % 1_000_000 != 0 {
    return Err("command timeout must be 1..=2147483647 whole milliseconds".into());
  }
  Ok(millis as u64)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Execute {
  task_id: String,
  mode: Mode,
  script: String,
  args: Vec<String>,
  timeout_ms: u64,
  max_output_bytes: usize,
  stream: bool,
  #[serde(skip)]
  rpc_timeout: Duration,
}

impl RpcRequest for Execute {
  type Response = WireOutput;
  const METHOD: &'static str = "command.execute";
  fn timeout(&self) -> Duration { self.rpc_timeout }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Cancel { task_id: String }

impl RpcRequest for Cancel {
  type Response = ();
  const METHOD: &'static str = "command.cancel";
  fn notification(&self) -> bool { true }
  fn timeout(&self) -> Duration { RESPONSE_GRACE }
}

struct CommandGuard {
  electron: Arc<Electron>,
  task_id: String,
  cancel: bool,
}

impl Drop for CommandGuard {
  fn drop(&mut self) {
    self.electron.remove_command(&self.task_id);
    if self.cancel {
      if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        let electron = Arc::clone(&self.electron);
        let request = Cancel { task_id: self.task_id.clone() };
        runtime.spawn(async move { let _ = electron.call(request).await; });
      }
    }
  }
}

#[derive(Deserialize)]
struct WireOutput {
  code: i32,
  stdout: String,
  stderr: String,
  result: Value,
}

impl WireOutput {
  fn decode(self, limit: usize) -> Result<CommandOutput, Error> {
    let stdout = decode_bytes(&self.stdout, limit)?;
    let stderr = decode_bytes(&self.stderr, limit - stdout.len())?;
    Ok(CommandOutput { code: self.code, stdout, stderr, result: self.result })
  }
}

#[derive(Deserialize)]
struct WireChunk {
  sequence: u64,
  stream: OutputStream,
  data: String,
}

#[derive(Deserialize)]
struct WireChunks { chunks: Vec<WireChunk> }

/// Only the IPC reader writes this route; user callbacks run in Command::execute.
pub(crate) struct CommandRoute {
  sender: mpsc::Sender<CommandChunk>,
  failure: Option<oneshot::Sender<Error>>,
  remaining: usize,
}

impl CommandRoute {
  fn new(limit: usize) -> (Self, mpsc::Receiver<CommandChunk>, oneshot::Receiver<Error>) {
    let (sender, receiver) = mpsc::channel(OUTPUT_QUEUE_CAPACITY);
    let (failure, failed) = oneshot::channel();
    (Self { sender, failure: Some(failure), remaining: limit }, receiver, failed)
  }

  pub(crate) fn fail(&mut self, error: Error) {
    if let Some(failure) = self.failure.take() { let _ = failure.send(error); }
  }

  pub(crate) fn dispatch(&mut self, params: Value) -> bool {
    let result = (|| {
      let batch: WireChunks = serde_json::from_value(params)
        .map_err(|error| Error::with_source("invalid command.output notification", error))?;
      for wire in batch.chunks {
        let data = decode_bytes(&wire.data, self.remaining)?;
        self.remaining -= data.len();
        self.sender.try_send(CommandChunk { sequence: wire.sequence, stream: wire.stream, data })
          .map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => protocol_error("command output queue overflow"),
            mpsc::error::TrySendError::Closed(_) => protocol_error("command output receiver closed"),
          })?;
      }
      Ok(())
    })();
    match result {
      Ok(()) => true,
      Err(error) => { self.fail(error); false }
    }
  }
}

#[derive(Default)]
struct CapturedOutput {
  sequence: u64,
  stdout: Vec<u8>,
  stderr: Vec<u8>,
}

impl CapturedOutput {
  fn deliver(&mut self, chunk: CommandChunk, callback: &mut Option<OutputCallback>) -> Result<(), Error> {
    if chunk.sequence != self.sequence {
      return Err(protocol_error(format!("command output sequence: expected {}, got {}", self.sequence, chunk.sequence)));
    }
    self.sequence = self.sequence.checked_add(1).ok_or_else(|| protocol_error("command output sequence overflow"))?;
    let Some(callback) = callback else { return Err(protocol_error("received command output with stream disabled")); };
    match chunk.stream {
      OutputStream::Stdout => self.stdout.extend_from_slice(&chunk.data),
      OutputStream::Stderr => self.stderr.extend_from_slice(&chunk.data),
    }
    catch_unwind(AssertUnwindSafe(|| callback(chunk)))
      .map_err(|_| protocol_error("command output callback panicked"))
  }
}

fn decode_bytes(encoded: &str, limit: usize) -> Result<Vec<u8>, Error> {
  if encoded.len() > limit.div_ceil(3) * 4 {
    return Err(protocol_error("command output exceeded max_output_bytes"));
  }
  let bytes = STANDARD.decode(encoded)
    .map_err(|error| Error::with_source("invalid command output base64", error))?;
  if bytes.len() > limit { return Err(protocol_error("command output exceeded max_output_bytes")); }
  Ok(bytes)
}

fn protocol_error(message: impl Into<String>) -> Error {
  Error::Rpc { method: "command.execute".into(), message: message.into() }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  #[test]
  fn wire_output_preserves_raw_bytes_status_and_result() {
    let wire: WireOutput = serde_json::from_value(json!({
      "code": 23, "stdout": "AP+A", "stderr": "wyg=", "result": {"ok": true}
    })).unwrap();
    let output = wire.decode(5).unwrap();
    assert_eq!(output.stdout, [0, 255, 128]);
    assert_eq!(output.stderr, [195, 40]);
    assert!(!output.success());
    assert_eq!(output.result, json!({"ok": true}));
    assert!(decode_bytes("***=", 8).is_err());
    assert!(decode_bytes("AQID", 2).is_err());
    let wire: WireOutput = serde_json::from_value(json!({
      "code": 0, "stdout": "AQID", "stderr": "BA==", "result": null
    })).unwrap();
    assert!(wire.decode(3).is_err());
    for code in [0, 1, 124, 130] {
      let wire: WireOutput = serde_json::from_value(json!({
        "code": code, "stdout": "", "stderr": "", "result": null
      })).unwrap();
      let output = wire.decode(1).unwrap();
      assert_eq!(output.code, code);
      assert_eq!(output.success(), code == 0);
    }
  }

  #[test]
  fn command_route_reports_overflow_closed_receiver_and_byte_limit() {
    let (mut route, _receiver, mut failure) = CommandRoute::new(1);
    for sequence in 0..OUTPUT_QUEUE_CAPACITY {
      assert!(route.dispatch(json!({"chunks":[{"sequence":sequence,"stream":"stdout","data":""}]})));
    }
    assert!(!route.dispatch(json!({"chunks":[{"sequence":256,"stream":"stdout","data":""}]})));
    assert!(failure.try_recv().unwrap().to_string().contains("overflow"));
    let (mut route, receiver, mut failure) = CommandRoute::new(1);
    drop(receiver);
    assert!(!route.dispatch(json!({"chunks":[{"sequence":0,"stream":"stderr","data":"AA=="}]})));
    assert!(failure.try_recv().unwrap().to_string().contains("receiver closed"));
    let (mut route, _receiver, mut failure) = CommandRoute::new(1);
    assert!(!route.dispatch(json!({"chunks":[{"sequence":0,"stream":"stdout","data":"AAA="}]})));
    assert!(failure.try_recv().unwrap().to_string().contains("max_output_bytes"));
  }

  #[test]
  fn output_requires_contiguous_sequence_and_valid_deadline() {
    let mut captured = CapturedOutput::default();
    let mut callback: Option<OutputCallback> = Some(Box::new(|_| {}));
    assert!(captured.deliver(CommandChunk { sequence: 1, stream: OutputStream::Stdout, data: vec![] }, &mut callback).is_err());
    assert!(captured.deliver(CommandChunk { sequence: 0, stream: OutputStream::Stdout, data: vec![255] }, &mut callback).is_ok());
    assert_eq!(captured.stdout, [255]);
    assert!(timeout_millis(Duration::ZERO).is_err());
    assert!(timeout_millis(Duration::from_nanos(1_000_001)).is_err());
    assert!(timeout_millis(Duration::from_millis(2_147_483_648)).is_err());
    assert_eq!(timeout_millis(Duration::from_secs(30)).unwrap(), 30_000);
  }
}
