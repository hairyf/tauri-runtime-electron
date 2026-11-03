use std::sync::Once;

use objc2::{rc::autoreleasepool, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

pub(crate) fn init() {
  let Some(mtm) = MainThreadMarker::new() else {
    assert!(cfg!(test), "macOS native UI must run on the main thread");
    return;
  };
  static INITIALIZED: Once = Once::new();
  INITIALIZED.call_once(|| {
    autoreleasepool(|_| {
      let app = NSApplication::sharedApplication(mtm);
      // A bundled host that declares `LSUIElement` is already an accessory app.
      // AppKit (notably on macOS 26) reports `false` for a redundant policy change,
      // so assert on the resulting policy rather than on the return value.
      let changed = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
      assert!(
        changed || app.activationPolicy() == NSApplicationActivationPolicy::Accessory,
        "macOS host must remain an accessory application"
      );
      app.finishLaunching();
    });
  });
}

pub(crate) fn pump() {
  let Some(mtm) = MainThreadMarker::new() else {
    assert!(cfg!(test), "macOS native UI must run on the main thread");
    return;
  };
  autoreleasepool(|_| {
    let app = NSApplication::sharedApplication(mtm);
    let deadline = NSDate::distantPast();
    while let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
      NSEventMask::Any,
      Some(&deadline),
      unsafe { NSDefaultRunLoopMode },
      true,
    ) {
      app.sendEvent(&event);
    }
    app.updateWindows();
  });
}
