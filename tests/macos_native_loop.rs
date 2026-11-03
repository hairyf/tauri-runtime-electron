#[cfg(target_os = "macos")]
#[path = "../src/platform/macos_loop.rs"]
mod macos_loop;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/macos_menu.rs"]
mod macos_menu;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/rpc_runtime.rs"]
mod rpc_runtime;

#[cfg(target_os = "macos")]
fn main() {
  use std::sync::atomic::{AtomicUsize, Ordering};

  use objc2::{
    define_class, msg_send,
    rc::{autoreleasepool, Retained},
    sel, ClassType, MainThreadMarker,
  };
  use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventModifierFlags, NSEventType,
    NSMenu, NSMenuItem,
  };
  use objc2_foundation::{NSPoint, NSString};
  use serde_json::json;

  static LAUNCHES: AtomicUsize = AtomicUsize::new(0);
  static DISPATCHED: AtomicUsize = AtomicUsize::new(0);
  static EVENTS: AtomicUsize = AtomicUsize::new(0);
  static APPLICATION_EVENTS: AtomicUsize = AtomicUsize::new(0);
  static MENU_COMMANDS: AtomicUsize = AtomicUsize::new(0);

  define_class!(
    #[unsafe(super(NSApplication))]
    struct ProbeApplication;

    impl ProbeApplication {
      #[unsafe(method(probeMenuCommand:))]
      fn menu_command(&self, _sender: &NSMenuItem) {
        MENU_COMMANDS.fetch_add(1, Ordering::Relaxed);
      }

      #[unsafe(method(finishLaunching))]
      fn finish_launching(&self) {
        LAUNCHES.fetch_add(1, Ordering::Relaxed);
        unsafe { msg_send![super(self), finishLaunching] }
      }

      #[unsafe(method(sendEvent:))]
      fn send_event(&self, event: &NSEvent) {
        EVENTS.fetch_add(1, Ordering::Relaxed);
        if event.r#type() == NSEventType::ApplicationDefined { APPLICATION_EVENTS.fetch_add(1, Ordering::Relaxed); }
        if event.r#type() == NSEventType::ApplicationDefined && event.subtype().0 == 1331 {
          DISPATCHED.fetch_add(event.data1() as usize, Ordering::Relaxed);
        } else {
          unsafe { msg_send![super(self), sendEvent: event] }
        }
      }
    }
  );

  let mtm = MainThreadMarker::new().expect("native loop test must start on the main thread");
  autoreleasepool(|_| {
    let app: Retained<NSApplication> =
      unsafe { msg_send![ProbeApplication::class(), sharedApplication] };
    // Mirrors a bundled host that declares `LSUIElement`: AppKit already reports the
    // accessory policy, so initialization must tolerate a refused redundant change.
    assert!(
      app.setActivationPolicy(NSApplicationActivationPolicy::Accessory)
        || app.activationPolicy() == NSApplicationActivationPolicy::Accessory,
      "probe app could not enter the accessory policy"
    );
    macos_loop::init();
    macos_loop::init();
    std::thread::spawn(|| {
      macos_loop::init();
      macos_loop::pump();
    })
    .join()
    .expect("IPC-only workers must not enter AppKit");
    assert_eq!(
      LAUNCHES.load(Ordering::Relaxed),
      1,
      "host initialization must finish launching exactly once"
    );
    assert_eq!(
      app.activationPolicy(),
      NSApplicationActivationPolicy::Accessory,
      "host must not create a second Dock entry"
    );

    for value in 1..=3 {
      let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::ZERO,
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        1331,
        value,
        0,
      ).expect("create native probe event");
      assert_eq!(event.r#type(), NSEventType::ApplicationDefined);
      assert_eq!(event.subtype().0, 1331);
      assert_eq!(event.data1(), value);
      app.postEvent_atStart(&event, false);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while DISPATCHED.load(Ordering::Relaxed) != 6 && std::time::Instant::now() < deadline {
      macos_loop::pump();
      std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
      DISPATCHED.load(Ordering::Relaxed),
      6,
      "runtime pumps must dispatch posted events: received {} events, {} application events",
      EVENTS.load(Ordering::Relaxed),
      APPLICATION_EVENTS.load(Ordering::Relaxed)
    );
    macos_loop::pump();
    assert_eq!(
      DISPATCHED.load(Ordering::Relaxed),
      6,
      "empty native queue must return without replaying events"
    );
    let root = NSMenu::new(mtm);
    let submenu = NSMenu::new(mtm);
    root.setAutoenablesItems(false);
    submenu.setAutoenablesItems(false);
    let parent = NSMenuItem::new(mtm);
    parent.setTitle(&NSString::from_str("Application"));
    parent.setSubmenu(Some(&submenu));
    root.addItem(&parent);
    let settings = NSMenuItem::new(mtm);
    settings.setTitle(&NSString::from_str("Settings"));
    settings.setKeyEquivalent(&NSString::from_str(","));
    settings.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
    unsafe {
      settings.setAction(Some(sel!(probeMenuCommand:)));
      settings.setTarget(Some(&app));
    }
    submenu.addItem(&settings);
    let previous_menu = app.mainMenu();
    app.setMainMenu(Some(&root));
    let mut commands = Vec::new();
    assert_eq!(
      macos_menu::snapshot(&root, &mut commands),
      json!([{"label": "Application", "enabled": true, "submenu": [
        {"label": "Settings", "enabled": true, "accelerator": "Super+,", "commandId": 0}
      ]}]),
      "native settings shortcut and callback identifier must reach the menu snapshot"
    );
    assert_eq!(commands, vec![settings.clone()]);
    settings.setKeyEquivalent(&NSString::from_str("\u{f708}"));
    settings.setKeyEquivalentModifierMask(NSEventModifierFlags::empty());
    commands.clear();
    assert_eq!(
      macos_menu::snapshot(&root, &mut commands)[0]["submenu"][0]["accelerator"],
      json!("F5")
    );
    settings.setKeyEquivalent(&NSString::from_str("\u{f703}"));
    settings.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
    commands.clear();
    assert_eq!(
      macos_menu::snapshot(&root, &mut commands)[0]["submenu"][0]["accelerator"],
      json!("Super+Right")
    );
    parent.setEnabled(false);
    macos_menu::dispatch(&settings);
    assert_eq!(
      MENU_COMMANDS.load(Ordering::Relaxed),
      0,
      "disabled ancestors must block menu callbacks"
    );
    parent.setEnabled(true);
    macos_menu::dispatch(&settings);
    assert_eq!(
      MENU_COMMANDS.load(Ordering::Relaxed),
      1,
      "enabled native menu command must invoke its actual selector once"
    );
    submenu.removeItem(&settings);
    macos_menu::dispatch(&settings);
    assert_eq!(
      MENU_COMMANDS.load(Ordering::Relaxed),
      1,
      "removed native menu items must not dispatch stale commands"
    );
    app.setMainMenu(previous_menu.as_deref());
  });
  println!("macos_native_loop: accessory initialization, native events, and menu dispatch passed");
}

#[cfg(not(target_os = "macos"))]
fn main() {
  println!("macos_native_loop: requires macOS; native assertions were not run");
}
