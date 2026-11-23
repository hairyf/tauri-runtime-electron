// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Detecting a launch on a profile last written by a newer Chromium, and what to do then.
//!
//! Chromium migrates its profile forward only. `Local State`, each profile's `Preferences`
//! and the SQLite and LevelDB stores behind cookies, local storage and IndexedDB are
//! written in the format of the version that runs, and an older version opening them gets
//! whatever its readers make of them: a preference it never heard of is dropped on the next
//! write, a database whose schema is newer than it knows is left unused or discarded, and
//! everything in between is undefined. Chrome calls a launch on a profile from a higher
//! milestone an unsupported downgrade (`chrome/browser/downgrade/`): it keeps a `Last
//! Version` breadcrumb in its user data directory to recognize one, runs on the newer
//! profile anyway when nobody administered the downgrade, and where an administrator did it
//! moves the directory aside and starts over. Electron inherits Chromium's profile format
//! but not that manager, so an application whose Electron release is rolled back would not
//! even know.
//!
//! This module is the missing piece. [`prepare_root_cache_path`] records the Chromium
//! milestone of the Electron release in the root cache path on every launch and, when the
//! recorded version belongs to a higher milestone, applies the application's
//! [`DowngradePolicy`]: by default the profile is kept and a warning logged, as Chrome
//! does, and [`DowngradePolicy::ResetProfile`] moves the whole directory aside instead so
//! Chromium starts on an empty one, deleting the old one in the background. A downgrade
//! within a milestone is left alone either way, as Chrome leaves it
//! (`DowngradeManager::kMinorDowngrade`).
//!
//! It has to run before the Electron child is spawned. The directory this module prepares
//! is the one the child opens as its user-data directory, and once that child is running
//! the directory holds Chromium's process singleton, its open files and its lock — the
//! rename a reset needs is only possible in the window between the runtime deciding to
//! launch and the child existing.
//!
//! A reset never runs under another instance of the application. On Unix, Chromium's
//! process singleton lock in the directory names the process holding it — Electron uses the
//! same `ProcessSingleton` CEF does — and a live holder leaves the directory to Chromium,
//! whose singleton then hands this launch over to that instance. On Windows the rename
//! itself fails while another process has files open inside the directory, which comes to
//! the same thing.

use std::{
  fmt, fs, io,
  path::{Path, PathBuf},
  time::{SystemTime, UNIX_EPOCH},
};

use crate::DowngradePolicy;

/// The file in the root cache path holding the Chromium version that last ran on it.
///
/// Chrome's downgrade manager keeps a file of this name, format and meaning in its user
/// data directory (`chrome/browser/downgrade/user_data_downgrade.cc`); Electron does not
/// write it, so this runtime does, and the two would agree if Electron ever started to.
/// The name and the format are kept byte-identical to the reference's so a profile migrated
/// from the CEF runtime is recognised rather than treated as one of unknown origin.
pub(crate) const LAST_VERSION_FILE: &str = "Last Version";

/// The suffix of a root cache path a reset moved aside, to be deleted in the background.
const MOVED_ASIDE_SUFFIX: &str = ".tauri-delete";

/// Chromium's process singleton lock on Unix: a symlink whose target is `<hostname>-<pid>`
/// (`chrome/browser/process_singleton_posix.cc`).
#[cfg(unix)]
const SINGLETON_LOCK_FILE: &str = "SingletonLock";

/// A Chromium version, `MAJOR.MINOR.BUILD.PATCH`. The major component is the milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ChromiumVersion {
  pub major: u32,
  pub minor: u32,
  pub build: u32,
  pub patch: u32,
}

