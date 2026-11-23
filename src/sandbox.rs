// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Decides whether Chromium's process sandbox has to be turned off.
//!
//! On every platform the answer normally comes straight from [`SandboxPolicy`], and the
//! answer is "keep it". Only Linux and the BSDs have a case where keeping it means the
//! application cannot start at all, and detecting that case is what the bulk of this
//! module is for.
//!
//! # The Linux case
//!
//! Chromium's zygote host picks, in order, the namespace sandbox when unprivileged user
//! namespaces work, then the root-owned setuid `chrome-sandbox` helper found next to the
//! executable (or at `CHROME_DEVEL_SANDBOX`). When neither is available it calls
//! `LOG(FATAL) << "No usable sandbox!"` and the process dies before a window is ever
//! shown.
//!
//! Tauri's deb and rpm bundlers install `chrome-sandbox` with mode 4755, so packaged
//! applications always have the helper. An AppImage cannot: the runtime mounts its
//! payload with `nosuid`, so a setuid binary inside it is inert. That leaves AppImages
//! relying on unprivileged user namespaces, which Ubuntu 23.10 and later restrict
//! through AppArmor — which is exactly the combination this module detects.
//!
//! The detection ports unchanged; the one thing that moves is where the helper is looked
//! for. Under CEF it sits next to the application's own executable, because that is where
//! the bundler puts it; Electron ships `chrome-sandbox` inside its own distribution, so
//! the candidate is the Electron binary's directory. An AppImage still copies a file by
//! that name into the payload, and one inside an AppImage is never treated as available.
//! Outside an AppImage the file is stat'ed against the same conditions Chromium's zygote
//! host applies — owned by root, setuid, executable by others — because Chromium treats a
//! helper that fails them as a fatal error rather than falling back to another sandbox.
//!
//! # The Windows case
//!
//! **There is none, and that is a difference from the reference.** CEF's Windows sandbox
//! is brokered by the executable rather than by the library: CEF wants a `sandbox_info`
//! pointer passed into `CefExecuteProcess` and `CefInitialize`, and when it gets a null one
//! it sets `no_sandbox` itself — so the CEF runtime reports the Windows sandbox as
//! unavailable whatever the policy says. Electron brokers its own Windows sandbox inside
//! the Electron binary, so there is nothing for this runtime to supply and nothing to
//! report: [`windows_sandbox_unavailable`] is `false` on every platform.
//!
//! # macOS
//!
//! macOS sandboxes through the Electron framework's own helper, which it initializes
//! before the application runs, so there is nothing to probe: the policy decides on its
//! own and [`SandboxPolicy::Auto`] always keeps the sandbox.
//!
//! # What Electron cannot do at all
//!
//! - **CEF's `SandboxPolicy` granularity is not reproducible.** CEF configures the sandbox
//!   per process type (browser, GPU, utility, zygote, renderer) and per platform, through a
//!   broker pointer plus settings. Electron exposes exactly two knobs: the process-wide
//!   `--no-sandbox` switch and the per-webview `webPreferences.sandbox` boolean, which
//!   defaults to `true` since Electron 20. There is no per-process-type control and no
//!   broker.
//! - **The Electron main process cannot be sandboxed at all.** It is Node, and it runs with
//!   the user's full privileges by design; only renderers and utility processes are
//!   sandboxed. `SandboxPolicy::Required` therefore guarantees a sandbox for the web
//!   content, which is what the policy is about, and never for the process that drives it.
//! - **`Required` becomes *more* honourable, not less.** Because the Windows broker problem
//!   does not exist, `SandboxPolicy::Required` succeeds on Windows under Electron where it
//!   always failed under CEF. [`SandboxDisableReason::WindowsBrokerUnavailable`] is dead
//!   code in this runtime, kept so the decision table keeps the arm the reference has.

use crate::runtime::SandboxPolicy;

