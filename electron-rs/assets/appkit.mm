#import <AppKit/AppKit.h>
#include <pthread.h>
#include <stdint.h>
#include <string.h>
#include <cmath>

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
    bool enabled[3], concealed[3];
    for (int i = 0; i < 3; ++i) {
      enabled[i] = buttons[i].enabled;
      concealed[i] = buttons[i].hidden;
    }
    NSRect frame = window.frame;
    NSWindowStyleMask mask = window.styleMask;
    if (decorated) mask |= NSWindowStyleMaskTitled;
    else mask &= ~NSWindowStyleMaskTitled;
    // Match Tao: Visible and Overlay use full-size content, Transparent does not.
    if (strcmp(style, "transparent")) mask |= NSWindowStyleMaskFullSizeContentView;
    else mask &= ~NSWindowStyleMaskFullSizeContentView;
    window.styleMask = mask;
    window.titlebarAppearsTransparent = strcmp(style, "visible") != 0;
    window.titleVisibility = hidden ? NSWindowTitleHidden : NSWindowTitleVisible;
    [window setFrame:frame display:YES];
    for (int i = 0; i < 3; ++i) {
      NSButton* button = [window standardWindowButton:(NSWindowButton)i];
      button.enabled = enabled[i];
      button.hidden = concealed[i];
    }
    [window.contentView layoutSubtreeIfNeeded];
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

napi_value traffic_lights(napi_env env, napi_callback_info info) {
  napi_value args[3];
  double x = 0, y = 0;
  if (!args_for(env, info, 3, args) || napi_get_value_double(env, args[1], &x) != 0
      || napi_get_value_double(env, args[2], &y) != 0 || !std::isfinite(x) || !std::isfinite(y)) {
    return fail(env, "invalid traffic light position");
  }
  NSWindow* window = window_for_view(env, args[0]);
  if (!window) return nullptr;
  @try {
    NSButton* close = [window standardWindowButton:NSWindowCloseButton];
    NSButton* mini = [window standardWindowButton:NSWindowMiniaturizeButton];
    NSButton* zoom = [window standardWindowButton:NSWindowZoomButton];
    NSView* container = close.superview.superview;
    if (!close || !mini || !zoom || !container) return fail(env, "native window controls are unavailable");
    // Same window-relative inset as Tao, preserving the native button spacing.
    NSRect frame = container.frame;
    frame.size.height = close.frame.size.height + y;
    frame.origin.y = window.frame.size.height - frame.size.height;
    [container setFrame:frame];
    double spacing = mini.frame.origin.x - close.frame.origin.x;
    NSButton* buttons[] = { close, mini, zoom };
    for (int i = 0; i < 3; ++i) {
      NSPoint origin = buttons[i].frame.origin;
      origin.x = x + i * spacing;
      [buttons[i] setFrameOrigin:origin];
    }
    return done(env);
  } @catch (NSException* exception) { return fail(env, exception.reason.UTF8String); }
}
} // namespace

extern "C" __attribute__((visibility("default"))) napi_value napi_register_module_v1(napi_env env, napi_value exports) {
  const char* names[] = { "configureStyle", "isDecorated", "trafficLights" };
  napi_callback callbacks[] = { configure_style, is_decorated, traffic_lights };
  for (int i = 0; i < 3; ++i) {
    napi_value function = nullptr;
    if (napi_create_function(env, names[i], static_cast<size_t>(-1), callbacks[i], nullptr, &function) != 0
        || napi_set_named_property(env, exports, names[i], function) != 0) return nullptr;
  }
  return exports;
}