impl ChromiumVersion {
  /// The Chromium milestone of the Electron release this runtime launches.
  ///
  /// CEF publishes its Chromium version as compile-time constants (`CHROME_VERSION_*`), and
  /// the library a running application loaded can never disagree with them because
  /// `cef::initialize` refuses one whose API hash differs. Electron publishes nothing of
  /// the sort: the Chromium milestone is a property of the Electron release, and the
  /// release this runtime launches is the one the bridge pins in
  /// [`electron_rs::DEFAULT_VERSION`]. Between Electron 20 and Electron 44 each Electron
  /// major carries the next two Chromium milestones, so the milestone is `2 * electron + 64`
  /// — Electron 44 ships Chromium 152, Electron 39 ships 142, Electron 20 ships 104
  /// (<https://releases.electronjs.org/schedule>). Electron 45 moved to Chromium 156, which
  /// that relation does not predict, so nothing is extrapolated: a release outside the range
  /// answers [`None`] and [`prepare_root_cache_path`] refuses to guess.
  ///
  /// Only the milestone is known this early. `process.versions.chrome`, which names the
  /// build and patch as well, arrives with the handshake — long after the breadcrumb has to
  /// be written — so the breadcrumb records `MAJOR.0.0.0`. That is enough for what the file
  /// is for: Chrome's downgrade manager compares milestones, and a downgrade *within* a
  /// milestone is one Chrome supports (`DowngradeManager::kMinorDowngrade`).
  pub(crate) fn embedded() -> Option<Self> {
    let electron = electron_rs::DEFAULT_VERSION
      .split('.')
      .next()?
      .parse::<u32>()
      .ok()?;
    Some(Self {
      major: chromium_milestone(electron)?,
      minor: 0,
      build: 0,
      patch: 0,
    })
  }

  /// Parses `MAJOR[.MINOR[.BUILD[.PATCH]]]`, with missing components read as zero.
  pub(crate) fn parse(text: &str) -> Option<Self> {
    let mut components = text.trim().split('.').map(|c| c.parse::<u32>().ok());
    let major = components.next()??;
    let mut rest = [0u32; 3];
    for slot in &mut rest {
      match components.next() {
        Some(component) => *slot = component?,
        None => break,
      }
    }
    if components.next().is_some() {
      return None;
    }
    Some(Self {
      major,
      minor: rest[0],
      build: rest[1],
      patch: rest[2],
    })
  }
}

impl fmt::Display for ChromiumVersion {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "{}.{}.{}.{}",
      self.major, self.minor, self.build, self.patch
    )
  }
}

/// The Chromium milestone an Electron major carries, for the releases this runtime knows.
///
/// See [`ChromiumVersion::embedded`] for why the relation is a formula over a closed range
/// rather than a table of pinned pairs, and why a release outside the range is not guessed.
fn chromium_milestone(electron_major: u32) -> Option<u32> {
  (20..=44)
    .contains(&electron_major)
    .then(|| 2 * electron_major + 64)
}

/// What [`prepare_root_cache_path`] found on disk and did about it.
#[derive(Debug)]
pub(crate) enum Outcome {
  /// No breadcrumb: an empty directory, or a profile from before the runtime kept one.
  Unknown,
  /// The same version or an older one last ran here: the profile is usable as is.
  Kept,
  /// A later build of the same milestone last ran here, which Chrome supports.
  MinorDowngrade { last: ChromiumVersion },
  /// A higher milestone last ran here and [`DowngradePolicy::KeepProfile`] kept it.
  KeptNewer { last: ChromiumVersion },
  /// A higher milestone last ran here; the directory was moved aside and recreated.
  Reset { last: ChromiumVersion },
  /// A higher milestone last ran here, but another instance holds the profile.
  InUse {
    last: ChromiumVersion,
    holder: String,
  },
  /// A higher milestone last ran here and moving the directory aside failed.
  ResetFailed {
    last: ChromiumVersion,
    error: io::Error,
  },
}