/// Whether this platform can actually sandbox, given how the runtime launches Electron.
///
/// `false` everywhere: Electron's Windows renderer sandbox is brokered by the Electron
/// binary itself, so unlike CEF there is no platform where this runtime has to hand it a
/// broker it cannot create. Kept as a function of a `cfg` rather than a `cfg` at every use
/// site so the decision table stays testable on one platform, and so the Windows arm stays
/// visible as the dead code it is instead of disappearing from the table.
pub(crate) const fn windows_sandbox_unavailable() -> bool {
  false
}

/// Why the sandbox is being turned off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SandboxDisableReason {
  /// The application asked for it through [`SandboxPolicy::Disabled`].
  Policy,
  /// AppImage, no setuid helper, and AppArmor restricts unprivileged user namespaces.
  AppImageUserNamespacesRestricted,
  /// AppImage, no setuid helper, and user namespaces are unavailable altogether.
  AppImageUserNamespacesUnavailable,
  /// Windows, where the reference's runtime cannot supply CEF with a sandbox broker.
  ///
  /// Unreachable here — Electron brokers its own — and kept so the decision table keeps
  /// the arm the reference has. See the module docs.
  WindowsBrokerUnavailable,
}

impl SandboxDisableReason {
  /// Message logged when the sandbox is dropped for this reason.
  pub(crate) fn message(self) -> &'static str {
    match self {
      Self::Policy => "the application set SandboxPolicy::Disabled",
      Self::AppImageUserNamespacesRestricted => {
        "running from an AppImage, which cannot ship the setuid chrome-sandbox helper, \
         and unprivileged user namespaces are restricted \
         (/proc/sys/kernel/apparmor_restrict_unprivileged_userns is 1)"
      }
      Self::AppImageUserNamespacesUnavailable => {
        "running from an AppImage, which cannot ship the setuid chrome-sandbox helper, \
         and unprivileged user namespaces are unavailable \
         (/proc/sys/user/max_user_namespaces is 0)"
      }
      Self::WindowsBrokerUnavailable => {
        "the Windows sandbox needs a broker this runtime cannot supply: Electron brokers \
         its own, so this reason is unreachable and never reported"
      }
    }
  }
}

/// What the decision table concluded, in the reference's three-way shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SandboxVerdict {
  /// Leave the sandbox alone.
  Keep,
  /// Append `--no-sandbox`, logging `reason`.
  Disable(SandboxDisableReason),
  /// [`SandboxPolicy::Required`] asked for a sandbox this platform cannot provide, so
  /// startup fails rather than silently running without one.
  Refuse(SandboxDisableReason),
}

/// The answer the runtime acts on: whether `--no-sandbox` goes on the Electron command
/// line.
///
/// The reference hands its three-way [`SandboxVerdict`] back to the runtime, which matches
/// on it and logs the reason it carries. This runtime assembles the command line before the
/// Electron child exists and reads nothing but the switch decision, so the verdict is
/// narrowed here — the reason is logged where it is decided, in [`narrow`], rather than
/// travelling in a field the caller would never look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SandboxDecision {
  /// Whether the runtime appends `--no-sandbox`.
  pub(crate) no_sandbox: bool,
}

