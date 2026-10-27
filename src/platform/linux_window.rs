//! X11 frame/function requests. The window manager decides the visible result.
//!
//! XCB returns errors per request without changing another toolkit's error handler.
use std::{ffi::{c_char, c_int, c_void}, ptr, sync::Mutex};

const FUNCTIONS: u32 = 1;
const DECORATIONS: u32 = 2;
const ALL: u32 = 1;
const CLOSE: u32 = 1 << 5;
static REQUESTS: Mutex<()> = Mutex::new(());

#[repr(C)]
#[derive(Clone, Copy)]
struct Cookie { sequence: u32 }
#[repr(C)]
struct AtomReply { response_type: u8, pad: u8, sequence: u16, length: u32, atom: u32 }
#[repr(C)]
struct PropertyReply {
  response_type: u8, format: u8, sequence: u16, length: u32,
  property_type: u32, bytes_after: u32, value_len: u32, pad: [u8; 12],
}
impl PropertyReply {
  fn has_values(&self, property_type: u32, count: u32) -> bool {
    self.property_type == property_type && self.format == 32 && self.length == count
      && self.value_len == count && self.bytes_after == 0
  }
}
#[link(name = "xcb")]
extern "C" {
  fn xcb_connect(display: *const c_char, screen: *mut c_int) -> *mut c_void;
  fn xcb_connection_has_error(connection: *mut c_void) -> c_int;
  fn xcb_disconnect(connection: *mut c_void);
  fn xcb_intern_atom(connection: *mut c_void, only_if_exists: u8, length: u16, name: *const c_char) -> Cookie;
  fn xcb_intern_atom_reply(connection: *mut c_void, cookie: Cookie, error: *mut *mut u8) -> *mut AtomReply;
  fn xcb_get_property(connection: *mut c_void, delete: u8, window: u32, property: u32, property_type: u32, offset: u32, length: u32) -> Cookie;
  fn xcb_get_property_reply(connection: *mut c_void, cookie: Cookie, error: *mut *mut u8) -> *mut PropertyReply;
  fn xcb_get_property_value(reply: *const PropertyReply) -> *mut c_void;
  fn xcb_change_property_checked(connection: *mut c_void, mode: u8, window: u32, property: u32, property_type: u32, format: u8, length: u32, data: *const c_void) -> Cookie;
  fn xcb_request_check(connection: *mut c_void, cookie: Cookie) -> *mut u8;
  fn free(memory: *mut c_void);
}

struct Connection(*mut c_void);
impl Drop for Connection {
  fn drop(&mut self) {
    // SAFETY: this helper exclusively owns the connection, if one was allocated.
    if !self.0.is_null() { unsafe { xcb_disconnect(self.0) }; }
  }
}
impl Connection {
  fn atom(&self, name: &[u8]) -> Result<u32, String> {
    let mut error = ptr::null_mut();
    // SAFETY: the byte string has the supplied length, and error is writable.
    let reply = unsafe {
      let cookie = xcb_intern_atom(self.0, 0, name.len() as u16, name.as_ptr().cast());
      xcb_intern_atom_reply(self.0, cookie, &mut error)
    };
    if let Err(error) = request_error(error) {
      unsafe { free(reply.cast()) };
      return Err(error);
    }
    if reply.is_null() { return Err("X11 atom reply was unavailable".into()); }
    let atom = unsafe { (*reply).atom };
    unsafe { free(reply.cast()) };
    Ok(atom)
  }

  fn property<const N: usize>(&self, window: u32, atom: u32, property_type: u32) -> Result<Option<[u32; N]>, String> {
    let mut error = ptr::null_mut();
    // SAFETY: this connection is exclusively owned; XCB allocates the reply.
    let reply = unsafe {
      let cookie = xcb_get_property(self.0, 0, window, atom, 0, 0, N as u32);
      xcb_get_property_reply(self.0, cookie, &mut error)
    };
    if let Err(error) = request_error(error) {
      unsafe { free(reply.cast()) };
      return Err(error);
    }
    if reply.is_null() { return Err("X11 property reply was unavailable".into()); }
    // SAFETY: validate the format, item count and tail length before copying.
    let result = unsafe {
      if (*reply).property_type == 0 { Ok(None) }
      else if (*reply).has_values(property_type, N as u32) {
        let mut values = [0; N];
        ptr::copy_nonoverlapping(xcb_get_property_value(reply).cast::<u32>(), values.as_mut_ptr(), N);
        Ok(Some(values))
      } else { Err("invalid X11 property reply".into()) }
    };
    unsafe { free(reply.cast()) };
    result
  }
}

fn request_error(error: *mut u8) -> Result<(), String> {
  if error.is_null() { return Ok(()); }
  // SAFETY: XCB allocates a generic error whose second byte is error_code.
  let code = unsafe { *error.add(1) };
  unsafe { free(error.cast()) };
  Err(format!("X11 window request failed with error {code}"))
}