/// Prepares the root cache path for the Chromium Electron ships, before the Electron child
/// is spawned on it.
///
/// Must run after the directory exists and before the child is spawned, which is the first
/// thing to open anything inside it.
///
/// Answers `Err` only when the profile cannot be classified at all: the Chromium milestone
/// of the Electron release is unknown, so no breadcrumb can be written and a rollback would
/// go undetected from then on. Every outcome the reference logs and continues from stays a
/// log line — Chromium opens the profile either way — but a build that cannot name its own
/// Chromium milestone has to be fixed rather than papered over, because recording a wrong
/// version silently disables the detection this module exists for.
pub(crate) fn prepare_root_cache_path(root: &Path, policy: DowngradePolicy) -> Result<(), String> {
  let Some(current) = ChromiumVersion::embedded() else {
    return Err(format!(
      "cannot prepare the Electron profile at {}: the Chromium milestone of Electron {} is \
       unknown, so a rollback to an older Electron could not be detected. Add the release to \
       `chromium_milestone` in src/downgrade.rs.",
      root.display(),
      electron_rs::DEFAULT_VERSION
    ));
  };

  prepare(root, current, policy);
  Ok(())
}

fn prepare(root: &Path, current: ChromiumVersion, policy: DowngradePolicy) -> Outcome {
  // A reset the previous launch started and did not finish deleting.
  delete_in_background(moved_aside_siblings(root));

  let outcome = match read_last_version(root) {
    None => Outcome::Unknown,
    Some(last) if last.major < current.major || last <= current => Outcome::Kept,
    Some(last) if last.major == current.major => Outcome::MinorDowngrade { last },
    Some(last) => match policy {
      DowngradePolicy::KeepProfile => Outcome::KeptNewer { last },
      DowngradePolicy::ResetProfile => match singleton_holder(root) {
        Some(holder) => Outcome::InUse { last, holder },
        None => match move_aside(root) {
          Ok(moved) => {
            let _ = fs::create_dir_all(root);
            delete_in_background(vec![moved]);
            Outcome::Reset { last }
          }
          Err(error) => Outcome::ResetFailed { last, error },
        },
      },
    },
  };
  report(&outcome, current);

  // A reset that did not happen stays pending: the breadcrumb keeps naming the newer
  // version so the next launch tries again, and an instance that does hold the profile
  // is not told it ran an older Chromium than it did.
  if !matches!(outcome, Outcome::InUse { .. } | Outcome::ResetFailed { .. })
    && let Err(error) = write_last_version(root, current)
  {
    log::warn!(
      "failed to record the Chromium version in {}: {error}. A rollback to an older Electron will not be detected on the next launch.",
      root.join(LAST_VERSION_FILE).display()
    );
  }
  outcome
}

fn report(outcome: &Outcome, current: ChromiumVersion) {
  match outcome {
    Outcome::Unknown | Outcome::Kept => {}
    Outcome::MinorDowngrade { last } => log::info!(
      "the Electron profile was last used by Chromium {last}, a later build of the milestone this Chromium {current} belongs to; keeping it, as Chrome does"
    ),
    Outcome::KeptNewer { last } => log::warn!(
      "the Electron profile was last used by Chromium {last}, a newer milestone than this Chromium {current}; keeping it, as Chrome does. Chromium never migrates a profile backwards, so a store whose format changed in between may go unused or be discarded. Set DowngradePolicy::ResetProfile to start on an empty profile instead."
    ),
    Outcome::Reset { last } => log::warn!(
      "the Electron profile was last used by Chromium {last}, a newer milestone than this Chromium {current}; it was moved aside and Chromium starts on an empty one, as DowngradePolicy::ResetProfile asks. Cookies, local storage, IndexedDB and granted permissions are gone."
    ),
    Outcome::InUse { last, holder } => log::warn!(
      "the Electron profile was last used by Chromium {last}, a newer milestone than this Chromium {current}, but {holder} holds it, so it is left in place"
    ),
    Outcome::ResetFailed { last, error } => log::warn!(
      "the Electron profile was last used by Chromium {last}, a newer milestone than this Chromium {current}, and moving it aside failed: {error}. Chromium opens it anyway; the reset is retried on the next launch."
    ),
  }
}

fn read_last_version(root: &Path) -> Option<ChromiumVersion> {
  fs::read_to_string(root.join(LAST_VERSION_FILE))
    .ok()
    .and_then(|text| ChromiumVersion::parse(&text))
}

fn write_last_version(root: &Path, version: ChromiumVersion) -> io::Result<()> {
  fs::write(root.join(LAST_VERSION_FILE), version.to_string())
}