/// Decides whether to disable the sandbox, from inputs the caller has already gathered.
///
/// Kept free of I/O so every combination can be unit tested.
///
/// `apparmor_restrict_unprivileged_userns` and `max_user_namespaces` are the values read
/// from `/proc/sys/kernel/apparmor_restrict_unprivileged_userns` and
/// `/proc/sys/user/max_user_namespaces`; [`None`] means the file could not be read,
/// which is treated as no evidence of a restriction rather than as a restriction.
///
/// `windows_broker_unavailable` is [`windows_sandbox_unavailable`], which is `false`
/// everywhere under Electron. The parameter stays so the arm the reference needs is still
/// exercised by the tests rather than deleted.
pub(crate) fn sandbox_decision(
  policy: SandboxPolicy,
  windows_broker_unavailable: bool,
  running_from_appimage: bool,
  sandbox_helper_available: bool,
  apparmor_restrict_unprivileged_userns: Option<u64>,
  max_user_namespaces: Option<u64>,
) -> SandboxVerdict {
  if let SandboxPolicy::Disabled = policy {
    return SandboxVerdict::Disable(SandboxDisableReason::Policy);
  }

  // A platform that cannot supply a broker overrules the policy a moment later, so `Keep`
  // here would be a decision the platform undoes. Reporting it instead keeps the rule that
  // a lost sandbox is always named out loud. Nothing under Electron sets this.
  if windows_broker_unavailable {
    return match policy {
      SandboxPolicy::Required => {
        SandboxVerdict::Refuse(SandboxDisableReason::WindowsBrokerUnavailable)
      }
      _ => SandboxVerdict::Disable(SandboxDisableReason::WindowsBrokerUnavailable),
    };
  }

  match policy {
    SandboxPolicy::Disabled => SandboxVerdict::Disable(SandboxDisableReason::Policy),
    SandboxPolicy::Required => SandboxVerdict::Keep,
    SandboxPolicy::Auto => {
      // Everything but an AppImage can ship the setuid helper, so a missing sandbox
      // there is a packaging or system problem we should not paper over.
      if !running_from_appimage || sandbox_helper_available {
        return SandboxVerdict::Keep;
      }

      if apparmor_restrict_unprivileged_userns == Some(1) {
        SandboxVerdict::Disable(SandboxDisableReason::AppImageUserNamespacesRestricted)
      } else if max_user_namespaces == Some(0) {
        SandboxVerdict::Disable(SandboxDisableReason::AppImageUserNamespacesUnavailable)
      } else {
        SandboxVerdict::Keep
      }
    }
  }
}

/// Narrows the table's answer to what the runtime acts on, logging what it decided.
///
/// The reference hands the verdict to its caller, which logs it and, for `Required`, fails
/// startup. This runtime's caller only reads [`SandboxDecision::no_sandbox`], so the log
/// line and the refusal happen here — otherwise a dropped sandbox would go unnamed, which
/// is the one thing this module exists to prevent.
fn narrow(verdict: SandboxVerdict) -> Option<SandboxDecision> {
  match verdict {
    SandboxVerdict::Keep => None,
    SandboxVerdict::Disable(reason) => {
      log::warn!(
        "running Chromium without a sandbox: {}. A compromised renderer process runs with \
         the full privileges of the current user.",
        reason.message()
      );
      Some(SandboxDecision { no_sandbox: true })
    }
    SandboxVerdict::Refuse(reason) => {
      // Unreachable under Electron: nothing reports a missing broker any more, and the
      // AppImage arms only ever disable. Reported rather than quietly turned into `Keep`,
      // so a future platform input cannot lose the sandbox without a word. Keeping the
      // sandbox is the closest this signature can come to refusing to start, and it is
      // what `Required` asked for.
      log::error!(
        "refusing to start: SandboxPolicy::Required asked for a Chromium sandbox, but {}.",
        reason.message()
      );
      crate::unsupported::report(
        "SandboxPolicy::Required: the runtime cannot fail startup from its command-line assembly",
      );
      None
    }
  }
}

/// Whether this process was launched with Chromium's `--no-sandbox` switch.
///
/// The reference needs this because a CEF helper process inherits the switch from the
/// browser process and has to know that entering the sandbox would be wrong. This runtime
/// sandboxes nothing itself: Electron launches and sandboxes its own helper processes, so
/// there is no helper of ours to ask. The function stays for shape parity and answers
/// [`false`] — "this process is not a helper this runtime launched without a sandbox",
/// which is the truth on every launch.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub(crate) fn launched_without_sandbox() -> bool {
  false
}

