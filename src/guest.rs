use std::{collections::HashMap, sync::{Mutex, OnceLock}};

fn hosts() -> &'static Mutex<HashMap<String, String>> {
  static HOSTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
  HOSTS.get_or_init(Default::default)
}

pub fn with_guest<R>(host_label: &str, guest_label: &str, build: impl FnOnce() -> R) -> R {
  struct Restore(String, Option<String>);
  impl Drop for Restore {
    fn drop(&mut self) {
      let mut hosts = hosts().lock().unwrap();
      match self.1.take() {
        Some(host) => { hosts.insert(self.0.clone(), host); }
        None => { hosts.remove(&self.0); }
      }
    }
  }
  let previous = hosts().lock().unwrap().insert(guest_label.to_owned(), host_label.to_owned());
  let _restore = Restore(guest_label.to_owned(), previous);
  build()
}

pub(crate) fn take_host(guest_label: &str) -> Option<String> {
  hosts().lock().unwrap().remove(guest_label)
}

#[test]
fn guest_registration_crosses_threads_and_cleans_up() {
  with_guest("main", "guest", || {
    std::thread::spawn(|| {
      assert_eq!(take_host("guest").as_deref(), Some("main"));
      assert_eq!(take_host("guest"), None);
    }).join().unwrap();
  });
  with_guest("main", "unused", || {});
  assert_eq!(take_host("unused"), None);
}