/// Renames `root` to a sibling `<name>.<millis>.tauri-delete`, staying on the same volume.
fn move_aside(root: &Path) -> io::Result<PathBuf> {
  let (Some(parent), Some(name)) = (root.parent(), root.file_name()) else {
    return Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      "the root cache path has no parent directory to move it within",
    ));
  };
  let mut stamp = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|elapsed| elapsed.as_millis())
    .unwrap_or_default();
  loop {
    let mut target_name = name.to_os_string();
    target_name.push(format!(".{stamp}{MOVED_ASIDE_SUFFIX}"));
    let target = parent.join(target_name);
    if !target.exists() {
      fs::rename(root, &target)?;
      return Ok(target);
    }
    stamp += 1;
  }
}

/// The directories next to `root` that earlier resets moved aside and did not finish
/// deleting.
fn moved_aside_siblings(root: &Path) -> Vec<PathBuf> {
  let (Some(parent), Some(name)) = (root.parent(), root.file_name().and_then(|n| n.to_str()))
  else {
    return Vec::new();
  };
  let prefix = format!("{name}.");
  let Ok(entries) = fs::read_dir(parent) else {
    return Vec::new();
  };
  entries
    .flatten()
    .filter(|entry| {
      let file_name = entry.file_name();
      let Some(file_name) = file_name.to_str() else {
        return false;
      };
      file_name.starts_with(&prefix)
        && file_name.ends_with(MOVED_ASIDE_SUFFIX)
        && entry.path().is_dir()
    })
    .map(|entry| entry.path())
    .collect()
}

fn delete_in_background(paths: Vec<PathBuf>) {
  if paths.is_empty() {
    return;
  }
  let spawned = std::thread::Builder::new()
    .name("tauri-electron-profile-cleanup".into())
    .spawn(move || delete_moved_aside(&paths));
  if let Err(error) = spawned {
    log::warn!("failed to spawn the Electron profile cleanup thread: {error}");
  }
}

fn delete_moved_aside(paths: &[PathBuf]) {
  for path in paths {
    if let Err(error) = fs::remove_dir_all(path) {
      log::warn!(
        "failed to delete the Electron profile moved aside at {}: {error}",
        path.display()
      );
    }
  }
}

/// Who holds Chromium's process singleton lock in `root`, if anyone.
///
/// Follows `ProcessSingleton` on POSIX, which Electron uses unchanged: the lock is a symlink
/// to `<hostname>-<pid>`, and it is stale when it names this host and a process that no
/// longer exists. A lock from another host cannot be checked, so it counts as held, which is
/// also how Chromium treats it.
#[cfg(unix)]
fn singleton_holder(root: &Path) -> Option<String> {
  let target = fs::read_link(root.join(SINGLETON_LOCK_FILE)).ok()?;
  let target = target.to_string_lossy();
  let (host, pid) = target.rsplit_once('-')?;
  let pid: i32 = pid.parse().ok()?;
  if hostname().as_deref() != Some(host) {
    return Some(format!("process {pid} on {host}"));
  }
  // SAFETY: a null signal performs error checking only and never delivers anything.
  let alive = unsafe { posix::kill(pid, 0) } == 0
    || io::Error::last_os_error().raw_os_error() == Some(posix::EPERM);
  alive.then(|| format!("process {pid}"))
}

/// Windows has no lock to read: `ProcessSingleton` there is a named window plus a file
/// opened without delete sharing, and renaming a directory with such a file open inside
/// fails, which `move_aside` reports.
#[cfg(not(unix))]
fn singleton_holder(_root: &Path) -> Option<String> {
  None
}