/// Whether a `chrome-sandbox` candidate passes the checks Chromium's zygote host makes
/// before it will use the helper, given the `st_uid` and `st_mode` a `stat` reported.
///
/// `ZygoteHostImpl::Init` requires the file to be owned by root, to carry the setuid bit
/// and to be executable by others; a file that is there but fails any of those aborts
/// with "The SUID sandbox helper binary was found, but is not configured correctly", so
/// a half-configured helper must not count as available.
#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
pub(crate) fn helper_stat_is_usable(uid: u32, mode: u32) -> bool {
  /// `S_ISUID`.
  const SETUID: u32 = 0o4000;
  /// `S_IXOTH`.
  const OTHER_EXECUTE: u32 = 0o0001;

  uid == 0 && mode & SETUID != 0 && mode & OTHER_EXECUTE != 0
}

/// Gathers the inputs [`sandbox_decision`] needs from the environment and the filesystem.
#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
pub(crate) fn resolve_sandbox_decision(policy: SandboxPolicy) -> Option<SandboxDecision> {
  let running_from_appimage = running_from_appimage();
  narrow(sandbox_decision(
    policy,
    windows_sandbox_unavailable(),
    running_from_appimage,
    sandbox_helper_available(running_from_appimage),
    read_sysctl("/proc/sys/kernel/apparmor_restrict_unprivileged_userns"),
    read_sysctl("/proc/sys/user/max_user_namespaces"),
  ))
}

/// The policy's own answer, with nothing to probe: neither Windows nor macOS has an
/// equivalent of the AppImage case. Both keep the sandbox under [`SandboxPolicy::Auto`],
/// because Electron brokers it on both.
#[cfg(not(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
)))]
pub(crate) fn resolve_sandbox_decision(policy: SandboxPolicy) -> Option<SandboxDecision> {
  narrow(sandbox_decision(
    policy,
    windows_sandbox_unavailable(),
    false,
    false,
    None,
    None,
  ))
}

/// AppImage runtimes export `APPIMAGE` with the path of the mounted image.
#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
fn running_from_appimage() -> bool {
  std::env::var_os("APPIMAGE").is_some_and(|path| !path.is_empty())
}

/// The directory the Electron binary lives in, which is where Electron ships
/// `chrome-sandbox`.
///
/// The same resolution `Electron::resolve_binary` performs, repeated here because the
/// command line is assembled before that value is available to this module: the staged
/// distribution first, then the two layouts a packaged application uses.
#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
fn electron_binary_directory() -> Option<std::path::PathBuf> {
  if let Ok(binary) = electron_rs::electron_binary!(None::<&std::path::Path>) {
    return binary.parent().map(|parent| parent.to_path_buf());
  }

  let executable = std::env::current_exe().ok()?;
  let dir = executable.parent()?;
  for candidate in [dir.join("electron"), dir.join("resources").join("electron")] {
    if let Ok(binary) = electron_rs::electron_binary(&candidate) {
      return binary.parent().map(|parent| parent.to_path_buf());
    }
  }
  None
}

/// Whether Chromium can find *and use* the setuid `chrome-sandbox` helper.
///
/// The helper inside the distribution is disregarded entirely when running from an
/// AppImage: the bundler copies `chrome-sandbox` into the payload, and the AppImage runtime
/// mounts it `nosuid`, so the setuid bit `stat` still reports has no effect.
///
/// `CHROME_DEVEL_SANDBOX` is somebody deliberately pointing at a helper outside the
/// distribution, so it is honoured on every layout, but it is stat'ed like any other
/// candidate.
#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
fn sandbox_helper_available(running_from_appimage: bool) -> bool {
  if let Some(path) = std::env::var_os("CHROME_DEVEL_SANDBOX").filter(|path| !path.is_empty()) {
    return helper_path_is_usable(std::path::Path::new(&path));
  }

  if running_from_appimage {
    return false;
  }

  electron_binary_directory().is_some_and(|dir| helper_path_is_usable(&dir.join("chrome-sandbox")))
}

