use electron_rs::{Electron, RpcRequest};
use std::{
  future::Future,
  path::PathBuf,
  sync::{mpsc, Arc},
  time::Duration,
};
use tokio::runtime::Runtime;

type Task = Box<dyn FnOnce(&Runtime) + Send>;

/// The synchronous Tauri traits enter the async client only through this bridge.
#[derive(Debug)]
pub(crate) struct Ipc {
  electron: Arc<Electron>,
  tasks: mpsc::SyncSender<Task>,
}

impl Ipc {
  pub(crate) fn connect(
    binary: PathBuf,
    port: u16,
    args: Vec<String>,
    timeout: Duration,
  ) -> Result<Arc<Self>, electron_rs::Error> {
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
            let _ = ready_tx.send(Err(error.into()));
            return;
          }
        };
        let electron = runtime.block_on(Electron::with_args(binary, port, &args, timeout));
        if ready_tx.send(electron).is_err() {
          return;
        }
        while let Ok(task) = receiver.recv() {
          task(&runtime);
        }
      })?;
    let electron = ready_rx
      .recv()
      .map_err(|_| electron_rs::Error::ChannelDisconnected)??;
    Ok(Arc::new(Self { electron, tasks }))
  }

  pub(crate) fn call<R: RpcRequest>(&self, request: R) -> Result<R::Response, electron_rs::Error> {
    let (sender, receiver) = mpsc::sync_channel(1);
    self.dispatch(request, move |result| {
      let _ = sender.send(result);
    })?;
    receiver
      .recv()
      .map_err(|_| electron_rs::Error::ChannelDisconnected)?
  }

  pub(crate) fn client(&self) -> Arc<Electron> {
    self.electron.clone()
  }

  pub(crate) fn events(&self) -> Result<mpsc::Receiver<electron_rs::Event>, electron_rs::Error> {
    self.electron.events()
  }

  pub(crate) fn shutdown(&self) {
    self.electron.shutdown();
  }

  pub(crate) fn notify<R: RpcRequest>(&self, request: R) -> Result<(), electron_rs::Error> {
    self.dispatch(request, |result| {
      if let Err(error) = result {
        eprintln!("[electron] RPC notification failed: {error}");
      }
    })
  }

  pub(crate) fn dispatch<R: RpcRequest>(
    &self,
    request: R,
    callback: impl FnOnce(Result<R::Response, electron_rs::Error>) + Send + 'static,
  ) -> Result<(), electron_rs::Error> {
    let electron = Arc::clone(&self.electron);
    self
      .tasks
      .try_send(Box::new(move |runtime| {
        let mut task = Box::pin(async move {
          callback(electron.call(request).await);
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
        mpsc::TrySendError::Disconnected(_) => electron_rs::Error::ChannelDisconnected,
        mpsc::TrySendError::Full(_) => "Electron adapter task queue is full".into(),
      })
  }
}