/// The host name as `gethostname` reports it, which is what Chromium writes into the lock.
#[cfg(unix)]
fn hostname() -> Option<String> {
  let mut buffer = [0u8; 256];
  // SAFETY: `gethostname` writes at most `buffer.len()` bytes into `buffer`, and the buffer
  // outlives the call.
  let status = unsafe { posix::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
  if status != 0 {
    return None;
  }
  let end = buffer
    .iter()
    .position(|&byte| byte == 0)
    .unwrap_or(buffer.len());
  Some(String::from_utf8_lossy(&buffer[..end]).into_owned())
}

/// The two POSIX calls the singleton probe needs.
///
/// This crate does not depend on `libc`, and these are the only two symbols it would use, so
/// they are declared where they are used rather than pulling in a binding crate for them.
#[cfg(unix)]
mod posix {
  use std::ffi::{c_char, c_int};

  unsafe extern "C" {
    /// Sends `signal` to `pid`; signal `0` only checks that the process exists.
    pub(super) fn kill(pid: c_int, signal: c_int) -> c_int;
    /// Writes this host's name into `name`, up to `len` bytes.
    pub(super) fn gethostname(name: *mut c_char, len: usize) -> c_int;
  }

  /// `EPERM`, which is 1 on every Unix this runtime targets: Linux, macOS, DragonFly and
  /// the BSDs all define it as 1.
  pub(super) const EPERM: c_int = 1;
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::{AtomicUsize, Ordering};

  fn v(text: &str) -> ChromiumVersion {
    ChromiumVersion::parse(text).unwrap()
  }

  /// A fresh `<temp>/<unique>/electron` root, so the parent directory is ours to inspect.
  fn temp_root() -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
      "tauri-electron-downgrade-{}-{}",
      std::process::id(),
      COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    let root = dir.join("electron");
    fs::create_dir_all(&root).unwrap();
    root
  }

  fn stamp(root: &Path, version: &str) {
    write_last_version(root, v(version)).unwrap();
  }

  fn recorded(root: &Path) -> Option<String> {
    read_last_version(root).map(|version| version.to_string())
  }

  fn with_site_data(root: &Path) -> PathBuf {
    let cookies = root.join("Default").join("Cookies");
    fs::create_dir_all(cookies.parent().unwrap()).unwrap();
    fs::write(&cookies, b"jar").unwrap();
    cookies
  }

  #[test]
  fn parses_and_orders_chromium_versions() {
    assert_eq!(v("152.0.7977.83").to_string(), "152.0.7977.83");
    assert_eq!(v(" 151.0.7922.174\n").to_string(), "151.0.7922.174");
    assert_eq!(
      v("152").to_string(),
      "152.0.0.0",
      "missing components are zero"
    );
    assert!(v("152.0.7977.83") > v("151.0.7922.174"));
    assert!(v("152.0.7977.83") > v("152.0.7977.50"));
    assert!(v("152.1.0.0") > v("152.0.9999.9999"));
    for invalid in ["", "abc", "152.x", "1.2.3.4.5", "-1"] {
      assert!(ChromiumVersion::parse(invalid).is_none(), "{invalid:?}");
    }
  }

  #[test]
  fn the_embedded_version_is_the_pinned_electrons_milestone() {
    let electron = electron_rs::DEFAULT_VERSION
      .split('.')
      .next()
      .unwrap()
      .parse::<u32>()
      .unwrap();
    let embedded = ChromiumVersion::embedded().expect("the pinned Electron release is mapped");
    assert_eq!(embedded.major, chromium_milestone(electron).unwrap());
    assert_eq!(
      embedded.to_string(),
      format!("{}.0.0.0", embedded.major),
      "only the milestone is known before the child starts"
    );
  }

  #[test]
  fn the_milestone_relation_covers_the_supported_range_and_stops_there() {
    assert_eq!(chromium_milestone(20), Some(104));
    assert_eq!(chromium_milestone(39), Some(142));
    assert_eq!(chromium_milestone(44), Some(152));
    assert_eq!(
      chromium_milestone(45),
      None,
      "Electron 45 ships Chromium 156, which the relation does not predict"
    );
    assert_eq!(chromium_milestone(19), None);
  }

  #[test]
  fn preparing_the_root_cache_path_records_the_embedded_milestone() {
    let root = temp_root();
    prepare_root_cache_path(&root, DowngradePolicy::KeepProfile).unwrap();
    assert_eq!(
      recorded(&root),
      ChromiumVersion::embedded().map(|version| version.to_string()),
      "an unknown Chromium version would be a startup error, not a silent skip"
    );
  }

  #[test]
  fn a_profile_without_a_breadcrumb_only_gets_one() {
    let root = temp_root();
    let cookies = with_site_data(&root);
    let outcome = prepare(&root, v("152.0.7977.83"), DowngradePolicy::ResetProfile);
    assert!(matches!(outcome, Outcome::Unknown), "{outcome:?}");
    assert!(
      cookies.exists(),
      "a profile of unknown origin is never reset"
    );
    assert_eq!(recorded(&root).as_deref(), Some("152.0.7977.83"));
  }

  #[test]
  fn an_unreadable_breadcrumb_counts_as_none() {
    let root = temp_root();
    fs::write(root.join(LAST_VERSION_FILE), "not a version").unwrap();
    let outcome = prepare(&root, v("151.0.7922.174"), DowngradePolicy::ResetProfile);
    assert!(matches!(outcome, Outcome::Unknown), "{outcome:?}");
    assert_eq!(recorded(&root).as_deref(), Some("151.0.7922.174"));
  }

  #[test]
  fn an_upgrade_keeps_the_profile_and_moves_the_breadcrumb_forward() {
    let root = temp_root();
    stamp(&root, "151.0.7922.174");
    let cookies = with_site_data(&root);
    let outcome = prepare(&root, v("152.0.7977.83"), DowngradePolicy::ResetProfile);
    assert!(matches!(outcome, Outcome::Kept), "{outcome:?}");
    assert!(cookies.exists());
    assert_eq!(recorded(&root).as_deref(), Some("152.0.7977.83"));
  }

  #[test]
  fn the_same_version_keeps_the_profile() {
    let root = temp_root();
    stamp(&root, "152.0.7977.83");
    let cookies = with_site_data(&root);
    let outcome = prepare(&root, v("152.0.7977.83"), DowngradePolicy::ResetProfile);
    assert!(matches!(outcome, Outcome::Kept), "{outcome:?}");
    assert!(cookies.exists());
  }

  #[test]
  fn a_downgrade_within_the_milestone_keeps_the_profile() {
    // Chrome supports this one (`kMinorDowngrade`), so the runtime does not second-guess it.
    let root = temp_root();
    stamp(&root, "152.0.7977.90");
    let cookies = with_site_data(&root);
    let outcome = prepare(&root, v("152.0.7977.83"), DowngradePolicy::ResetProfile);
    assert!(
      matches!(outcome, Outcome::MinorDowngrade { .. }),
      "{outcome:?}"
    );
    assert!(cookies.exists());
    assert_eq!(
      recorded(&root).as_deref(),
      Some("152.0.7977.83"),
      "the breadcrumb always names the version that ran last"
    );
  }

  #[test]
  fn keep_profile_keeps_a_newer_milestones_profile() {
    let root = temp_root();
    stamp(&root, "152.0.7977.83");
    let cookies = with_site_data(&root);
    let outcome = prepare(&root, v("151.0.7922.174"), DowngradePolicy::KeepProfile);
    assert!(matches!(outcome, Outcome::KeptNewer { .. }), "{outcome:?}");
    assert!(cookies.exists());
    assert_eq!(
      recorded(&root).as_deref(),
      Some("151.0.7922.174"),
      "the older Chromium is now the last one to have run on the profile"
    );
  }

  #[test]
  fn reset_profile_moves_a_newer_milestones_profile_aside() {
    let root = temp_root();
    stamp(&root, "152.0.7977.83");
    let cookies = with_site_data(&root);
    let outcome = prepare(&root, v("151.0.7922.174"), DowngradePolicy::ResetProfile);
    assert!(matches!(outcome, Outcome::Reset { .. }), "{outcome:?}");
    assert!(
      root.is_dir(),
      "the root is recreated for Chromium to start on"
    );
    assert!(
      !cookies.exists(),
      "nothing of the newer profile is left in it"
    );
    assert_eq!(recorded(&root).as_deref(), Some("151.0.7922.174"));
  }

  #[test]
  fn moving_aside_stays_next_to_the_root_and_is_found_again() {
    let root = temp_root();
    let cookies = with_site_data(&root);
    let moved = move_aside(&root).unwrap();
    assert_eq!(moved.parent(), root.parent());
    assert!(
      moved.join("Default").join("Cookies").exists(),
      "the contents travel with the directory"
    );
    assert!(!cookies.exists());
    assert_eq!(moved_aside_siblings(&root), vec![moved.clone()]);

    // What the cleanup thread does with what it finds.
    delete_moved_aside(&moved_aside_siblings(&root));
    assert!(!moved.exists());
    assert!(moved_aside_siblings(&root).is_empty());
  }

  #[test]
  fn unrelated_siblings_are_not_cleanup_candidates() {
    let root = temp_root();
    let parent = root.parent().unwrap();
    fs::create_dir_all(parent.join("electron-backup")).unwrap();
    fs::create_dir_all(parent.join("other.tauri-delete")).unwrap();
    fs::write(
      parent.join("electron.1.tauri-delete"),
      b"a file, not a directory",
    )
    .unwrap();
    assert!(moved_aside_siblings(&root).is_empty());
  }

  #[test]
  fn a_root_without_a_parent_cannot_be_reset() {
    let root = PathBuf::from("/");
    assert!(move_aside(&root).is_err());
    assert!(moved_aside_siblings(&root).is_empty());
  }

  #[cfg(unix)]
  fn lock(root: &Path, target: &str) {
    std::os::unix::fs::symlink(target, root.join(SINGLETON_LOCK_FILE)).unwrap();
  }

  #[cfg(unix)]
  #[test]
  fn a_live_singleton_holder_blocks_the_reset_and_keeps_it_pending() {
    let root = temp_root();
    stamp(&root, "152.0.7977.83");
    let cookies = with_site_data(&root);
    lock(
      &root,
      &format!("{}-{}", hostname().unwrap(), std::process::id()),
    );
    let outcome = prepare(&root, v("151.0.7922.174"), DowngradePolicy::ResetProfile);
    assert!(matches!(outcome, Outcome::InUse { .. }), "{outcome:?}");
    assert!(cookies.exists());
    assert_eq!(
      recorded(&root).as_deref(),
      Some("152.0.7977.83"),
      "the breadcrumb is left to the instance holding the profile, so the reset is retried"
    );
  }

  #[cfg(unix)]
  #[test]
  fn a_lock_from_another_host_blocks_the_reset() {
    let root = temp_root();
    stamp(&root, "152.0.7977.83");
    lock(&root, "some-other-host.example-1");
    let outcome = prepare(&root, v("151.0.7922.174"), DowngradePolicy::ResetProfile);
    assert!(matches!(outcome, Outcome::InUse { .. }), "{outcome:?}");
  }

  #[cfg(unix)]
  #[test]
  fn a_stale_singleton_lock_does_not_block_the_reset() {
    let root = temp_root();
    stamp(&root, "152.0.7977.83");
    let cookies = with_site_data(&root);
    // A process that has already exited, as a crashed instance leaves behind.
    let mut child = std::process::Command::new("true").spawn().unwrap();
    child.wait().unwrap();
    lock(&root, &format!("{}-{}", hostname().unwrap(), child.id()));
    let outcome = prepare(&root, v("151.0.7922.174"), DowngradePolicy::ResetProfile);
    assert!(matches!(outcome, Outcome::Reset { .. }), "{outcome:?}");
    assert!(!cookies.exists());
  }

  #[cfg(unix)]
  #[test]
  fn a_malformed_lock_does_not_block_the_reset() {
    let root = temp_root();
    stamp(&root, "152.0.7977.83");
    lock(&root, "no-pid-here-");
    let outcome = prepare(&root, v("151.0.7922.174"), DowngradePolicy::ResetProfile);
    assert!(matches!(outcome, Outcome::Reset { .. }), "{outcome:?}");
  }
}
