use super::*;
use std::{
  process::Command,
  sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_PEER: AtomicUsize = AtomicUsize::new(0);

struct Peer {
  ipc: Arc<Ipc>,
  directory: PathBuf,
}

impl Peer {
  fn new(workers: usize) -> Self {
    Self::with_fifo(workers, true)
  }

  fn with_fifo(workers: usize, fifo: bool) -> Self {
    let directory = std::env::temp_dir().join(format!(
      "rpc-writer-{}-{}",
      std::process::id(),
      NEXT_PEER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    let binary = directory.join(if cfg!(windows) { "peer.exe" } else { "peer" });
    std::fs::write(directory.join("main.cjs"), "").unwrap();
    let source = directory.join("peer.rs");
    std::fs::write(&source, r#"
use std::{io::{BufRead, BufReader, Write}, net::TcpStream};
fn number(line: &str, key: &str) -> usize {
  line.split(&format!("\"{key}\":")).nth(1).unwrap().chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap()
}
fn main() {
  let args: Vec<_> = std::env::args().collect();
  let port = &args[args.iter().position(|arg| arg == "--port").unwrap() + 1];
  let mut token = String::new();
  std::io::stdin().read_line(&mut token).unwrap();
  let mut socket = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
  writeln!(socket, "{{\"event\":\"ready\",\"params\":{{\"token\":\"{}\",\"version\":\"test\"}}}}", token.trim()).unwrap();
  for (next, line) in BufReader::new(socket.try_clone().unwrap()).lines().enumerate() {
    let line = line.unwrap();
    let sequence = number(&line, "sequence");
    if !args.iter().any(|arg| arg == "--unordered") { assert_eq!(sequence, next, "FIFO violated: {line}"); }
    if !line.contains("\"notification\":true") {
      writeln!(socket, "{{\"id\":{},\"result\":{sequence}}}", number(&line, "id")).unwrap();
    }
  }
}
"#).unwrap();
    let status = Command::new("rustc")
      .arg("--edition=2021")
      .arg(&source)
      .arg("-o")
      .arg(&binary)
      .status()
      .unwrap();
    assert!(status.success(), "compile native loopback peer");
    let runtime = tokio::runtime::Builder::new_multi_thread()
      .worker_threads(workers)
      .max_blocking_threads(1)
      .enable_time()
      .build()
      .unwrap();
    let args = if fifo {
      vec![]
    } else {
      vec!["--unordered".into()]
    };
    let electron = runtime
      .block_on(Electron::with_args(
        binary,
        0,
        &args,
        Duration::from_secs(5),
      ))
      .unwrap();
    let (tasks, receiver) = mpsc::sync_channel::<Task>(1024);
    std::thread::spawn(move || {
      while let Ok(task) = receiver.recv() {
        task(&runtime);
      }
    });
    Self {
      ipc: Arc::new(Ipc { electron, tasks }),
      directory,
    }
  }
}

impl Drop for Peer {
  fn drop(&mut self) {
    self.ipc.shutdown();
    std::fs::remove_dir_all(&self.directory).unwrap();
  }
}

#[derive(serde::Serialize)]
struct Request {
  sequence: usize,
}

impl RpcRequest for Request {
  type Response = usize;
  const METHOD: &'static str = "test.echo";
}

fn queued_requests_preserve_fifo(workers: usize) {
  let peer = Peer::new(workers);
  let (sender, receiver) = mpsc::channel();
  for sequence in 0..800 {
    if sequence % 3 == 0 {
      peer.ipc.notify(Request { sequence }).unwrap();
    } else {
      let sender = sender.clone();
      peer
        .ipc
        .dispatch(Request { sequence }, move |result| {
          sender.send((sequence, result)).unwrap();
        })
        .unwrap();
    }
  }
  let mut results = Vec::new();
  for _ in 0..533 {
    let (sequence, result) = receiver
      .recv_timeout(Duration::from_secs(7))
      .expect("writer queue stalled");
    assert_eq!(result.unwrap(), sequence);
    results.push(sequence);
  }
  results.sort_unstable();
  assert_eq!(
    results,
    (0..800)
      .filter(|sequence| sequence % 3 != 0)
      .collect::<Vec<_>>()
  );
  assert_eq!(peer.ipc.call(Request { sequence: 800 }).unwrap(), 800);
}

fn external_runtime_workers_can_wait_for_adapter_calls(workers: usize) {
  let peer = Peer::with_fifo(workers, false);
  let external = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(workers)
    .enable_time()
    .build()
    .unwrap();
  let (release, blocked) = mpsc::channel();
  let (started, blocking) = mpsc::channel();
  peer
    .ipc
    .tasks
    .send(Box::new(move |runtime| {
      runtime.spawn_blocking(move || {
        started.send(()).unwrap();
        blocked.recv().unwrap();
      });
    }))
    .unwrap();
  blocking.recv_timeout(Duration::from_secs(2)).unwrap();
  let (first_done, first) = mpsc::channel();
  peer
    .ipc
    .dispatch(Request { sequence: 0 }, move |result| {
      first_done.send(result).unwrap();
    })
    .unwrap();
  let (reserved, reservation) = mpsc::channel();
  peer
    .ipc
    .tasks
    .send(Box::new(move |_| {
      reserved.send(()).unwrap();
    }))
    .unwrap();
  reservation.recv_timeout(Duration::from_secs(2)).unwrap();
  let (polled, queued) = mpsc::channel();
  let (async_done, asynchronous) = mpsc::channel();
  let electron = peer.ipc.client();
  external.spawn(async move {
    let mut request = Box::pin(electron.call(Request { sequence: 1 }));
    std::future::poll_fn(|cx| {
      assert!(request.as_mut().poll(cx).is_pending());
      std::task::Poll::Ready(())
    })
    .await;
    polled.send(()).unwrap();
    async_done.send(request.await).unwrap();
  });
  queued.recv_timeout(Duration::from_secs(2)).unwrap();
  let (entered, entrances) = mpsc::channel();
  let (done, completion) = mpsc::channel();
  for sequence in 2..workers + 2 {
    let ipc = peer.ipc.clone();
    let entered = entered.clone();
    let done = done.clone();
    external.spawn(async move {
      entered.send(()).unwrap();
      done.send(ipc.call(Request { sequence })).unwrap();
    });
  }
  for _ in 0..workers {
    entrances.recv_timeout(Duration::from_secs(2)).unwrap();
  }
  release.send(()).unwrap();
  assert_eq!(
    first.recv_timeout(Duration::from_secs(7)).unwrap().unwrap(),
    0
  );
  let results: Vec<_> = (0..workers)
    .map(|_| completion.recv_timeout(Duration::from_secs(7)).unwrap())
    .collect();
  let asynchronous = asynchronous.recv_timeout(Duration::from_secs(7)).unwrap();
  external.shutdown_timeout(Duration::from_secs(1));
  assert!(
    results.iter().all(Result::is_ok),
    "adapter calls stalled behind external waiter: {results:?}"
  );
  assert_eq!(asynchronous.unwrap(), 1);
}

#[test]
fn current_thread_runtime_can_call_adapter() {
  let peer = Peer::new(1);
  let runtime = tokio::runtime::Builder::new_current_thread()
    .enable_time()
    .build()
    .unwrap();
  assert_eq!(
    runtime
      .block_on(async { peer.ipc.call(Request { sequence: 0 }) })
      .unwrap(),
    0
  );
}

#[test]
fn one_external_worker_can_wait_for_adapter_calls() {
  external_runtime_workers_can_wait_for_adapter_calls(1);
}

#[test]
fn two_external_workers_can_wait_for_adapter_calls() {
  external_runtime_workers_can_wait_for_adapter_calls(2);
}

#[test]
fn one_worker_queued_requests_preserve_fifo() {
  queued_requests_preserve_fifo(1);
}

#[test]
fn two_workers_queued_requests_preserve_fifo() {
  queued_requests_preserve_fifo(2);
}
