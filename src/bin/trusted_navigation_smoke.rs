use std::{
    process::ExitCode,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Duration,
};
use tauri::{WebviewUrl, WebviewWindowBuilder};
use tauri_runtime_electron::{
    navigate_webview_with_trusted_origin, set_webview_trusted_origin, ElectronRuntime,
};

fn main() -> ExitCode {
    tauri_runtime_electron::register_custom_protocol_scheme("handoff").unwrap();
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(30));
        eprintln!("trusted navigation smoke exceeded 30s");
        std::process::exit(1);
    });
    let served = Arc::new(AtomicUsize::new(0));
    let requests = served.clone();
    let (sender, receiver) = mpsc::channel();
    let app = tauri::Builder::<ElectronRuntime>::new()
        .register_uri_scheme_protocol("handoff", move |_, request| {
            requests.fetch_add(1, Ordering::SeqCst);
            http::Response::builder()
                .header("content-type", "text/html")
                .body(
                    format!("<!doctype html><title>{}</title>", request.uri().path()).into_bytes(),
                )
                .unwrap()
        })
        .setup(move |app| {
            let window = WebviewWindowBuilder::new(
                app,
                "trusted-navigation",
                WebviewUrl::External("handoff://localhost/old".parse()?),
            )
            .visible(false)
            .build()?;
            let app = app.handle().clone();
            sender
                .send(thread::spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let (tx, rx) = mpsc::channel();
                        window
                            .eval_with_callback("document.title", move |value| {
                                let _ = tx.send(value);
                            })
                            .unwrap();
                        assert_eq!(rx.recv_timeout(Duration::from_secs(8)).unwrap(), "\"/old\"");
                        let (tx, rx) = mpsc::channel();
                        window
                            .eval_with_callback("location.href", move |value| {
                                let _ = tx.send(value);
                            })
                            .unwrap();
                        let current: String =
                            serde_json::from_str(&rx.recv_timeout(Duration::from_secs(8)).unwrap())
                                .unwrap();
                        let target = url::Url::parse(&current).unwrap().join("/new").unwrap();
                        let origin = if target.origin().ascii_serialization() == "null" {
                            "handoff://localhost".to_owned()
                        } else {
                            target.origin().ascii_serialization()
                        };
                        eprintln!(
                            "custom protocol current={current} origin={origin} target={target}"
                        );
                        let webview = window.as_ref();
                        set_webview_trusted_origin(webview, "http://127.0.0.1:1").unwrap();
                        tauri::async_runtime::block_on(navigate_webview_with_trusted_origin(
                            webview,
                            &origin,
                            target.as_str(),
                        ))
                        .unwrap();
                        assert!(
                            served.load(Ordering::SeqCst) >= 2,
                            "target custom protocol must dispatch while handoff is pending"
                        );
                        let (tx, rx) = mpsc::channel();
                        window
                            .eval_with_callback("document.title", move |value| {
                                let _ = tx.send(value);
                            })
                            .unwrap();
                        assert_eq!(rx.recv_timeout(Duration::from_secs(8)).unwrap(), "\"/new\"");
                    }));
                    app.exit(i32::from(result.is_err()));
                    result.is_ok()
                }))
                .unwrap();
            Ok(())
        })
        .build(tauri::generate_context!(
            "tests/assets/tauri-new-window/tauri.conf.json",
            test = true
        ))
        .unwrap();
    let status = app.run_return(|_, _| {});
    if receiver
        .recv_timeout(Duration::from_secs(8))
        .unwrap()
        .join()
        .unwrap()
        && status == 0
    {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