fn hints<R>((window, process_id): (u32, u32), update: impl FnOnce(&mut [u32; 5]) -> (R, bool)) -> Result<R, String> {
  if window == 0 || process_id == 0 { return Err("invalid X11 window identity".into()); }
  // ponytail: serialize read-modify-write requests; per-window locks if contention matters.
  let _guard = REQUESTS.lock().map_err(|_| "X11 window request lock poisoned")?;
  // SAFETY: a null display name uses DISPLAY; the connection is never shared.
  let connection = Connection(unsafe { xcb_connect(ptr::null(), ptr::null_mut()) });
  if connection.0.is_null() || unsafe { xcb_connection_has_error(connection.0) } != 0 {
    return Err("could not connect to the X11 display".into());
  }
  // Session variables may say Wayland while Electron actually uses XWayland.
  // Verify the real X11 resource belongs to Electron before reading or mutating it;
  // a Wayland accelerated-widget number is not an XID, even if numbers collide.
  let pid_atom = connection.atom(b"_NET_WM_PID")?;
  if connection.property::<1>(window, pid_atom, 6)? != Some([process_id]) { // XA_CARDINAL
    return Err("native frame/function requests require an Electron-owned X11 window".into());
  }
  let atom = connection.atom(b"_MOTIF_WM_HINTS")?;
  let mut values = connection.property::<5>(window, atom, atom)?.unwrap_or([0; 5]);
  let (result, changed) = update(&mut values);
  if changed {
    // SAFETY: five native u32 values match X11's format=32 property representation.
    let error = unsafe {
      let cookie = xcb_change_property_checked(connection.0, 0, window, atom, atom, 32, 5, values.as_ptr().cast());
      xcb_request_check(connection.0, cookie)
    };
    request_error(error)?;
  }
  Ok(result)
}

fn change_closable(hints: &mut [u32; 5], enabled: bool) {
  if hints[0] & FUNCTIONS == 0 { hints[1] = ALL; }
  hints[0] |= FUNCTIONS;
  // ALL reverses the meaning of the other bits: those bits are exclusions.
  if enabled != (hints[1] & ALL != 0) { hints[1] |= CLOSE; }
  else { hints[1] &= !CLOSE; }
}

pub(crate) fn set_closable(window: (u32, u32), enabled: bool) -> Result<(), String> {
  hints(window, |hints| { change_closable(hints, enabled); ((), true) })
}
pub(crate) fn is_closable(window: (u32, u32)) -> Result<bool, String> {
  hints(window, |hints| ((hints[0] & FUNCTIONS == 0) || ((hints[1] & CLOSE != 0) != (hints[1] & ALL != 0)), false))
}
pub(crate) fn set_decorations(window: (u32, u32), enabled: bool) -> Result<(), String> {
  hints(window, |hints| { hints[0] |= DECORATIONS; hints[2] = u32::from(enabled); ((), true) })
}
pub(crate) fn is_decorated(window: (u32, u32)) -> Result<bool, String> {
  hints(window, |hints| (hints[0] & DECORATIONS == 0 || hints[2] != 0, false))
}

#[test]
fn property_reply_rejects_short_and_inconsistent_payloads() {
  assert_eq!(std::mem::size_of::<PropertyReply>(), 32);
  let mut reply = PropertyReply {
    response_type: 1, format: 32, sequence: 0, length: 5,
    property_type: 42, bytes_after: 0, value_len: 5, pad: [0; 12],
  };
  assert!(reply.has_values(42, 5));
  for length in [0, 4, 6, 11, u32::MAX] {
    reply.length = length;
    assert!(!reply.has_values(42, 5));
  }
  reply.length = 5;
  reply.value_len = 4;
  assert!(!reply.has_values(42, 5));
  reply.value_len = 5;
  reply.bytes_after = 4;
  assert!(!reply.has_values(42, 5));
  reply.bytes_after = 0;
  reply.length = 1;
  reply.value_len = 1;
  reply.property_type = 6;
  assert!(reply.has_values(6, 1));
  assert!(!reply.has_values(42, 1));
}

#[test]
fn closable_preserves_unrelated_motif_bits_and_all_inversion() {
  for functions in [0, ALL, 1 << 1, ALL | (1 << 2), CLOSE | (1 << 4), u32::MAX] {
    let original = [FUNCTIONS | DECORATIONS | 8, functions, 17, 23, 42];
    let mut hints = original;
    for enabled in [false, true, false] {
      change_closable(&mut hints, enabled);
      assert_eq!((hints[1] & CLOSE != 0) != (hints[1] & ALL != 0), enabled);
      assert_eq!(hints[1] & !CLOSE, original[1] & !CLOSE);
      assert_eq!(hints[0], original[0]);
      assert_eq!(&hints[2..], &original[2..]);
    }
  }
  let mut unset = [DECORATIONS, 0, 1, 0, 0];
  change_closable(&mut unset, false);
  assert_eq!(unset, [FUNCTIONS | DECORATIONS, ALL | CLOSE, 1, 0, 0]);
}
