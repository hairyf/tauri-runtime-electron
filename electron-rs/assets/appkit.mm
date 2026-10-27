#import <AppKit/AppKit.h>
#include <pthread.h>
#include <stdint.h>
#include <string.h>
#include <cmath>
#import <objc/runtime.h>

// AppKit relayouts native caption controls after title/size/fullscreen changes.
// Retain the inset with the owning window; notifications never retain the window.
@interface DSHWindowButtonInset : NSObject
@property(nonatomic, weak) NSWindow* window;
@property(nonatomic) NSPoint inset;
- (void)apply;
@end

@implementation DSHWindowButtonInset
- (void)apply {
  NSWindow* window = self.window;
  NSButton* close = [window standardWindowButton:NSWindowCloseButton];
  NSButton* mini = [window standardWindowButton:NSWindowMiniaturizeButton];
  NSButton* zoom = [window standardWindowButton:NSWindowZoomButton];
  NSView* container = close.superview.superview;
  if (!window || window.styleMask & NSWindowStyleMaskFullScreen || !close || !mini || !zoom || !container) return;
  // Match Tao's logical-coordinate inset of the actual standard NSButtons.
  NSRect frame = container.frame;
  frame.size.height = close.frame.size.height + self.inset.y;
  frame.origin.y = window.frame.size.height - frame.size.height;
  if (!NSEqualRects(container.frame, frame)) container.frame = frame;
  CGFloat spacing = mini.frame.origin.x - close.frame.origin.x;
  NSButton* buttons[] = { close, mini, zoom };
  for (int i = 0; i < 3; ++i) {
    NSPoint origin = buttons[i].frame.origin;
    origin.x = self.inset.x + i * spacing;
    if (!NSEqualPoints(buttons[i].frame.origin, origin)) [buttons[i] setFrameOrigin:origin];
  }
}
- (void)layoutChanged:(NSNotification*)notification { [self apply]; }
- (void)dealloc { [NSNotificationCenter.defaultCenter removeObserver:self]; }
@end

static char buttonInsetKey;

// Stable Node-API ABI; no Electron/V8 headers or renderer access is required.
extern "C" {
typedef void* napi_env;
typedef void* napi_value;
typedef void* napi_callback_info;
typedef napi_value (*napi_callback)(napi_env, napi_callback_info);
typedef int napi_status;
napi_status napi_get_cb_info(napi_env, napi_callback_info, size_t*, napi_value*, napi_value*, void**);
napi_status napi_is_buffer(napi_env, napi_value, bool*);
napi_status napi_get_buffer_info(napi_env, napi_value, void**, size_t*);
napi_status napi_get_value_bool(napi_env, napi_value, bool*);
napi_status napi_get_boolean(napi_env, bool, napi_value*);
napi_status napi_get_undefined(napi_env, napi_value*);
napi_status napi_get_value_double(napi_env, napi_value, double*);
napi_status napi_get_value_string_utf8(napi_env, napi_value, char*, size_t, size_t*);
napi_status napi_create_function(napi_env, const char*, size_t, napi_callback, void*, napi_value*);
napi_status napi_set_named_property(napi_env, napi_value, const char*, napi_value);
napi_status napi_throw_error(napi_env, const char*, const char*);
}

