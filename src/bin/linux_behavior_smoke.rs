//! Linux X11 behavior checks against independent EWMH properties.
#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("Linux behavior: not applicable; no member evidence emitted");
    std::process::ExitCode::SUCCESS
}

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    match linux::run() {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::FAILURE,
        Err(error) => {
            eprintln!("Linux behavior: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use raw_window_handle::RawWindowHandle;
    use serde_json::json;
    use std::{
        borrow::Cow,
        io::{self, Read},
        process::{Child, Command, ExitStatus, Stdio},
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    use tauri_runtime::{
        window::{PendingWindow, RawWindow, WindowBuilder},
        Icon, RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs, WindowDispatch,
    };
    use tauri_runtime_electron::{
        ElectronRuntime, ElectronRuntimeHandle, ElectronWindowBuilder,
        ElectronWindowDispatcher as Window,
    };

    type Check = Result<(), String>;
    const WAIT: Duration = Duration::from_secs(4);
    const XPROP_TIMEOUT: Duration = Duration::from_secs(2);
    const MAX_XPROP_OUTPUT: usize = 16 * 1024;
    const RED: [u8; 4] = [231, 37, 61, 255];
    const BLUE: [u8; 4] = [29, 83, 219, 255];

    fn icon(color: [u8; 4]) -> Icon<'static> {
        Icon {
            rgba: Cow::Owned(color.repeat(4)),
            width: 2,
            height: 2,
        }
    }

    fn evidence(member: &str, assertion: &str, status: &str) {
        println!(
            "[runtime-evidence] {}",
            json!({
              "member": member,
              "test": "linux_behavior_smoke::native_x11",
              "assertion": assertion,
              "kind": "behavior",
              "status": status,
            })
        );
    }

    fn check(member: &str, assertion: &str, action: impl FnOnce() -> Check) -> bool {
        match action() {
            Ok(()) => {
                evidence(member, assertion, "pass");
                true
            }
            Err(error) => {
                let status = if error.starts_with("UNKNOWN:") {
                    "unknown"
                } else {
                    "fail"
                };
                evidence(member, &format!("{assertion}; {error}"), status);
                eprintln!("{member}: {error}");
                status != "fail"
            }
        }
    }

    fn fail(message: impl Into<String>) -> String {
        format!("FAIL: {}", message.into())
    }

    fn unknown(message: impl Into<String>) -> String {
        format!("UNKNOWN: {}", message.into())
    }

    fn read_bounded<R: Read>(
        mut reader: R,
        overflow: std::sync::Arc<AtomicBool>,
    ) -> io::Result<Vec<u8>> {
        let mut output = Vec::with_capacity(MAX_XPROP_OUTPUT.min(4096));
        let mut buffer = [0u8; 1024];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                return Ok(output);
            }
            if output.len() + count <= MAX_XPROP_OUTPUT {
                output.extend_from_slice(&buffer[..count]);
            } else {
                // Keep draining until the parent kills an unexpectedly noisy xprop.
                overflow.store(true, Ordering::Relaxed);
            }
        }
    }

    fn stop_child(child: &mut Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    fn collect_xprop(mut child: Child) -> Result<String, String> {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| unknown("xprop stdout was not piped"))?;
        let overflow = std::sync::Arc::new(AtomicBool::new(false));
        let reader_overflow = overflow.clone();
        let reader = thread::spawn(move || read_bounded(stdout, reader_overflow));
        let started = Instant::now();
        let status: ExitStatus = loop {
            if overflow.load(Ordering::Relaxed) {
                stop_child(&mut child);
                let _ = reader.join();
                return Err(unknown("xprop output exceeded the 16 KiB bound"));
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => {
                    stop_child(&mut child);
                    let _ = reader.join();
                    return Err(unknown(format!("could not poll xprop: {error}")));
                }
            }
            if started.elapsed() >= XPROP_TIMEOUT {
                stop_child(&mut child);
                let _ = reader.join();
                return Err(unknown("xprop exceeded its 2 second timeout"));
            }
            thread::sleep(Duration::from_millis(10));
        };
        let output = reader
            .join()
            .map_err(|_| unknown("xprop output reader panicked"))?
            .map_err(|error| unknown(format!("could not read xprop output: {error}")))?;
        if overflow.load(Ordering::Relaxed) {
            return Err(unknown("xprop output exceeded the 16 KiB bound"));
        }
        if !status.success() {
            return Err(unknown(format!("xprop exited with {status}")));
        }
        String::from_utf8(output)
            .map_err(|error| unknown(format!("xprop output was not UTF-8: {error}")))
    }

    fn xprop_window(xid: u32, properties: &[&str]) -> Result<String, String> {
        let xid = format!("0x{xid:x}");
        let child = Command::new("xprop")
            .arg("-len")
            .arg("64")
            .arg("-id")
            .arg(xid)
            .args(properties)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| unknown(format!("could not start xprop: {error}")))?;
        collect_xprop(child)
    }

    fn xprop_root(property: &str) -> Result<String, String> {
        let child = Command::new("xprop")
            .arg("-len")
            .arg("32")
            .arg("-root")
            .arg(property)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| unknown(format!("could not start xprop: {error}")))?;
        collect_xprop(child)
    }

    fn property_values(output: &str) -> Result<Option<Vec<u32>>, String> {
        let Some((_, value)) = output.split_once('=') else {
            if output.contains("not found") {
                return Ok(None);
            }
            return Err(unknown("xprop returned an unrecognized property response"));
        };
        let value = value.trim();
        if value == "not found" {
            return Ok(None);
        }
        let mut values = Vec::new();
        for token in value
            .split(|ch: char| ch == ',' || ch.is_ascii_whitespace())
            .filter(|part| !part.is_empty())
        {
            let parsed = if let Some(hex) = token.strip_prefix("0x") {
                u32::from_str_radix(hex, 16)
            } else {
                token.parse::<u32>()
            }
            .map_err(|_| unknown(format!("xprop cardinal was not numeric: {token}")))?;
            values.push(parsed);
        }
        Ok(Some(values))
    }

    fn single_cardinal(output: &str) -> Result<Option<u32>, String> {
        let Some(values) = property_values(output)? else {
            return Ok(None);
        };
        values
            .first()
            .copied()
            .map(Some)
            .ok_or_else(|| unknown("xprop cardinal property was empty"))
    }

    fn icon_groups(output: &str) -> Result<Option<Vec<(u32, u32, Vec<u32>)>>, String> {
        let Some(values) = property_values(output)? else {
            return Ok(None);
        };
        let mut groups = Vec::new();
        let mut offset = 0usize;
        while offset < values.len() {
            if values.len() - offset < 2 {
                return Err(unknown("_NET_WM_ICON ended inside an icon header"));
            }
            let width = values[offset];
            let height = values[offset + 1];
            let count = (width as usize)
                .checked_mul(height as usize)
                .filter(|count| *count > 0 && *count <= 4096)
                .ok_or_else(|| unknown("_NET_WM_ICON contained invalid dimensions"))?;
            offset += 2;
            let end = offset
                .checked_add(count)
                .ok_or_else(|| unknown("_NET_WM_ICON size overflow"))?;
            if end > values.len() {
                return Err(unknown("_NET_WM_ICON ended inside pixel data"));
            }
            groups.push((width, height, values[offset..end].to_vec()));
            offset = end;
        }
        Ok(Some(groups))
    }

    fn argb(rgba: [u8; 4]) -> u32 {
        (u32::from(rgba[3]) << 24)
            | (u32::from(rgba[0]) << 16)
            | (u32::from(rgba[1]) << 8)
            | u32::from(rgba[2])
    }

    fn wait_icon(xid: u32, color: [u8; 4], description: &str) -> Check {
        let expected = argb(color);
        let start = Instant::now();
        let mut found_2x2 = false;
        loop {
            let output = xprop_window(xid, &["_NET_WM_ICON"])?;
            let pixels = icon_groups(&output)?
                .unwrap_or_default()
                .iter()
                .filter(|(width, height, _)| *width == 2 && *height == 2)
                .flat_map(|(_, _, pixels)| pixels.iter().copied())
                .collect::<Vec<_>>();
            found_2x2 |= !pixels.is_empty();
            if pixels.len() == 4 && pixels.iter().all(|pixel| *pixel == expected) {
                return Ok(());
            }
            if start.elapsed() >= WAIT {
                return if found_2x2 {
                    Err(fail(format!("{description}: native _NET_WM_ICON pixels were {pixels:?}, expected four {expected:#010x}")))
                } else {
                    Err(unknown(format!(
                        "{description}: WM did not expose a 2x2 _NET_WM_ICON; observed {pixels:?}"
                    )))
                };
            }
            thread::sleep(Duration::from_millis(80));
        }
    }

    fn wait_desktop(xid: u32, expected: u32, description: &str) -> Check {
        let start = Instant::now();
        loop {
            let output = xprop_window(xid, &["_NET_WM_DESKTOP"])?;
            let observed = single_cardinal(&output)?;
            if observed == Some(expected) {
                return Ok(());
            }
            if start.elapsed() >= WAIT {
                return match observed {
                    Some(actual) => Err(fail(format!(
                        "{description}: _NET_WM_DESKTOP is {actual:#x}, expected {expected:#x}"
                    ))),
                    None => Err(unknown(format!(
                        "{description}: window manager did not expose _NET_WM_DESKTOP"
                    ))),
                };
            }
            thread::sleep(Duration::from_millis(80));
        }
    }

    fn current_desktop() -> Result<Option<u32>, String> {
        single_cardinal(&xprop_root("_NET_CURRENT_DESKTOP")?)
    }

    fn query_window_identity(xid: u32) -> Check<String> {
        let output = xprop_window(xid, &["_NET_WM_PID", "WM_NAME"])?;
        let mut pid = None;
        let mut name = None;
        for line in output.lines() {
            if line.starts_with("_NET_WM_PID") {
                pid = single_cardinal(line)?;
            }
            if line.starts_with("WM_NAME") {
                let Some((_, value)) = line.split_once('=') else {
                    continue;
                };
                let value = value.trim();
                if value != "not found" {
                    name = Some(
                        value
                            .strip_prefix('"')
                            .and_then(|text| text.strip_suffix('"'))
                            .unwrap_or(value)
                            .to_string(),
                    );
                }
            }
        }
        let pid = pid.ok_or_else(|| unknown("Xlib XID did not expose _NET_WM_PID"))?;
        if pid == 0 || pid == std::process::id() {
            return Err(fail(format!(
                "Xlib XID must belong to Electron, not pid {} (observed {pid})",
                std::process::id()
            )));
        }
        let name = name.ok_or_else(|| fail("Xlib XID has no WM_NAME"))?;
        Ok(name)
    }

    struct NativeWindow(Window);
    impl Drop for NativeWindow {
        fn drop(&mut self) {
            let _ = WindowDispatch::<()>::destroy(&self.0);
        }
    }

    fn create_window(handle: &ElectronRuntimeHandle, title: &str) -> Result<NativeWindow, String> {
        let builder = ElectronWindowBuilder::new()
            .title(title)
            .inner_size(240.0, 160.0)
            .visible(true)
            .icon(icon(RED))
            .map_err(|error| unknown(format!("WindowBuilder::icon rejected 2x2 RGBA: {error}")))?;
        let pending =
            PendingWindow::<(), ElectronRuntime>::new(builder, title).map_err(|error| {
                unknown(format!("could not build pending Electron window: {error}"))
            })?;
        let detached = <ElectronRuntimeHandle as RuntimeHandle<()>>::create_window(
            handle,
            pending,
            None::<fn(RawWindow<'_>)>,
        )
        .map_err(|error| unknown(format!("could not create Electron window: {error}")))?;
        Ok(NativeWindow(detached.dispatcher))
    }

    fn xlib_id(window: &Window) -> Result<u32, String> {
        let handle = WindowDispatch::<()>::window_handle(window).map_err(|error| {
            unknown(format!(
                "WindowDispatch::window_handle unavailable: {error}"
            ))
        })?;
        let RawWindowHandle::Xlib(raw) = handle.as_raw() else {
            return Err(fail(format!(
                "WindowDispatch::window_handle did not return Xlib: {:?}",
                handle.as_raw()
            )));
        };
        let xid = u32::try_from(raw.window).map_err(|_| {
            fail("WindowDispatch::window_handle returned a zero or out-of-range XID")
        })?;
        if xid == 0 {
            return Err(fail("WindowDispatch::window_handle returned XID 0"));
        }
        Ok(xid)
    }

    fn icon_builder_check(xid: u32) -> Check {
        wait_icon(xid, RED, "WindowBuilder::icon")
    }

    fn icon_dispatch_check(window: &Window, xid: u32) -> Check {
        WindowDispatch::<()>::set_icon(window, icon(BLUE)).map_err(|error| {
            unknown(format!(
                "WindowDispatch::set_icon rejected 2x2 RGBA: {error}"
            ))
        })?;
        wait_icon(xid, BLUE, "WindowDispatch::set_icon")
    }

    fn workspace_check(window: &Window, xid: u32) -> Check {
        let mut unknown_reason = None;
        let mut failure = None;
        let mut baseline = None;
        if let Err(error) = WindowDispatch::<()>::set_visible_on_all_workspaces(window, false) {
            unknown_reason = Some(unknown(format!(
                "set_visible_on_all_workspaces(false) rejected: {error}"
            )));
        }
        match current_desktop() {
            Ok(Some(desktop)) => {
                baseline = Some(desktop);
                if let Err(error) = wait_desktop(xid, desktop, "false/current desktop") {
                    if error.starts_with("FAIL:") {
                        failure = Some(error)
                    } else {
                        unknown_reason = Some(error)
                    }
                }
            }
            Ok(None) => {
                unknown_reason = Some(unknown(
                    "root window manager did not expose _NET_CURRENT_DESKTOP",
                ))
            }
            Err(error) => unknown_reason = Some(error),
        }
        if let Err(error) = WindowDispatch::<()>::set_visible_on_all_workspaces(window, true) {
            unknown_reason = Some(unknown(format!(
                "set_visible_on_all_workspaces(true) rejected: {error}"
            )));
        }
        if let Err(error) = wait_desktop(xid, u32::MAX, "true/all workspaces") {
            if error.starts_with("FAIL:") {
                failure = Some(error)
            } else {
                unknown_reason = Some(error)
            }
        }
        if let Err(error) = WindowDispatch::<()>::set_visible_on_all_workspaces(window, false) {
            unknown_reason = Some(unknown(format!(
                "set_visible_on_all_workspaces(false) restore rejected: {error}"
            )));
        }
        if baseline.is_none() {
            baseline = match current_desktop() {
                Ok(value) => value,
                Err(error) => {
                    unknown_reason = Some(error);
                    None
                }
            };
        }
        if let Some(desktop) = baseline {
            if let Err(error) = wait_desktop(xid, desktop, "false/restore current desktop") {
                if error.starts_with("FAIL:") {
                    failure = Some(error)
                } else {
                    unknown_reason = Some(error)
                }
            }
        }
        if let Some(error) = failure {
            Err(error)
        } else if let Some(error) = unknown_reason {
            Err(error)
        } else if baseline.is_none() {
            Err(unknown(
                "current desktop was unavailable for the final false state",
            ))
        } else {
            Ok(())
        }
    }

    fn handle_check(window: &Window, title: &str) -> Check {
        let xid = xlib_id(window)?;
        let name = query_window_identity(xid)?;
        if name != title {
            return Err(fail(format!(
                "Xlib XID {xid:#x} names {name:?}, expected unique Electron WM_NAME {title:?}"
            )));
        }
        Ok(())
    }

    fn checks(handle: &ElectronRuntimeHandle) -> bool {
        let title = format!(
            "linux-behavior-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |time| time.as_nanos())
        );
        let window = match create_window(handle, &title) {
            Ok(window) => window,
            Err(error) => {
                for member in [
                    "WindowBuilder::icon",
                    "WindowDispatch::set_icon",
                    "WindowDispatch::set_visible_on_all_workspaces",
                    "WindowDispatch::window_handle",
                ] {
                    evidence(
                        member,
                        &format!("native observation unavailable; {error}"),
                        "unknown",
                    );
                }
                return true;
            }
        };
        let xid = match xlib_id(&window.0) {
            Ok(xid) => xid,
            Err(error) => {
                evidence(
                    "WindowDispatch::window_handle",
                    &format!("returned handle cannot be independently queried; {error}"),
                    if error.starts_with("UNKNOWN:") {
                        "unknown"
                    } else {
                        "fail"
                    },
                );
                for member in [
                    "WindowBuilder::icon",
                    "WindowDispatch::set_icon",
                    "WindowDispatch::set_visible_on_all_workspaces",
                ] {
                    evidence(
                        member,
                        "native X11 observation unavailable because no valid Xlib XID was returned",
                        "unknown",
                    );
                }
                return !error.starts_with("FAIL:");
            }
        };
        let identity_assertion = "returned Xlib XID has external _NET_WM_PID (nonzero and not the Rust process) and the unique Electron WM_NAME";
        if let Err(error) = handle_check(&window.0, &title) {
            let status = if error.starts_with("UNKNOWN:") {
                "unknown"
            } else {
                "fail"
            };
            evidence(
                "WindowDispatch::window_handle",
                &format!("{identity_assertion}; {error}"),
                status,
            );
            for member in [
                "WindowBuilder::icon",
                "WindowDispatch::set_icon",
                "WindowDispatch::set_visible_on_all_workspaces",
            ] {
                evidence(member, "not checked because the returned Xlib XID was not independently identified as the Electron window", "unknown");
            }
            return status != "fail";
        }
        evidence("WindowDispatch::window_handle", identity_assertion, "pass");
        let mut passed = true;
        passed &= check("WindowBuilder::icon", "independent xprop _NET_WM_ICON contains four 2x2 ARGB pixels matching the builder's red RGBA icon", || icon_builder_check(xid));
        passed &= check("WindowDispatch::set_icon", "after set_icon, independent xprop _NET_WM_ICON contains four 2x2 ARGB pixels matching the changed blue RGBA icon", || icon_dispatch_check(&window.0, xid));
        passed &= check("WindowDispatch::set_visible_on_all_workspaces", "independent _NET_WM_DESKTOP follows current desktop, 0xFFFFFFFF, then current desktop after false/true/false", || workspace_check(&window.0, xid));
        passed
    }

    pub fn run() -> Result<bool, String> {
        thread::spawn(|| {
            thread::sleep(Duration::from_secs(90));
            eprintln!("Linux behavior: UNKNOWN (90s watchdog expired; unfinished assertions have no pass evidence)");
            std::process::exit(124);
        });
        let runtime = match <ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs::default()) {
            Ok(runtime) => runtime,
            Err(error) => {
                let message = unknown(format!("could not initialize Electron runtime: {error}"));
                for member in [
                    "WindowBuilder::icon",
                    "WindowDispatch::set_icon",
                    "WindowDispatch::set_visible_on_all_workspaces",
                    "WindowDispatch::window_handle",
                ] {
                    evidence(member, &message, "unknown");
                }
                return Ok(true);
            }
        };
        let handle = <ElectronRuntime as Runtime<()>>::handle(&runtime);
        let keeper = PendingWindow::<(), ElectronRuntime>::new(
            ElectronWindowBuilder::new()
                .title("linux-behavior-keeper")
                .visible(false),
            "linux-behavior-keeper",
        )
        .map_err(|error| unknown(format!("could not create runtime keeper: {error}")))?;
        let _keeper = <ElectronRuntime as Runtime<()>>::create_window(
            &runtime,
            keeper,
            None::<fn(RawWindow<'_>)>,
        )
        .map_err(|error| unknown(format!("could not create runtime keeper: {error}")))?;
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ready_rx.recv_timeout(WAIT).map_err(|error| {
                    unknown(format!("runtime pump did not become ready: {error}"))
                })?;
                Ok::<bool, String>(checks(&handle))
            }));
            let passed = matches!(result, Ok(Ok(true)));
            let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(
                &handle,
                i32::from(!passed),
            );
            result
        });
        let code = <ElectronRuntime as Runtime<()>>::run_return(runtime, move |event| {
            if let RunEvent::Ready = event {
                let _ = ready_tx.send(());
            }
        });
        match worker.join() {
            Ok(Ok(Ok(passed))) if code == 0 => Ok(passed),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) | Err(_) => Err(fail("Linux behavior worker panicked")),
            Ok(Ok(Ok(_))) => Err(fail(format!("Electron runtime exited with {code}"))),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_ewmh_icon_argb_and_cardinals() {
            let output =
                "_NET_WM_ICON(CARDINAL) = 2, 2, 0xffe7253d, 0xffe7253d, 0xffe7253d, 0xffe7253d";
            let groups = icon_groups(output).unwrap().unwrap();
            assert_eq!(groups, vec![(2, 2, vec![argb(RED); 4])]);
            assert_eq!(
                single_cardinal("_NET_CURRENT_DESKTOP(CARDINAL) = 3").unwrap(),
                Some(3)
            );
            assert_eq!(
                single_cardinal("_NET_WM_DESKTOP: not found.").unwrap(),
                None
            );
        }
    }
}
