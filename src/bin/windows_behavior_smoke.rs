//! Finite Windows regressions against Electron's real HWND, never its Rust menu proxy.
//! Run serially on an interactive Windows desktop. Unobserved effects are not passes.
#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
  eprintln!("Windows behavior: not applicable; no member evidence emitted");
  std::process::ExitCode::SUCCESS
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
  match win::run() {
    Ok(()) => std::process::ExitCode::SUCCESS,
    Err(error) => {
      eprintln!("Windows behavior: {error}");
      std::process::ExitCode::FAILURE
    }
  }
}

#[cfg(windows)]
mod win {
  use std::{
    borrow::Cow,
    mem::size_of,
    sync::{atomic::{AtomicUsize, Ordering}, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
  };
  use serde_json::json;
  use tauri_runtime::{
    Icon, RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs, WindowDispatch,
    window::{PendingWindow, RawWindow, WindowBuilder},
  };
  use tauri_runtime_electron::{
    ElectronRuntime, ElectronRuntimeHandle, ElectronWindowBuilder, ElectronWindowDispatcher,
  };
  use tauri_utils::config::WindowConfig;
  use windows::{
    core::PCWSTR,
    Win32::{
      Foundation::{HWND, LPARAM, POINT, RECT, WPARAM},
      Graphics::{
        Dwm::DwmIsCompositionEnabled,
        Gdi::{
          ClientToScreen, GetDC, GetDIBits, GetMonitorInfoW, GetObjectW, MonitorFromWindow,
          ReleaseDC, DeleteObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
          DIB_RGB_COLORS, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        },
      },
      UI::HiDpi::{
        GetAwarenessFromDpiAwarenessContext, GetThreadDpiAwarenessContext,
        DPI_AWARENESS_PER_MONITOR_AWARE,
      },
      UI::WindowsAndMessaging::{
        ClipCursor, FindWindowExW, FindWindowW, GetClassNameW, GetClientRect, GetClipCursor,
        GetIconInfo, GetParent, GetPhysicalCursorPos, GetWindow, GetWindowDisplayAffinity,
        GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
        SendMessageTimeoutW, SetPhysicalCursorPos, SetWindowPos, GWL_EXSTYLE, GWL_STYLE,
        GW_OWNER, HICON, ICONINFO, ICON_BIG, ICON_SMALL, SMTO_ABORTIFHUNG,
        SMTO_ERRORONEXIT, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
        WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WM_GETICON, WS_CHILD, WS_EX_APPWINDOW,
        WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_POPUP,
      },
    },
  };

  type Check<T = ()> = Result<T, String>;
  const WAIT: Duration = Duration::from_secs(8);
  const RED: [u8; 4] = [231, 37, 61, 255];
  const BLUE: [u8; 4] = [29, 83, 219, 255];
  static SERIAL: AtomicUsize = AtomicUsize::new(0);

  fn require(condition: bool, message: impl Into<String>) -> Check {
    if condition { Ok(()) } else { Err(format!("FAIL: {}", message.into())) }
  }

  fn api<T>(result: tauri_runtime::Result<T>, operation: &str) -> Check<T> {
    result.map_err(|error| format!("UNKNOWN: {operation} rejected: {error}"))
  }

  fn evidence(member: &str, test: &str, assertion: &str, status: &str) {
    println!("[runtime-evidence] {}", json!({
      "member": member, "test": format!("windows_behavior_smoke::{test}"),
      "assertion": assertion, "kind": "behavior", "status": status,
    }));
  }

  fn check(member: &str, test: &str, assertion: &str, action: impl FnOnce() -> Check) -> bool {
    match action() {
      Ok(()) => { evidence(member, test, assertion, "pass"); true }
      Err(error) => {
        let status = if error.starts_with("UNKNOWN:") { "unknown" } else { "fail" };
        evidence(member, test, &format!("{assertion}; {error}"), status);
        eprintln!("{member}: {error}");
        status != "fail"
      }
    }
  }

  fn wait(mut sample: impl FnMut() -> Check<bool>, description: &str) -> Check {
    let start = Instant::now();
    loop {
      if sample()? { return Ok(()) }
      require(start.elapsed() < WAIT, format!("timed out: {description}"))?;
      thread::sleep(Duration::from_millis(40));
    }
  }

  struct NativeWindow {
    dispatcher: ElectronWindowDispatcher,
    hwnd: HWND,
    pid: u32,
  }
  impl Drop for NativeWindow {
    fn drop(&mut self) {
      // An owner's destruction may already have destroyed this native window.
      let _ = WindowDispatch::<()>::destroy(&self.dispatcher);
    }
  }

  fn create(handle: &ElectronRuntimeHandle, prefix: &str, builder: ElectronWindowBuilder) -> Check<NativeWindow> {
    create_native(handle, prefix, builder, None)
  }

  fn create_native(handle: &ElectronRuntimeHandle, prefix: &str, builder: ElectronWindowBuilder, parent: Option<HWND>) -> Check<NativeWindow> {
    let label = format!("windows-behavior-{}", SERIAL.fetch_add(1, Ordering::SeqCst));
    let title = format!("{prefix}-{label}");
    let pending = api(PendingWindow::<(), ElectronRuntime>::new(
      builder.title(&title).inner_size(260.0, 180.0).visible(true).focused(false), &label,
    ), "PendingWindow::new")?;
    let detached = api(<ElectronRuntimeHandle as RuntimeHandle<()>>::create_window(
      handle, pending, None::<fn(RawWindow<'_>)>,
    ), "create native window")?;
    let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    let start = Instant::now();
    let hwnd = loop {
      // SAFETY: the NUL-terminated title lives through the OS lookup; no proxy
      // handle, dispatcher getter or process-local object pointer is involved.
      let found = unsafe {
        if let Some(parent) = parent {
          FindWindowExW(Some(parent), None, PCWSTR::null(), PCWSTR(title.as_ptr()))
            // Discover an incorrectly owned top-level window too, so the
            // relationship assertions diagnose a parent-to-owner regression.
            .or_else(|_| FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())))
        } else {
          FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr()))
        }
      };
      if let Ok(hwnd) = found { break hwnd }
      if start.elapsed() >= WAIT {
        let _ = WindowDispatch::<()>::destroy(&detached.dispatcher);
        return Err("UNKNOWN: FindWindowW could not discover the unique Electron title on this desktop".into())
      }
      thread::sleep(Duration::from_millis(40));
    };
    let mut window = NativeWindow { dispatcher: detached.dispatcher, hwnd, pid: 0 };
    let mut class = [0u16; 256];
    // SAFETY: OS-discovered HWND and bounded output buffers, query-only calls.
    let (tid, length, style, ex_style) = unsafe {
      (
        GetWindowThreadProcessId(hwnd, Some(&mut window.pid)),
        GetClassNameW(hwnd, &mut class),
        GetWindowLongPtrW(hwnd, GWL_STYLE) as u32,
        GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32,
      )
    };
    require(tid != 0 && window.pid != 0 && window.pid != std::process::id(),
      "native observation must target an external Electron process, not the Rust proxy")?;
    require(length > 0, "GetClassNameW must return a real native class")?;
    wait(|| Ok(unsafe { IsWindowVisible(hwnd) }.as_bool()), "native window becomes visible")?;
    eprintln!("Windows HWND {hwnd:?} pid={} class={} style={style:#x} appwindow={} toolwindow={} noRedir={}",
      window.pid, String::from_utf16_lossy(&class[..length as usize]),
      ex_style & WS_EX_APPWINDOW.0 != 0, ex_style & WS_EX_TOOLWINDOW.0 != 0,
      ex_style & WS_EX_NOREDIRECTIONBITMAP.0 != 0);
    Ok(window)
  }

  fn icon(color: [u8; 4]) -> Icon<'static> {
    Icon { rgba: Cow::Owned(color.repeat(32 * 32)), width: 32, height: 32 }
  }

  fn native_icon_pixels(hwnd: HWND, slot: u32) -> Check<Vec<u8>> {
    let mut value = 0usize;
    // SAFETY: WM_GETICON is a system message, with no cross-process pointers;
    // the returned value is an OS HICON resource, not an Electron object address.
    let sent = unsafe { SendMessageTimeoutW(hwnd, WM_GETICON, WPARAM(slot as usize), LPARAM(0),
      SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT, 750, Some(&mut value)) };
    if sent.0 == 0 { return Err("UNKNOWN: WM_GETICON timed out or was denied".into()) }
    require(value != 0, format!("WM_GETICON slot {slot} must contain an icon"))?;
    let mut info = ICONINFO::default();
    // GetIconInfo creates local bitmap copies which this function must delete;
    // it does not transfer ownership of Electron's HICON to this process.
    unsafe { GetIconInfo(HICON(value as *mut std::ffi::c_void), &mut info) }
      .map_err(|error| format!("UNKNOWN: GetIconInfo on the native resource: {error}"))?;
    let result = (|| {
      require(info.fIcon.as_bool() && !info.hbmColor.0.is_null(), "WM_GETICON must expose a color icon")?;
      let mut bitmap = BITMAP::default();
      // SAFETY: local bitmap from GetIconInfo and correctly sized output storage.
      let count = unsafe { GetObjectW(HGDIOBJ(info.hbmColor.0), size_of::<BITMAP>() as i32,
        Some((&mut bitmap as *mut BITMAP).cast())) };
      require(count == size_of::<BITMAP>() as i32, "read native icon bitmap dimensions")?;
      require((1..=512).contains(&bitmap.bmWidth) && (1..=512).contains(&bitmap.bmHeight),
        format!("unexpected native icon dimensions {}x{}", bitmap.bmWidth, bitmap.bmHeight))?;
      let mut pixels = vec![0u8; bitmap.bmWidth as usize * bitmap.bmHeight as usize * 4];
      let mut dib = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
          biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: bitmap.bmWidth,
          biHeight: -bitmap.bmHeight, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0,
          ..Default::default()
        },
        ..Default::default()
      };
      // SAFETY: output covers every 32-bit pixel; all GDI resources are local.
      let dc = unsafe { GetDC(None) };
      if dc.0.is_null() { return Err("UNKNOWN: screen DC unavailable for native icon readback".into()) }
      let rows = unsafe { GetDIBits(dc, info.hbmColor, 0, bitmap.bmHeight as u32,
        Some(pixels.as_mut_ptr().cast()), &mut dib, DIB_RGB_COLORS) };
      unsafe { ReleaseDC(None, dc); }
      require(rows == bitmap.bmHeight, "read every row of the native icon bitmap")?;
      Ok(pixels)
    })();
    // SAFETY: these bitmap copies belong to this process, unlike the HICON.
    unsafe {
      if !info.hbmColor.0.is_null() { let _ = DeleteObject(HGDIOBJ(info.hbmColor.0)); }
      if !info.hbmMask.0.is_null() { let _ = DeleteObject(HGDIOBJ(info.hbmMask.0)); }
    }
    result
  }

  fn expect_icon(window: &NativeWindow, color: [u8; 4]) -> Check {
    let expected = [color[2], color[1], color[0]];
    for slot in [ICON_SMALL, ICON_BIG] {
      let pixels = native_icon_pixels(window.hwnd, slot)?;
      let matching = pixels.chunks_exact(4).filter(|pixel| pixel[..3] == expected).count();
      require(matching == pixels.len() / 4,
        format!("WM_GETICON slot {slot}: {matching}/{} resource pixels match BGR {expected:?}; first={:?}",
          pixels.len() / 4, &pixels[..4]))?;
    }
    Ok(())
  }

  fn owner_checks(handle: &ElectronRuntimeHandle, prefix: &str) -> Check {
    let a = create(handle, prefix, ElectronWindowBuilder::new())?;
    let b = create(handle, prefix, ElectronWindowBuilder::new())?;
    let first = create(handle, prefix, ElectronWindowBuilder::new().owner(b.hwnd).owner(a.hwnd))?;
    let second = create(handle, prefix, ElectronWindowBuilder::new().owner(a.hwnd).owner(b.hwnd))?;
    for (child, owner) in [(&first, &a), (&second, &b)] {
      require(child.pid == owner.pid, "owner and owned window must both be real Electron HWNDs")?;
      // GetParent alone is ambiguous: it can report a popup's owner. WS_CHILD
      // plus GW_OWNER distinguishes a true child from the official owner API.
      let (native_owner, parent, style) = unsafe {
        (GetWindow(child.hwnd, GW_OWNER).ok(), GetParent(child.hwnd).ok(),
          GetWindowLongPtrW(child.hwnd, GWL_STYLE) as u32)
      };
      require(native_owner == Some(owner.hwnd), format!("GW_OWNER={native_owner:?}, expected {:?}", owner.hwnd))?;
      require(style & WS_CHILD.0 == 0, "owner() must not create a WS_CHILD window")?;
      require(parent.is_none() || parent == Some(owner.hwnd), "GetParent must not name a different native window")?;
    }
    api(WindowDispatch::<()>::destroy(&a.dispatcher), "destroy first owner")?;
    wait(|| Ok(!unsafe { IsWindow(Some(first.hwnd)) }.as_bool()), "OS destroys first owned HWND with its final owner")?;
    require(unsafe { IsWindow(Some(second.hwnd)) }.as_bool(), "the opposite owner's window must survive")?;
    api(WindowDispatch::<()>::destroy(&b.dispatcher), "destroy second owner")?;
    wait(|| Ok(!unsafe { IsWindow(Some(second.hwnd)) }.as_bool()), "OS destroys second owned HWND with its final owner")
  }

  fn physical_desktop() -> Check {
    // Geometry and ClipCursor must use physical coordinates in both processes.
    let awareness = unsafe { GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext()) };
    if awareness == DPI_AWARENESS_PER_MONITOR_AWARE { Ok(()) }
    else { Err("UNKNOWN: native observer thread is not per-monitor DPI aware".into()) }
  }

  fn frame(hwnd: HWND) -> Check<RECT> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }
      .map_err(|error| format!("UNKNOWN: GetWindowRect: {error}"))?;
    Ok(rect)
  }

  fn client_rect(hwnd: HWND) -> Check<RECT> {
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect) }
      .map_err(|error| format!("UNKNOWN: GetClientRect: {error}"))?;
    let mut first = POINT { x: rect.left, y: rect.top };
    let mut last = POINT { x: rect.right, y: rect.bottom };
    // SAFETY: HWND is independently discovered; Win32 only copies these points.
    if !unsafe { ClientToScreen(hwnd, &mut first) }.as_bool()
      || !unsafe { ClientToScreen(hwnd, &mut last) }.as_bool() {
      return Err("UNKNOWN: native client-to-screen coordinates unavailable".into())
    }
    Ok(RECT { left: first.x, top: first.y, right: last.x, bottom: last.y })
  }

  fn work_area(hwnd: HWND) -> Check<RECT> {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
      return Err("UNKNOWN: monitor work area unavailable".into())
    }
    Ok(info.rcWork)
  }

  fn move_native(hwnd: HWND, x: i32, y: i32) -> Check {
    // Independent native stimulus, not a dispatcher position setter or cache.
    unsafe { SetWindowPos(hwnd, None, x, y, 0, 0,
      SWP_ASYNCWINDOWPOS | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) }
      .map_err(|error| format!("UNKNOWN: external SetWindowPos denied: {error}"))?;
    wait(|| { let actual = frame(hwnd)?; Ok(actual.left == x && actual.top == y) },
      "external SetWindowPos must move the real HWND")
      .map_err(|error| format!("UNKNOWN: independent native movement stimulus unavailable: {error}"))
  }

  fn parent_checks(handle: &ElectronRuntimeHandle, prefix: &str) -> Check {
    physical_desktop()?;
    let a = create(handle, prefix, ElectronWindowBuilder::new())?;
    let b = create(handle, prefix, ElectronWindowBuilder::new())?;
    let first = create_native(handle, prefix,
      ElectronWindowBuilder::new().parent(b.hwnd).parent(a.hwnd), Some(a.hwnd))?;
    let second = create_native(handle, prefix,
      ElectronWindowBuilder::new().parent(a.hwnd).parent(b.hwnd), Some(b.hwnd))?;
    let owned = create(handle, prefix, ElectronWindowBuilder::new().owner(a.hwnd))?;
    for (child, parent) in [(&first, &a), (&second, &b)] {
      require(child.pid == parent.pid, "parent and child must belong to the external Electron process")?;
      let (native_parent, owner, style) = unsafe {
        (GetParent(child.hwnd).ok(), GetWindow(child.hwnd, GW_OWNER).ok(),
          GetWindowLongPtrW(child.hwnd, GWL_STYLE) as u32)
      };
      require(native_parent == Some(parent.hwnd) && owner.is_none()
        && style & WS_CHILD.0 != 0 && style & WS_POPUP.0 == 0,
        format!("parent() must create a true child: GetParent={native_parent:?}, GW_OWNER={owner:?}, style={style:#x}"))?;
      // SetWindowPos on a child is parent-client relative. Keep the entire
      // native child frame inside that independently measured client rectangle.
      unsafe { SetWindowPos(child.hwnd, None, 17, 19, 180, 100,
        SWP_ASYNCWINDOWPOS | SWP_NOZORDER | SWP_NOACTIVATE) }
        .map_err(|error| format!("UNKNOWN: place native child in parent client area: {error}"))?;
      wait(|| {
        let outer = frame(child.hwnd)?;
        let client = client_rect(parent.hwnd)?;
        Ok(outer.left - client.left == 17 && outer.top - client.top == 19
          && outer.right <= client.right && outer.bottom <= client.bottom)
      }, "child frame must occupy the requested parent-client offset")
        .map_err(|error| format!("UNKNOWN: independent child-placement stimulus unavailable: {error}"))?;
    }
    require(unsafe { GetWindow(owned.hwnd, GW_OWNER) }.ok() == Some(a.hwnd)
      && unsafe { GetWindowLongPtrW(owned.hwnd, GWL_STYLE) } as u32 & WS_CHILD.0 == 0,
      "owner control must be a native owned top-level window, not a child")?;
    let owned_before = frame(owned.hwnd)?;
    let second_before = frame(second.hwnd)?;
    for (parent, child) in [(&a, &first), (&b, &second)] {
      let before = frame(parent.hwnd)?;
      let local = frame(child.hwnd)?;
      let client = client_rect(parent.hwnd)?;
      let work = work_area(parent.hwnd)?;
      let x = work.left + 40;
      let y = work.top + 40;
      if work.right - work.left < before.right - before.left + 100
        || work.bottom - work.top < before.bottom - before.top + 100 {
        return Err("UNKNOWN: monitor work area cannot hold the native movement probe".into())
      }
      let x = if x == before.left { x + 37 } else { x };
      let y = if y == before.top { y + 29 } else { y };
      move_native(parent.hwnd, x, y)?;
      wait(|| {
        let moved = frame(child.hwnd)?;
        let origin = client_rect(parent.hwnd)?;
        Ok(moved.left == local.left + x - before.left && moved.top == local.top + y - before.top
          && moved.left - origin.left == local.left - client.left
          && moved.top - origin.top == local.top - client.top)
      }, "moving only the real parent must carry its child at the same client offset")?;
      require(frame(owned.hwnd)? == owned_before, "moving the parent must not translate the owner control")?;
      if parent.hwnd == a.hwnd {
        require(frame(second.hwnd)? == second_before, "child of the opposite final parent must not move")?;
      }
    }
    api(WindowDispatch::<()>::destroy(&a.dispatcher), "destroy first native parent")?;
    wait(|| Ok(!unsafe { IsWindow(Some(first.hwnd)) }.as_bool()
      && !unsafe { IsWindow(Some(owned.hwnd)) }.as_bool()), "parent destruction must remove its child and owned control")?;
    require(unsafe { IsWindow(Some(b.hwnd)) }.as_bool() && unsafe { IsWindow(Some(second.hwnd)) }.as_bool(),
      "the opposite parent and child must survive")?;
    api(WindowDispatch::<()>::destroy(&b.dispatcher), "destroy opposite native parent")?;
    wait(|| Ok(!unsafe { IsWindow(Some(second.hwnd)) }.as_bool()), "opposite parent destruction removes its child")
  }

  fn clip_rect() -> Check<RECT> {
    let mut rect = RECT::default();
    unsafe { GetClipCursor(&mut rect) }.map_err(|error| format!("UNKNOWN: GetClipCursor: {error}"))?;
    Ok(rect)
  }

  fn pointer() -> Check<POINT> {
    let mut point = POINT::default();
    unsafe { GetPhysicalCursorPos(&mut point) }
      .map_err(|error| format!("UNKNOWN: GetPhysicalCursorPos: {error}"))?;
    Ok(point)
  }

  struct CursorRestore { clip: RECT, point: POINT, released: Option<RECT>, changed: bool }
  impl CursorRestore {
    fn restore(&mut self) -> Check {
      if !self.changed { return Ok(()) }
      // Preserve an unrestricted desktop as None, rather than leaving a fixed
      // full-screen clip behind. Restore first even on an early error or panic.
      let clip = if self.released == Some(self.clip) { None } else { Some(&self.clip as *const RECT) };
      unsafe { ClipCursor(clip) }
        .map_err(|error| format!("FAIL: restore original cursor confinement: {error}"))?;
      if pointer()? != self.point {
        unsafe { SetPhysicalCursorPos(self.point.x, self.point.y) }
          .map_err(|error| format!("FAIL: restore original physical pointer: {error}"))?;
      }
      require(clip_rect()? == self.clip && pointer()? == self.point, "original cursor clip and position must be restored")?;
      self.changed = false;
      Ok(())
    }
  }
  impl Drop for CursorRestore {
    fn drop(&mut self) {
      if let Err(error) = self.restore() { eprintln!("{error}"); }
    }
  }

  fn cursor_grab_checks(handle: &ElectronRuntimeHandle, prefix: &str) -> Check {
    physical_desktop()?;
    let window = create(handle, prefix, ElectronWindowBuilder::new())?;
    let mut restore = CursorRestore { clip: clip_rect()?, point: pointer()?, released: None, changed: false };
    let result = (|| {
      unsafe { ClipCursor(None) }.map_err(|error| format!("UNKNOWN: desktop forbids releasing the native clip: {error}"))?;
      restore.changed = true;
      let released = clip_rect()?;
      restore.released = Some(released);
      let work = work_area(window.hwnd)?;
      let outer = frame(window.hwnd)?;
      if work.right - work.left < outer.right - outer.left + 160
        || work.bottom - work.top < outer.bottom - outer.top + 140 {
        return Err("UNKNOWN: monitor work area cannot hold two distinct cursor boundary probes".into())
      }
      for offset in [40, 80] {
        move_native(window.hwnd, work.left + offset, work.top + offset)?;
        let target = client_rect(window.hwnd)?;
        if target.left <= released.left + 4 || target.top <= released.top + 4
          || target.right >= released.right - 4 || target.bottom >= released.bottom - 4 {
          return Err("UNKNOWN: native client rectangle has no unrestricted outside boundary points".into())
        }
        require(clip_rect()? == released && released != target, "grab must start from a distinct unrestricted desktop clip")?;
        api(WindowDispatch::<()>::set_cursor_grab(&window.dispatcher, true), "set_cursor_grab true")?;
        require(clip_rect()? == target, format!("GetClipCursor must equal external client screen bounds {target:?}"))?;
        let attempts = [
          (POINT { x: target.left - 3, y: target.top - 3 }, POINT { x: target.left, y: target.top }),
          (POINT { x: target.right + 3, y: target.bottom + 3 }, POINT { x: target.right - 1, y: target.bottom - 1 }),
        ];
        for (outside, boundary) in attempts {
          unsafe { SetPhysicalCursorPos(outside.x, outside.y) }
            .map_err(|error| format!("UNKNOWN: native pointer boundary stimulus denied: {error}"))?;
          if pointer()? != boundary {
            return Err("UNKNOWN: physical pointer boundary observation unstable (desktop or human input interference)".into())
          }
          require(clip_rect()? == target, "native clip must remain installed during outside-pointer attempts")?;
        }
        api(WindowDispatch::<()>::set_cursor_grab(&window.dispatcher, false), "set_cursor_grab false")?;
        require(clip_rect()? == released, "false must release the real desktop clip, not just cached state")?;
        for (outside, _) in attempts {
          unsafe { SetPhysicalCursorPos(outside.x, outside.y) }
            .map_err(|error| format!("UNKNOWN: unconfined pointer stimulus denied: {error}"))?;
          if pointer()? != outside {
            return Err("UNKNOWN: physical pointer cannot be stably observed outside after release".into())
          }
        }
      }
      Ok(())
    })();
    restore.restore()?;
    result
  }

  fn builder_icon_checks(handle: &ElectronRuntimeHandle, prefix: &str) -> Check {
    // WindowConfig has no icon field; opposite repeated setters must beat both
    // the earlier icon and any application-default resource supplied at creation.
    let red = api(ElectronWindowBuilder::new().icon(icon(BLUE)), "builder first icon")?;
    let red = create(handle, prefix, api(red.icon(icon(RED)), "builder replacement icon")?)?;
    expect_icon(&red, RED)?;
    let blue = api(ElectronWindowBuilder::new().icon(icon(RED)), "builder opposite first icon")?;
    let blue = create(handle, prefix, api(blue.icon(icon(BLUE)), "builder opposite replacement icon")?)?;
    expect_icon(&blue, BLUE)
  }

  fn dispatch_icon_checks(handle: &ElectronRuntimeHandle, prefix: &str) -> Check {
    let window = create(handle, prefix, api(ElectronWindowBuilder::new().icon(icon(BLUE)), "initial icon")?)?;
    expect_icon(&window, BLUE)?;
    api(WindowDispatch::<()>::set_icon(&window.dispatcher, icon(RED)), "set_icon red")?;
    expect_icon(&window, RED)?;
    api(WindowDispatch::<()>::set_icon(&window.dispatcher, icon(BLUE)), "set_icon blue")?;
    expect_icon(&window, BLUE)
  }

  fn composition() -> Check {
    match unsafe { DwmIsCompositionEnabled() } {
      Ok(enabled) if enabled.as_bool() => Ok(()),
      result => Err(format!("UNKNOWN: display affinity needs a composed interactive desktop: {result:?}")),
    }
  }

  fn expect_affinity(window: &NativeWindow, protected: bool) -> Check {
    let expected = if protected { WDA_EXCLUDEFROMCAPTURE.0 } else { WDA_NONE.0 };
    let mut actual = u32::MAX;
    wait(|| {
      // SAFETY: read-only OS query on the independently discovered external HWND.
      unsafe { GetWindowDisplayAffinity(window.hwnd, &mut actual) }
        .map_err(|error| format!("UNKNOWN: external GetWindowDisplayAffinity unavailable: {error}"))?;
      if protected && actual == 1 {
        return Err("UNKNOWN: WDA_MONITOR only; this desktop cannot prove WDA_EXCLUDEFROMCAPTURE".into())
      }
      Ok(actual == expected)
    }, &format!("GetWindowDisplayAffinity must become {expected:#x}"))?;
    require(actual == expected, format!("native display affinity {actual:#x} != {expected:#x}"))
  }

  fn builder_protection_checks(handle: &ElectronRuntimeHandle, prefix: &str) -> Check {
    composition()?;
    for protected in [true, false] {
      let config = WindowConfig { content_protected: !protected, ..Default::default() };
      let window = create(handle, prefix,
        ElectronWindowBuilder::with_config(&config).content_protected(protected))?;
      expect_affinity(&window, protected)?;
    }
    Ok(())
  }

  fn dispatch_protection_checks(handle: &ElectronRuntimeHandle, prefix: &str) -> Check {
    composition()?;
    let window = create(handle, prefix, ElectronWindowBuilder::new().content_protected(false))?;
    expect_affinity(&window, false)?;
    api(WindowDispatch::<()>::set_content_protected(&window.dispatcher, true), "set_content_protected true")?;
    expect_affinity(&window, true)?;
    api(WindowDispatch::<()>::set_content_protected(&window.dispatcher, false), "set_content_protected false")?;
    expect_affinity(&window, false)
  }

  fn checks(handle: &ElectronRuntimeHandle, prefix: &str) -> bool {
    let mut passed = true;
    passed &= check("WindowBuilder::owner", "owner", "two fresh opposite final owner setters produce exact external GW_OWNER without WS_CHILD; each owned HWND is destroyed only with its final owner", || owner_checks(handle, prefix));
    passed &= check("WindowBuilder::parent", "parent", "opposite fresh final parents create external WS_CHILD/non-WS_POPUP HWNDs with exact GetParent and no GW_OWNER; parent-client offsets follow native parent movement unlike an owner control, and destruction removes only the final parent's children", || parent_checks(handle, prefix));
    passed &= check("WindowDispatch::set_cursor_grab", "cursor_grab", "at two distinct native client screen rectangles GetClipCursor matches grab bounds, outside physical pointer attempts clamp to opposite edges, false releases the clip and permits outside positions; original clip and pointer are restored", || cursor_grab_checks(handle, prefix));
    passed &= check("WindowBuilder::icon", "builder_icons", "two fresh opposite icon replacements expose the final RGB color in every native ICON_SMALL and ICON_BIG resource pixel", || builder_icon_checks(handle, prefix));
    passed &= check("WindowDispatch::set_icon", "dispatch_icons", "native WM_GETICON resource pixels change blue to red to blue in both small and big slots after opposite set_icon calls", || dispatch_icon_checks(handle, prefix));
    passed &= check("WindowBuilder::content_protected", "builder_protection", "two fresh content_protected setters override opposite WindowConfig values with native WDA_EXCLUDEFROMCAPTURE and WDA_NONE", || builder_protection_checks(handle, prefix));
    passed &= check("WindowDispatch::set_content_protected", "dispatch_protection", "external GetWindowDisplayAffinity changes WDA_NONE to WDA_EXCLUDEFROMCAPTURE to WDA_NONE after opposite dispatcher setters", || dispatch_protection_checks(handle, prefix));
    for (member, reason) in [
      ("WindowBuilder::window_classname", "not asserted: current creation bridge does not apply a custom native class; logged GetClassNameW is diagnostic only"),
      ("WindowBuilder::no_redirection_bitmap", "not asserted: current creation bridge does not apply this native style; a default bit is not setter evidence"),
      ("WindowBuilder::skip_taskbar", "not asserted: Electron and Tao use ITaskbarList; APPWINDOW/TOOLWINDOW observations alone cannot prove an actual shell tab"),
      ("WindowDispatch::set_skip_taskbar", "not asserted: opposite dispatcher calls need an independent shell-tab observer, not unchanged extended styles or an Ok result"),
    ] {
      evidence(member, "uncovered", reason, "unknown");
    }
    passed
  }

  pub fn run() -> Check {
    thread::spawn(|| {
      thread::sleep(Duration::from_secs(180));
      eprintln!("Windows behavior: UNKNOWN (180s watchdog expired; unfinished assertions have no pass evidence)");
      std::process::exit(124);
    });
    let prefix = format!("electron-win-native-{}-{}", std::process::id(),
      SystemTime::now().duration_since(UNIX_EPOCH).map_err(|error| error.to_string())?.as_nanos());
    // Runtime initialization stays on main; only handle operations run on the
    // worker while run_return pumps the real native creation/dispatch requests.
    let runtime = api(<ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs::default()), "Runtime::new")?;
    let handle = <ElectronRuntime as Runtime<()>>::handle(&runtime);
    let keeper = api(PendingWindow::<(), ElectronRuntime>::new(
      ElectronWindowBuilder::new().title(format!("{prefix}-keeper")).visible(false), "windows-behavior-keeper",
    ), "keeper pending window")?;
    let _keeper = api(<ElectronRuntime as Runtime<()>>::create_window(&runtime, keeper, None::<fn(RawWindow<'_>)>), "keeper native window")?;
    let (ready_tx, ready_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
      let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ready_rx.recv_timeout(WAIT).expect("main run_return must reach Ready");
        checks(&handle, &prefix)
      }));
      let passed = matches!(result, Ok(true));
      let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(&handle, i32::from(!passed));
      passed
    });
    let code = <ElectronRuntime as Runtime<()>>::run_return(runtime, move |event| {
      if let RunEvent::Ready = event { let _ = ready_tx.send(()); }
    });
    let passed = worker.join().map_err(|_| "FAIL: native observation worker panicked".to_string())?;
    require(passed && code == 0, format!("native checks or runtime failed (exit={code})"))?;
    println!("Windows native behavior checks completed; uncovered members remain unknown");
    Ok(())
  }
}