namespace {
napi_value fail(napi_env env, const char* message) {
  napi_throw_error(env, nullptr, message);
  return nullptr;
}
napi_value done(napi_env env) {
  napi_value result = nullptr;
  napi_get_undefined(env, &result);
  return result;
}
bool args_for(napi_env env, napi_callback_info info, size_t expected, napi_value* args) {
  size_t count = expected;
  return napi_get_cb_info(env, info, &count, args, nullptr, nullptr) == 0 && count == expected;
}
NSWindow* window_for_view(napi_env env, napi_value value) {
  if (!pthread_main_np()) {
    fail(env, "AppKit window access must run on the main thread");
    return nil;
  }
  bool buffer = false;
  void* data = nullptr;
  size_t length = 0;
  if (napi_is_buffer(env, value, &buffer) != 0 || !buffer
      || napi_get_buffer_info(env, value, &data, &length) != 0 || length != sizeof(void*) || !data) {
    fail(env, "native window handle must be a pointer-sized Buffer");
    return nil;
  }
  void* view = nullptr;
  memcpy(&view, data, sizeof(view));
  // Compare addresses against owned objects; never dereference a supplied pointer.
  for (NSWindow* window in NSApp.windows) {
    if (view && (__bridge void*)window.contentView == view) return window;
  }
  fail(env, "native view does not belong to an AppKit window in this process");
  return nil;
}

napi_value configure_style(napi_env env, napi_callback_info info) {
  napi_value args[4];
  bool decorated = false, hidden = false;
  char style[16] = {};
  size_t length = 0;
  if (!args_for(env, info, 4, args)
      || napi_get_value_bool(env, args[1], &decorated) != 0
      || napi_get_value_string_utf8(env, args[2], style, sizeof(style), &length) != 0
      || length >= sizeof(style)
      || (strcmp(style, "visible") && strcmp(style, "transparent") && strcmp(style, "overlay"))
      || napi_get_value_bool(env, args[3], &hidden) != 0) {
    return fail(env, "invalid AppKit style arguments");
  }
  NSWindow* window = window_for_view(env, args[0]);
  if (!window) return nullptr;
  @try {
    NSButton* buttons[] = { [window standardWindowButton:NSWindowCloseButton],
      [window standardWindowButton:NSWindowMiniaturizeButton], [window standardWindowButton:NSWindowZoomButton] };
    bool present[3], enabled[3], concealed[3];
    for (int i = 0; i < 3; ++i) {
      present[i] = buttons[i] != nil;
      enabled[i] = buttons[i].enabled;
      concealed[i] = buttons[i].hidden;
    }
    NSRect frame = window.frame;
    NSWindowStyleMask mask = window.styleMask;
    if (decorated) mask |= NSWindowStyleMaskTitled;
    else mask &= ~NSWindowStyleMaskTitled;
    // Only Overlay extends content behind the title bar; Visible keeps the native caption inset.
    if (!strcmp(style, "overlay")) mask |= NSWindowStyleMaskFullSizeContentView;
    else mask &= ~NSWindowStyleMaskFullSizeContentView;
    window.styleMask = mask;
    window.titlebarAppearsTransparent = strcmp(style, "visible") != 0;
    window.titleVisibility = hidden ? NSWindowTitleHidden : NSWindowTitleVisible;
    [window setFrame:frame display:YES];
    for (int i = 0; i < 3; ++i) {
      NSButton* button = [window standardWindowButton:(NSWindowButton)i];
      if (present[i] && button) {
        button.enabled = enabled[i];
        button.hidden = concealed[i];
      }
    }
    [window.contentView layoutSubtreeIfNeeded];
    return done(env);
  } @catch (NSException* exception) { return fail(env, exception.reason.UTF8String); }
}

napi_value set_maximizable(napi_env env, napi_callback_info info) {
  napi_value args[2];
  bool maximizable = false;
  if (!args_for(env, info, 2, args) || napi_get_value_bool(env, args[1], &maximizable) != 0) {
    return fail(env, "setMaximizable expects a native window handle and boolean");
  }
  NSWindow* window = window_for_view(env, args[0]);
  if (!window) return nullptr;
  @try {
    NSButton* zoom = [window standardWindowButton:NSWindowZoomButton];
    if (!zoom) {
      if (!(window.styleMask & NSWindowStyleMaskTitled)) return done(env);
      return fail(env, "native zoom button is unavailable");
    }
    zoom.enabled = maximizable && (window.styleMask & NSWindowStyleMaskResizable) != 0;
    return done(env);
  } @catch (NSException* exception) { return fail(env, exception.reason.UTF8String); }
}

napi_value is_maximizable(napi_env env, napi_callback_info info) {
  napi_value args[1], result = nullptr;
  if (!args_for(env, info, 1, args)) return fail(env, "isMaximizable expects a native window handle");
  NSWindow* window = window_for_view(env, args[0]);
  if (!window) return nullptr;
  @try {
    NSButton* zoom = [window standardWindowButton:NSWindowZoomButton];
    if (!zoom) {
      if (!(window.styleMask & NSWindowStyleMaskTitled)) return napi_get_boolean(env, false, &result) == 0 ? result : fail(env, "cannot read native zoom button state");
      return fail(env, "native zoom button is unavailable");
    }
    if (napi_get_boolean(env, zoom.enabled, &result) != 0) return fail(env, "cannot read native zoom button state");
    return result;
  } @catch (NSException* exception) { return fail(env, exception.reason.UTF8String); }
}

napi_value set_traffic_light_position(napi_env env, napi_callback_info info) {
  napi_value args[3];
  double x = 0, y = 0;
  if (!args_for(env, info, 3, args)
      || napi_get_value_double(env, args[1], &x) != 0
      || napi_get_value_double(env, args[2], &y) != 0
      || !std::isfinite(x) || !std::isfinite(y)) {
    return fail(env, "setTrafficLightPosition expects a native handle and finite logical coordinates");
  }
  NSWindow* window = window_for_view(env, args[0]);
  if (!window) return nullptr;
  @try {
    NSButton* close = [window standardWindowButton:NSWindowCloseButton];
    if (!close || !close.superview.superview) return fail(env, "native titlebar controls are unavailable");
    DSHWindowButtonInset* layout = objc_getAssociatedObject(window, &buttonInsetKey);
    if (!layout) {
      layout = [DSHWindowButtonInset new];
      layout.window = window;
      objc_setAssociatedObject(window, &buttonInsetKey, layout, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
      for (NSNotificationName name in @[NSWindowDidResizeNotification, NSWindowDidUpdateNotification,
          NSWindowDidExitFullScreenNotification]) {
        [NSNotificationCenter.defaultCenter addObserver:layout selector:@selector(layoutChanged:) name:name object:window];
      }
    }
    layout.inset = NSMakePoint(x, y);
    [layout apply];
    return done(env);
  } @catch (NSException* exception) { return fail(env, exception.reason.UTF8String); }
}

napi_value is_decorated(napi_env env, napi_callback_info info) {
  napi_value args[1], result = nullptr;
  if (!args_for(env, info, 1, args)) return fail(env, "isDecorated expects a native window handle");
  NSWindow* window = window_for_view(env, args[0]);
  if (!window) return nullptr;
  if (napi_get_boolean(env, (window.styleMask & NSWindowStyleMaskTitled) != 0, &result) != 0) {
    return fail(env, "cannot read AppKit decorations");
  }
  return result;
}

} // namespace

extern "C" __attribute__((visibility("default"))) napi_value napi_register_module_v1(napi_env env, napi_value exports) {
  const char* names[] = { "configureStyle", "isDecorated", "setMaximizable", "isMaximizable", "setTrafficLightPosition" };
  napi_callback callbacks[] = { configure_style, is_decorated, set_maximizable, is_maximizable, set_traffic_light_position };
  for (int i = 0; i < 5; ++i) {
    napi_value function = nullptr;
    if (napi_create_function(env, names[i], static_cast<size_t>(-1), callbacks[i], nullptr, &function) != 0
        || napi_set_named_property(env, exports, names[i], function) != 0) return nullptr;
  }
  return exports;
}
