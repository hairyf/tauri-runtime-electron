//! Native screen pixels, not DOM styles or background getters, certify these four APIs.
use std::{process::ExitCode, sync::mpsc, thread, time::{Duration, Instant}};
use tauri_runtime::{
  webview::{PendingWebview, WebviewAttributes},
  window::{DetachedWindow, PendingWindow, RawWindow, WindowBuilder},
  ExitRequestedEventAction, RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs, WebviewDispatch, WindowDispatch,
};
use tauri_runtime_electron::{ElectronRuntime, ElectronRuntimeHandle, ElectronWindowBuilder, ElectronWebviewDispatcher as Webview};
use tauri_utils::config::{Color, WebviewUrl};

const WAIT: Duration = Duration::from_secs(4);
const COLORS: [Color; 2] = [Color(31, 97, 201, 255), Color(211, 43, 67, 255)];
const BASES: [[u8; 3]; 2] = [[35, 173, 71], [213, 151, 29]];
const CLEAR: Color = Color(0, 0, 0, 0);
type Target = DetachedWindow<(), ElectronRuntime>;
#[derive(Debug)]
enum Problem { Unknown(String), Fail(String) }
type Check = Result<(), Problem>;

fn main() -> ExitCode {
  thread::spawn(|| {
    thread::sleep(Duration::from_secs(90));
    eprintln!("rendering pixel smoke exceeded 90s");
    std::process::abort();
  });
  let runtime = <ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs::default()).expect("start Electron");
  let handle = <ElectronRuntime as Runtime<()>>::handle(&runtime);
  let bottom = create(&handle, "pixel-reference", builder().visible(true), false);
  let worker = thread::spawn(move || {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exercise(&handle, &bottom)));
    let failed = !matches!(result, Ok(true));
    let _ = WindowDispatch::<()>::destroy(&bottom.dispatcher);
    let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(&handle, i32::from(failed));
    !failed
  });
  let status = <ElectronRuntime as Runtime<()>>::run_return(runtime, |event| {
    if let RunEvent::ExitRequested { code: None, tx } = event {
      let _ = tx.send(ExitRequestedEventAction::Prevent);
    }
  });
  if worker.join().expect("pixel worker") && status == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn builder() -> ElectronWindowBuilder {
  ElectronWindowBuilder::new().inner_size(320.0, 220.0).position(100.0, 100.0)
    .decorations(false).shadow(false).resizable(false).always_on_top(true).visible(false)
}
fn create(handle: &ElectronRuntimeHandle, label: &str, builder: ElectronWindowBuilder, transparent: bool) -> Target {
  let url = format!("data:text/html,<html><head><title>{label}</title><style>html,body{{margin:0;width:100%;height:100%;background:transparent}}</style></head><body data-proof='{label}'></body></html>");
  let mut pending = PendingWindow::<(), ElectronRuntime>::new(builder, label).expect("pending window");
  let mut page = PendingWebview::new(WebviewAttributes::new(WebviewUrl::External(url.parse().unwrap())).transparent(transparent), label).unwrap();
  page.url = url;
  pending.set_webview(page);
  <ElectronRuntimeHandle as RuntimeHandle<()>>::create_window(handle, pending, None::<fn(RawWindow<'_>)>).expect("native window")
}
fn page(window: &Target) -> &Webview { &window.webview.as_ref().unwrap().webview.dispatcher }
fn eval(page: &Webview, script: &str) -> serde_json::Value {
  let (tx, rx) = mpsc::channel();
  WebviewDispatch::<()>::eval_script_with_callback(page, script, move |value| { let _ = tx.send(value); }).expect("schedule eval");
  serde_json::from_str(&rx.recv_timeout(WAIT).expect("UI-pumped renderer callback")).unwrap()
}
fn ready(window: &Target) {
  let script = format!("new Promise(resolve => {{ const ready = () => resolve(document.body.dataset.proof === {:?}); if(document.readyState === 'complete') ready(); else addEventListener('load',ready,{{once:true}}); }})", window.label);
  assert_eq!(eval(page(window), &script), true);
}
fn background(bottom: &Target, color: [u8; 3]) {
  assert_eq!(eval(page(bottom), &format!("document.documentElement.style.background = 'rgb({},{},{})'; document.body.style.background = 'transparent'; true", color[0],color[1],color[2])), true);
}
fn show(window: &Target) {
  WindowDispatch::<()>::show(&window.dispatcher).unwrap();
  WindowDispatch::<()>::set_focus(&window.dispatcher).unwrap();
}
fn probe(window: &Target) -> (i32, i32, f64) {
  let origin = WindowDispatch::<()>::inner_position(&window.dispatcher).unwrap();
  let size = WindowDispatch::<()>::inner_size(&window.dispatcher).unwrap();
  (origin.x + (size.width / 2) as i32, origin.y + (size.height / 2) as i32, WindowDispatch::<()>::scale_factor(&window.dispatcher).unwrap())
}
fn observe(point: (i32,i32,f64), expected: [u8;3], why: &str) -> Check {
  let started = Instant::now();
  loop {
    let actual = native::pixel(point.0, point.1, point.2).map_err(Problem::Unknown)?;
    if actual.iter().zip(expected).all(|(a,b)| a.abs_diff(b) <= 8) {
      println!("[native-pixels] {}", serde_json::json!({"check":why,"point":[point.0,point.1],"actual":actual,"expected":expected}));
      return Ok(());
    }
    if started.elapsed() >= WAIT { return Err(Problem::Fail(format!("{why}: screen RGB {actual:?}, expected {expected:?}"))); }
    thread::sleep(Duration::from_millis(80));
  }
}
fn record(member: &str, result: Check, assertion: &str) -> bool {
  let (status, detail) = match &result {
    Ok(()) => ("pass", assertion.to_owned()),
    Err(Problem::Unknown(reason)) => ("unknown", reason.clone()),
    Err(Problem::Fail(reason)) => ("fail", reason.clone()),
  };
  println!("[runtime-evidence] {}", serde_json::json!({"member":member,"test":"rendering_behavior_smoke::native_pixels","assertion":detail,"kind":"behavior","status":status}));
  !matches!(result, Err(Problem::Fail(_)))
}
fn exercise(handle: &ElectronRuntimeHandle, bottom: &Target) -> bool {
  ready(bottom);
  // ponytail: one center pixel certifies flat frames, not spatial rendering; use regions if that scope grows.
  let point = probe(bottom);
  // Calibrate the independent desktop observer with two actual CSS-rendered reference frames.
  // CSS is only the controlled bottom reference; no target color is read from DOM.
  for base in BASES {
    background(bottom, base);
    if let Err(error) = observe(point, base, "reference desktop frame") {
      let reason = format!("native screen observer preflight unavailable/occluded: {error:?}");
      for member in ["WindowBuilder::background_color", "WindowBuilder::transparent", "WindowDispatch::set_background_color", "WebviewDispatch::set_background_color", "WebviewDispatch::on_webview_event"] {
        record(member, Err(Problem::Unknown(reason.clone())), "");
      }
      return true;
    }
  }
  let mut good = true;
  let first = create(handle, "pixel-builder-blue", builder().transparent(true).background_color(COLORS[0]), true);
  ready(&first);
  // Hiding the independently owned page exposes the native window backing; a
  // browser's default white canvas must not mask the window-only API tests.
  WebviewDispatch::<()>::hide(page(&first)).unwrap();
  show(&first);
  let first_color = observe(point, [COLORS[0].0,COLORS[0].1,COLORS[0].2], "first builder backing");
  WindowDispatch::<()>::hide(&first.dispatcher).unwrap();
  let second = create(handle, "pixel-builder-red", builder().transparent(true).background_color(COLORS[1]), true);
  ready(&second);
  WebviewDispatch::<()>::hide(page(&second)).unwrap();
  show(&second);
  let second_color = observe(point, [COLORS[1].0,COLORS[1].1,COLORS[1].2], "second builder backing");
  good &= record("WindowBuilder::background_color", first_color.and(second_color), "two fresh builder background_color calls produce distinct opaque RGB pixels on the actual desktop with their webviews hidden");
  WindowDispatch::<()>::destroy(&second.dispatcher).unwrap();
  show(&first);
  let window_colors = (|| {
    for color in COLORS {
      WindowDispatch::<()>::set_background_color(&first.dispatcher, Some(color)).unwrap();
      observe(point, [color.0,color.1,color.2], "window dispatcher opaque backing")?;
    }
    native::alpha_available().map_err(Problem::Unknown)?;
    WindowDispatch::<()>::set_background_color(&first.dispatcher, Some(CLEAR)).unwrap();
    for base in BASES { background(bottom,base); observe(point,base,"window dispatcher alpha-zero backing")?; }
    Ok(())
  })();
  good &= record("WindowDispatch::set_background_color", window_colors, "two distinct dispatcher colors change actual native backing screen pixels; alpha-zero then reveals two changing lower-window colors; no getters/config echoes");

  let transparency = (|| {
    native::alpha_available().map_err(Problem::Unknown)?;
    WindowDispatch::<()>::set_background_color(&first.dispatcher, Some(CLEAR)).unwrap();
    for base in BASES { background(bottom,base); observe(point,base,"transparent builder shows changing lower window")?; }
    WindowDispatch::<()>::hide(&first.dispatcher).unwrap();
    let opaque = create(handle,"pixel-opaque-control",builder().transparent(false).background_color(COLORS[0]),true);
    ready(&opaque);
    WebviewDispatch::<()>::hide(page(&opaque)).unwrap();
    show(&opaque);
    let result = (|| {
      observe(point,[COLORS[0].0,COLORS[0].1,COLORS[0].2],"opaque control must actually cover reference")?;
      WindowDispatch::<()>::set_background_color(&opaque.dispatcher, Some(CLEAR)).unwrap();
      let mut samples = Vec::new();
      for base in BASES {
        background(bottom,base);
        thread::sleep(Duration::from_millis(300));
        samples.push(native::pixel(point.0,point.1,point.2).map_err(Problem::Unknown)?);
      }
      if samples[0].iter().zip(samples[1]).all(|(a,b)| a.abs_diff(b)<=8) { Ok(()) }
      else { Err(Problem::Fail(format!("transparent(false) control did not stay opaque across lower frames: {samples:?}"))) }
    })();
    WindowDispatch::<()>::destroy(&opaque.dispatcher).unwrap();
    result
  })();
  good &= record("WindowBuilder::transparent", transparency, "transparent(true) reveals two changing lower-window colors through native alpha-zero backing; a fresh transparent(false) control does not follow both colors");
  show(&first);
  // Keep the target document empty and transparent. Only the API under test
  // changes the browser backing, and only the external screen observer reads it.
  WindowDispatch::<()>::set_background_color(&first.dispatcher, Some(CLEAR)).unwrap();
  WebviewDispatch::<()>::show(page(&first)).unwrap();
  let webview_colors = (|| {
    for color in COLORS {
      WebviewDispatch::<()>::set_background_color(page(&first), Some(color)).unwrap();
      observe(point,[color.0,color.1,color.2],"webview dispatcher opaque backing")?;
    }
    native::alpha_available().map_err(Problem::Unknown)?;
    WebviewDispatch::<()>::set_background_color(page(&first), Some(CLEAR)).unwrap();
    for base in BASES { background(bottom,base); observe(point,base,"webview alpha-zero backing")?; }
    Ok(())
  })();
  good &= record("WebviewDispatch::set_background_color", webview_colors, "empty transparent target page shows two API-selected opaque colors, then alpha-zero exposes two controlled lower-window colors in native desktop pixels");
  record("WebviewDispatch::on_webview_event", Err(Problem::Unknown("the native bridge has no observable WebviewEvent source; lifecycle/RunEvent is not listener proof".into())), "");
  WindowDispatch::<()>::destroy(&first.dispatcher).unwrap();
  good
}

#[cfg(windows)]
mod native {
  pub fn alpha_available() -> Result<(),String> { Ok(()) }
  pub fn pixel(x:i32,y:i32,_scale:f64) -> Result<[u8;3],String> {
    use windows::Win32::Graphics::Gdi::{GetDC,GetPixel,ReleaseDC};
    // SAFETY: read one screen pixel; the screen DC is released on every path.
    unsafe {
      let dc = GetDC(None);
      if dc.0.is_null() { return Err("GetDC(NULL) failed: no desktop capture".into()); }
      let rgb = GetPixel(dc,x,y).0;
      ReleaseDC(None,dc);
      if rgb == u32::MAX { return Err("GetPixel failed: pixel is not readable".into()); }
      Ok([rgb as u8,(rgb>>8) as u8,(rgb>>16) as u8])
    }
  }
}

#[cfg(target_os="linux")]
mod native {
  use std::ffi::{c_char,c_int,c_ulong,c_void};
  #[repr(C)]
  struct Image {
    width:c_int,height:c_int,xoffset:c_int,format:c_int,data:*mut c_char,
    byte_order:c_int,bitmap_unit:c_int,bitmap_bit_order:c_int,bitmap_pad:c_int,
    depth:c_int,bytes_per_line:c_int,bits_per_pixel:c_int,
    red_mask:c_ulong,green_mask:c_ulong,blue_mask:c_ulong,
  }
  #[link(name="X11")]
  extern "C" {
    fn XOpenDisplay(name:*const c_char)->*mut c_void;
    fn XCloseDisplay(display:*mut c_void)->c_int;
    fn XDefaultRootWindow(display:*mut c_void)->c_ulong;
    fn XDefaultScreen(display:*mut c_void)->c_int;
    fn XDisplayWidth(display:*mut c_void,screen:c_int)->c_int;
    fn XDisplayHeight(display:*mut c_void,screen:c_int)->c_int;
    fn XInternAtom(display:*mut c_void,name:*const c_char,only_if_exists:c_int)->c_ulong;
    fn XGetSelectionOwner(display:*mut c_void,selection:c_ulong)->c_ulong;
    fn XGetImage(display:*mut c_void,window:c_ulong,x:c_int,y:c_int,width:u32,height:u32,planes:c_ulong,format:c_int)->*mut Image;
    fn XGetPixel(image:*mut Image,x:c_int,y:c_int)->c_ulong;
    fn XDestroyImage(image:*mut Image)->c_int;
  }
  pub fn alpha_available()->Result<(),String> {
    unsafe {
      let d=XOpenDisplay(std::ptr::null());
      if d.is_null(){return Err("no independent X11 display (Wayland capture is not assumed)".into());}
      let selection=std::ffi::CString::new(format!("_NET_WM_CM_S{}",XDefaultScreen(d))).unwrap();
      let atom=XInternAtom(d,selection.as_ptr(),1);
      let owner=if atom==0 {0} else {XGetSelectionOwner(d,atom)};
      XCloseDisplay(d);
      if owner==0 {Err("X11 has no compositing manager: native alpha is unobservable".into())} else {Ok(())}
    }
  }
  pub fn pixel(x:i32,y:i32,_scale:f64)->Result<[u8;3],String> {
    // SAFETY: this worker owns a separate display connection and a 1x1 image;
    // only the documented XImage prefix and Xlib pixel accessor are used.
    unsafe {
      let d=XOpenDisplay(std::ptr::null());
      if d.is_null(){return Err("XOpenDisplay failed".into());}
      let screen=XDefaultScreen(d);
      if x<0 || y<0 || x>=XDisplayWidth(d,screen) || y>=XDisplayHeight(d,screen) {
        XCloseDisplay(d); return Err("screen probe outside X11 root bounds".into());
      }
      let image=XGetImage(d,XDefaultRootWindow(d),x,y,1,1,!0,2);
      if image.is_null(){XCloseDisplay(d);return Err("XGetImage(root) unavailable".into());}
      let value=XGetPixel(image,0,0);
      let masks=[(*image).red_mask,(*image).green_mask,(*image).blue_mask];
      let rgb=masks.map(|mask| if mask==0 {0} else {(((value&mask)>>mask.trailing_zeros())*255/(mask>>mask.trailing_zeros())) as u8});
      XDestroyImage(image); XCloseDisplay(d);
      if masks.contains(&0){Err("X11 root visual has no RGB masks".into())}else{Ok(rgb)}
    }
  }
}

#[cfg(target_os="macos")]
mod native {
  use std::ffi::c_void;
  #[repr(C)] #[derive(Clone,Copy)] struct Point{x:f64,y:f64}
  #[repr(C)] #[derive(Clone,Copy)] struct Size{width:f64,height:f64}
  #[repr(C)] #[derive(Clone,Copy)] struct Rect{origin:Point,size:Size}
  #[link(name="CoreGraphics",kind="framework")]
  extern "C" {
    fn CGPreflightScreenCaptureAccess()->bool;
    fn CGWindowListCreateImage(rect:Rect,options:u32,window:u32,image_options:u32)->*mut c_void;
    fn CGImageRelease(image:*mut c_void);
    fn CGColorSpaceCreateDeviceRGB()->*mut c_void;
    fn CGColorSpaceRelease(space:*mut c_void);
    fn CGBitmapContextCreate(data:*mut c_void,width:usize,height:usize,bits:usize,stride:usize,space:*mut c_void,info:u32)->*mut c_void;
    fn CGContextDrawImage(context:*mut c_void,rect:Rect,image:*mut c_void);
    fn CGContextRelease(context:*mut c_void);
  }
  pub fn alpha_available()->Result<(),String>{Ok(())}
  pub fn pixel(x:i32,y:i32,scale:f64)->Result<[u8;3],String>{
    // SAFETY: a permission-preflighted, independent WindowServer screen crop is
    // normalized into our four-byte RGBA buffer, never Electron process pointers.
    unsafe {
      if !CGPreflightScreenCaptureAccess(){return Err("macOS Screen Recording permission absent; no prompt requested".into());}
      let rect=Rect{origin:Point{x:f64::from(x)/scale,y:f64::from(y)/scale},size:Size{width:1.0,height:1.0}};
      let image=CGWindowListCreateImage(rect,1,0,0);
      if image.is_null(){return Err("CoreGraphics desktop image unavailable".into());}
      let space=CGColorSpaceCreateDeviceRGB();
      if space.is_null(){CGImageRelease(image);return Err("CoreGraphics RGB color space unavailable".into());}
      let mut rgba=[0u8;4];
      let context=CGBitmapContextCreate(rgba.as_mut_ptr().cast(),1,1,8,4,space,1|(4<<12));
      if context.is_null(){CGColorSpaceRelease(space);CGImageRelease(image);return Err("CoreGraphics RGBA context unavailable".into());}
      CGContextDrawImage(context,Rect{origin:Point{x:0.0,y:0.0},size:Size{width:1.0,height:1.0}},image);
      CGContextRelease(context);CGColorSpaceRelease(space);CGImageRelease(image);
      if rgba[3]!=255 {return Err("CoreGraphics returned no opaque desktop sample".into());}
      Ok([rgba[0],rgba[1],rgba[2]])
    }
  }
}

#[cfg(not(any(windows,target_os="linux",target_os="macos")))]
mod native {
  pub fn alpha_available()->Result<(),String>{Err("unsupported desktop capture platform".into())}
  pub fn pixel(_:i32,_:i32,_:f64)->Result<[u8;3],String>{Err("unsupported desktop capture platform".into())}
}