/// Stats `path` and hands what it reports to [`helper_stat_is_usable`].
#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
fn helper_path_is_usable(path: &std::path::Path) -> bool {
  use std::os::unix::fs::MetadataExt;

  let Ok(metadata) = std::fs::metadata(path) else {
    return false;
  };

  let usable = helper_stat_is_usable(metadata.uid(), metadata.mode());
  if !usable {
    log::debug!(
      "ignoring the chrome-sandbox helper at {}: it is not a root-owned setuid binary executable by others",
      path.display()
    );
  }
  usable
}

/// Reads a numeric sysctl, returning [`None`] when it is missing or unparseable.
#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
fn read_sysctl(path: &str) -> Option<u64> {
  std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Shorthand for the `Auto` policy, which is the only one that inspects the system, on
  /// a platform whose sandbox broker works.
  fn auto(
    running_from_appimage: bool,
    sandbox_helper_available: bool,
    apparmor: Option<u64>,
    max_user_namespaces: Option<u64>,
  ) -> SandboxVerdict {
    sandbox_decision(
      SandboxPolicy::Auto,
      false,
      running_from_appimage,
      sandbox_helper_available,
      apparmor,
      max_user_namespaces,
    )
  }

  #[test]
  fn explicit_policies_ignore_the_system() {
    for appimage in [false, true] {
      for helper in [false, true] {
        assert_eq!(
          sandbox_decision(
            SandboxPolicy::Disabled,
            false,
            appimage,
            helper,
            Some(1),
            Some(0)
          ),
          SandboxVerdict::Disable(SandboxDisableReason::Policy)
        );
        assert_eq!(
          sandbox_decision(
            SandboxPolicy::Required,
            false,
            appimage,
            helper,
            Some(1),
            Some(0)
          ),
          SandboxVerdict::Keep,
          "Required must keep the sandbox even when Chromium will abort"
        );
      }
    }
  }

  #[test]
  fn the_table_still_answers_for_a_broker_nothing_reports() {
    // Dead code under Electron, but the arm stays in the table rather than disappearing:
    // a platform that could not supply a broker must never claim `Keep`.
    assert_eq!(
      sandbox_decision(SandboxPolicy::Auto, true, false, false, None, None),
      SandboxVerdict::Disable(SandboxDisableReason::WindowsBrokerUnavailable)
    );
    assert_eq!(
      sandbox_decision(SandboxPolicy::Required, true, false, false, None, None),
      SandboxVerdict::Refuse(SandboxDisableReason::WindowsBrokerUnavailable)
    );
  }

  #[test]
  fn disabled_is_answered_before_the_platform_is_consulted() {
    // An application that asked for no sandbox is told what it asked for, not what the
    // platform could not give it.
    assert_eq!(
      sandbox_decision(SandboxPolicy::Disabled, true, false, false, None, None),
      SandboxVerdict::Disable(SandboxDisableReason::Policy)
    );
  }

  #[test]
  fn auto_keeps_the_sandbox_outside_an_appimage() {
    // A deb or rpm install ships the setuid helper, and a system that lost it should
    // fail loudly rather than silently run unsandboxed.
    assert_eq!(auto(false, false, Some(1), Some(0)), SandboxVerdict::Keep);
    assert_eq!(auto(false, true, None, None), SandboxVerdict::Keep);
  }

  #[test]
  fn auto_keeps_the_sandbox_when_the_helper_is_available() {
    assert_eq!(auto(true, true, Some(1), Some(0)), SandboxVerdict::Keep);
  }

  #[test]
  fn auto_keeps_the_sandbox_when_user_namespaces_work() {
    assert_eq!(
      auto(true, false, Some(0), Some(31231)),
      SandboxVerdict::Keep
    );
  }

  #[test]
  fn auto_disables_for_an_appimage_restricted_by_apparmor() {
    assert_eq!(
      auto(true, false, Some(1), Some(31231)),
      SandboxVerdict::Disable(SandboxDisableReason::AppImageUserNamespacesRestricted)
    );
  }

  #[test]
  fn auto_disables_for_an_appimage_without_user_namespaces() {
    assert_eq!(
      auto(true, false, Some(0), Some(0)),
      SandboxVerdict::Disable(SandboxDisableReason::AppImageUserNamespacesUnavailable)
    );
    // The AppArmor sysctl only exists on kernels carrying that patch.
    assert_eq!(
      auto(true, false, None, Some(0)),
      SandboxVerdict::Disable(SandboxDisableReason::AppImageUserNamespacesUnavailable)
    );
  }

  #[test]
  fn unreadable_sysctls_are_not_evidence_of_a_restriction() {
    // Nothing readable: assume namespaces work and let Chromium have the last word.
    assert_eq!(auto(true, false, None, None), SandboxVerdict::Keep);
  }

  #[test]
  fn apparmor_restriction_is_reported_over_a_missing_namespace_quota() {
    // Both point the same way; the AppArmor one is the actionable message.
    assert_eq!(
      auto(true, false, Some(1), Some(0)),
      SandboxVerdict::Disable(SandboxDisableReason::AppImageUserNamespacesRestricted)
    );
  }

  /// The platforms with nothing to probe answer from the policy alone, which is what
  /// [`resolve_sandbox_decision`] passes there. Asserted everywhere so the contract
  /// cannot drift on the platforms that do not compile that arm.
  #[test]
  fn nothing_to_probe_means_the_policy_decides() {
    assert_eq!(
      sandbox_decision(SandboxPolicy::Auto, false, false, false, None, None),
      SandboxVerdict::Keep,
      "Auto must keep the sandbox where there is no AppImage case to escape"
    );
    assert_eq!(
      sandbox_decision(SandboxPolicy::Required, false, false, false, None, None),
      SandboxVerdict::Keep
    );
    assert_eq!(
      sandbox_decision(SandboxPolicy::Disabled, false, false, false, None, None),
      SandboxVerdict::Disable(SandboxDisableReason::Policy),
      "Disabled is the only way for macOS to lose the sandbox"
    );
  }

  #[test]
  fn narrowing_turns_a_disable_into_the_switch_and_nothing_else_into_one() {
    assert_eq!(narrow(SandboxVerdict::Keep), None);
    assert_eq!(
      narrow(SandboxVerdict::Disable(SandboxDisableReason::Policy)),
      Some(SandboxDecision { no_sandbox: true })
    );
    assert_eq!(
      narrow(SandboxVerdict::Refuse(
        SandboxDisableReason::WindowsBrokerUnavailable
      )),
      None,
      "an unreachable refusal keeps the sandbox rather than dropping it"
    );
  }

  /// Electron brokers its own Windows sandbox, so the reference's Windows gap is gone.
  /// Asserted on every platform so the constant cannot drift back to `cfg!(windows)`.
  #[test]
  fn electron_never_reports_a_missing_windows_broker() {
    assert!(!windows_sandbox_unavailable());
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  #[test]
  fn a_correctly_installed_helper_is_usable() {
    // Mode 4755, which is what the deb and rpm bundlers install.
    assert!(helper_stat_is_usable(0, 0o104755));
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  #[test]
  fn a_helper_missing_any_of_chromiums_conditions_is_not_usable() {
    // Chromium aborts outright on a helper that fails these, so "present but wrong" has
    // to read as unavailable.
    assert!(
      !helper_stat_is_usable(1000, 0o104755),
      "a helper not owned by root cannot raise privileges"
    );
    assert!(
      !helper_stat_is_usable(0, 0o100755),
      "without the setuid bit the helper runs as the calling user"
    );
    assert!(
      !helper_stat_is_usable(0, 0o104750),
      "the helper has to be executable by others"
    );
    // What `fs::copy` produces in an AppDir: right name, none of the bits.
    assert!(!helper_stat_is_usable(1000, 0o100644));
  }
}
