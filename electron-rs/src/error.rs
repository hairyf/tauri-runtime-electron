use std::{io, time::Duration};

/// A preparation, validation, process, or RPC failure with a matchable cause.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ElectronError {
  #[error("Electron RPC timed out after {timeout_duration:?}")]
  IpcTimeout { timeout_duration: Duration },
  #[error("Electron checksum mismatch: expected {expected}, got {actual}")]
  ChecksumMismatch { expected: String, actual: String },
  #[error("Electron IPC channel disconnected")]
  ChannelDisconnected,
  #[error("Electron RPC `{method}` failed: {message}")]
  Rpc { method: String, message: String },
  #[error("{0}")]
  InvalidInput(String),
  #[error("I/O operation failed: {0}")]
  Io(#[from] io::Error),
  #[error("invalid JSON: {0}")]
  Json(#[from] serde_json::Error),
  #[error("invalid UTF-16: {0}")]
  Utf16(#[from] std::string::FromUtf16Error),
  #[cfg(windows)]
  #[error("Windows operation failed: {0}")]
  Windows(#[from] windows::core::Error),
  #[error("{message}: {source}")]
  Context {
    message: String,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
  },
}

impl ElectronError {
  pub(crate) fn with_source(message: impl Into<String>, source: impl std::error::Error + Send + Sync + 'static) -> Self {
    Self::Context { message: message.into(), source: Box::new(source) }
  }

  pub(crate) fn context(self, message: impl Into<String>) -> Self {
    Self::with_source(message, self)
  }
}

impl From<String> for ElectronError {
  fn from(message: String) -> Self { Self::InvalidInput(message) }
}

impl From<&str> for ElectronError {
  fn from(message: &str) -> Self { Self::InvalidInput(message.to_owned()) }
}

/// Backwards-compatible name for the concrete error enum.
pub type Error = ElectronError;

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn errors_preserve_variants_and_underlying_sources() {
    let error = ElectronError::IpcTimeout { timeout_duration: Duration::from_millis(25) };
    assert!(matches!(error, ElectronError::IpcTimeout { timeout_duration } if timeout_duration == Duration::from_millis(25)));
    let error = ElectronError::with_source("read archive", io::Error::from(io::ErrorKind::NotFound));
    assert!(std::error::Error::source(&error).unwrap().is::<io::Error>());
    assert!(error.to_string().contains("read archive"));
    let error = ElectronError::ChecksumMismatch { expected: "a".into(), actual: "b".into() };
    assert!(matches!(error, ElectronError::ChecksumMismatch { expected, actual } if expected == "a" && actual == "b"));
  }
}
