# Module parity spec — `tauri-runtime-cef` → `tauri-runtime-electron`

Implementation contract for the modules in scope. Reference: `source/tauri/crates/tauri-runtime-cef/`
(Tauri v3 CEF runtime, version `3.0.0-alpha.7`). Target: `src/` of `tauri-runtime-electron`
(branch `next`), which drives an Electron main process over the `electron-rs` JSON-RPC bridge.

The target must mirror the reference **file for file**, with `Cef*` → `Electron*` renaming and no
renaming of anything that is not CEF-specific. Every declaration below is copied verbatim from the
reference, with its line number.

---

## 0. Conventions

### 0.1 Effective visibility

Almost nothing in this scope is *externally* public. `lib.rs` (reference, lines 18–20, 21, 38) re-exports
exactly three things from these files:

```rust
pub use dialog::{
  NativeDialogKind, NativeDialogObservation, NativeDialogSnapshot, NativeDialogToken,
};
pub use environment::DebugEnvironment;
pub use devtools::{DevToolsMessageIdExhausted, allocate_devtools_message_id};
```

Everything else is `pub` only inside a private module (`mod cef_impl;`, `mod external_message_pump;`,
`mod sandbox;`, …), so it is **crate-internal** even when spelled `pub`. The spec marks each item
`[ext]` (reachable from outside the crate) or `[int]` (crate-internal, `pub` or `pub(crate)`).
`AppWebview` is `pub(crate)` (`webview.rs:496`), so `impl AppWebview { pub fn cookie_manager }` in
`cookie.rs` is `[int]` too.

Parity therefore has two halves: the three `[ext]` re-export groups must keep their exact names and
shapes, and every `[int]` item must keep its name and role so the rest of the crate ports mechanically.

### 0.2 Feasibility tags

| tag | meaning |
| --- | --- |
| `direct` | Electron/electron-rs has an equivalent API; the item maps one-to-one. |
| `emulated` | Achievable by combining Electron APIs; behaviour is reproduced, mechanism differs. |
| `absent` | Electron/Chromium exposes no equivalent. The item must still exist for API-shape parity, with a documented no-op / always-`false` / always-`None` body. |

### 0.3 Electron mechanism vocabulary (electron-rs, pinned rev `c97c052`)

The pinned `electron-rs` is rev `c97c052ba2f923912dbeba4b277a45d57091d958` and stages Electron
`44.7.0` (`electron_rs::DEFAULT_VERSION`). Everything below is verified against that checkout
(`.temp/electron-rs`), not assumed.

Requests (`Electron::call`, `electron_rs::rpc::*`), verified against `.temp/electron-rs/src/rpc.rs`:

- webview: `webview.create`, `webview.url`, `webview.eval`, `webview.navigate`, `webview.reload`,
  `webview.setAutoResize`, `webview.goBack`/`canGoBack`/`goForward`/`canGoForward`,
  `webview.openDevtools`/`closeDevtools`/`isDevtoolsOpen`, `webview.setZoom`, `webview.reparent`,
  `webview.bounds`/`setBounds`, `webview.visibility`, `webview.backgroundColor`, `webview.focus`,
  `webview.print`, `webview.capture`, `webview.close`, `webview.cookies`/`setCookie`/`deleteCookie`/
  `clearBrowsingData`, `webview.newWindowResponse`, `webview.navigationResponse`,
  `webview.downloadResponse`, `webview.permissionResponse`.
- browser: `browser.acquire`/`browser.release`; `webview.trustedOrigin`, `webview.trustedOriginAndNavigate`.
- protocol: `protocol.register`, `protocol.respond`; `webHost.authenticate`/`cookieHeader`/`bind`.
- window: `window.create`, `window.event`-driven commands, `window.popupMenu`, `window.setMenu`, …
- app: `app.setMenu`, `app.theme`, `app.focus`/`show`/`hide`, `app.activationPolicy`,
  `app.dockVisibility`, `app.badgeLabel`, `shell.openExternal`, …

Events (`Electron::events()` / `subscribe()`, `Event = serde_json::Value` with `event` + `params`):

- `webview.navigationRequested`, `webview.navigationCommitted`, `webview.loadStarted`,
  `webview.loadFinished`, `webview.loadFailed`, `webview.titleChanged`, `webview.processGone`,
  `webview.closed`, `webview.attached`, `webview.newWindow`.
- `download.requested`, `download.finished`, `permission.requested`, `protocol.request`.
- `window.event` (carries `appCommand` among others), `window.ready`, `window.closed`,
  `window.closeRequested`, `app.menuCommand`, `app.reopen`, `app.exitRequested`, `ipc.closed`.

The Electron main-process side of each of these is TypeScript in `electron-rs`'s
`assets/runtime/*.cjs`; the Rust crate only names the method/event.

---

## 1. `src/dialog.rs` (207 lines)

### 1.1 Purpose

Observes CEF's *existing* native JavaScript dialog UI without taking it over. There is no custom
dialog handler, callback, message text or prompt value retained anywhere. The events arrive through
the DevTools `Page` domain the runtime enables on every browser: enabling that domain observes
dialogs without owning them, because Chromium notifies each enabled `PageHandler` and *then* still
runs the browser's own dialog manager, which CEF always supplies. A dialog only stalls a page when no
browser handler exists at all — which the protocol reports as `hasBrowserHandler == false` — so the
runtime never has to answer a dialog with `Page.handleJavaScriptDialog`. The module exists so an
application can answer "is a modal dialog up, and of what kind" without the runtime becoming the
dialog's owner.

### 1.2 Public items

`[ext]` **`NativeDialogKind`** — `dialog.rs:19-27`

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum NativeDialogKind {
  Alert,
  Confirm,
  Prompt,
  BeforeUnload,
}
```

No per-variant docs. Deserialized from CDP's `type` field, so the wire spellings are `alert`,
`confirm`, `prompt`, `beforeunload`. `#[non_exhaustive]` means downstream matches need a `_` arm.

`[ext]` **`NativeDialogToken`** — `dialog.rs:29-43`

```rust
/// Opaque lifetime of one native JavaScript dialog notification. Compare with
/// the current UI-thread snapshot before submitting any dialog action.
#[derive(Clone)]
pub struct NativeDialogToken(Arc<()>);
```

with `impl PartialEq` (`Arc::ptr_eq`), `impl Eq`, and a manual `Debug` that prints
`NativeDialogToken { .. }` (`finish_non_exhaustive`) so the token value never leaks into logs.
Doc intent: identity of *one opening*, not of a dialog kind — a reopen is a different token, and a
token from another browser is never equal.

`[ext]` **`NativeDialogSnapshot`** — `dialog.rs:45-53`

```rust
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct NativeDialogSnapshot {
  pub token: NativeDialogToken,
  pub kind: NativeDialogKind,
  /// Chromium reports a non-DevTools dialog handler. This does not establish
  /// native visibility, the exact button label, or the current prompt value.
  pub has_browser_handler: bool,
}
```

`[ext]` **`NativeDialogObservation`** — `dialog.rs:55-61`

```rust
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct NativeDialogObservation {
  /// A dialog event was observed for this document. False never means absent.
  pub known: bool,
  pub dialog: Option<NativeDialogSnapshot>,
}
```

Doc intent: `known == false` means "this document was never observed", **not** "there is no dialog".
`Default` is `{ known: false, dialog: None }`.

`[int]` **`DialogState`** — `dialog.rs:69-73`, methods `dialog.rs:74-133`

```rust
#[derive(Clone)]
pub(crate) struct DialogState {
  browser: FrameNavigationState,
  state: Arc<Mutex<ObservedDialog>>,
}
impl DialogState {
  pub(crate) fn new(browser: FrameNavigationState) -> Self
  pub(crate) fn accepts_browser(&self, id: i32) -> bool
  pub(crate) fn snapshot(&self, document: Option<&NativeDocumentToken>) -> NativeDialogObservation
  pub(crate) fn on_event(&self, method: &str, params: &[u8])
}
```

Callers: `webview.rs:503` (field `dialogs: crate::dialog::DialogState` on `AppWebview`),
`webview.rs:761`, `webview.rs:2148`, `webview.rs:2315`; `popup.rs:40`, `popup.rs:134`.

Private support: `struct ObservedDialog { document: Option<NativeDocumentToken>, observation: NativeDialogObservation }`
(`dialog.rs:63-67`).

### 1.3 Behavioural rules

1. **Observation only.** Nothing in the module may call `Page.handleJavaScriptDialog`, retain
   `message`/`defaultPrompt`, or install a dialog handler.
2. **`on_event(method, params)`** dispatches on `method`:
   - `"Page.javascriptDialogOpening"`: parse only when `params.len() <= 262_144` (256 KiB). Deserialize
     into a private `Opening { #[serde(rename="type")] kind: NativeDialogKind, #[serde(rename="hasBrowserHandler")] has_browser_handler: bool }`.
     On success produce `NativeDialogObservation { known: true, dialog: Some(NativeDialogSnapshot { token: NativeDialogToken(Arc::new(())), kind, has_browser_handler }) }`.
     Over-long input, malformed JSON, or an unknown `type` string all fall through to
     `NativeDialogObservation::default()` — i.e. `{ known: false, dialog: None }`, which *revokes*
     any previously held reference (fail closed).
   - `"Page.javascriptDialogClosed"`: `{ known: true, dialog: None }`. The payload is never parsed —
     it may carry the prompt value.
   - Any other method: `return` immediately, state untouched.
   - Afterwards: `let document = self.browser.document();` and, when the mutex locks, replace the
     whole `ObservedDialog { document, observation }`. The observation is therefore bound to the
     document that was current when the event arrived.
3. **`snapshot(document)`** returns the stored observation only when the stored `document` equals the
   argument (`Option<NativeDocumentToken>` equality); otherwise `NativeDialogObservation::default()`.
   A poisoned mutex also yields the default (`.ok()` → `filter` → `map` → `unwrap_or_default`). A
   navigation therefore invalidates the observation; the caller must pass the *current* document
   token.
4. **`accepts_browser(id)`** delegates to `browser.has_browser_id(id)` — the dialog state belongs to
   one browser, and a DevTools window or popup routed through the same client is not it.
5. **Token identity.** Each opening allocates a fresh `Arc<()>`, so two openings never compare equal,
   even of the same kind, and tokens from different `DialogState`s never compare equal.
6. Tests (`dialog.rs:135-207`) pin the contract: reopen rotates the token; tokens do not cross
   browsers; `accepts_browser(2) == false`; `browser.close()` makes `snapshot` report
   `known == false`; a payload missing `hasBrowserHandler` fails closed; an unknown `type` yields
   `dialog == None`.

### 1.4 Electron feasibility

| item | tag | mechanism / reason |
| --- | --- | --- |
| `NativeDialogKind` | `absent` | Electron implements `content::JavaScriptDialogManager` with a no-op `RunJavaScriptDialog`, so `alert`/`confirm`/`prompt` never open a native dialog; `window.prompt()` throws `prompt() is not supported`. There is no dialog to classify and no `webContents` dialog event. Keep the enum verbatim for API-shape parity. |
| `NativeDialogToken` | `absent` | No native dialog lifetime exists to name. Keep the opaque `Arc<()>` type so downstream code keeps compiling; it can only ever be constructed by an emulated path. |
| `NativeDialogSnapshot` | `absent` | `has_browser_handler` has no meaning: Electron's dialog manager *is* the browser handler and it suppresses. |
| `NativeDialogObservation` | `absent` | The honest body is `{ known: false, dialog: None }` for every document, forever. |
| `DialogState` (`[int]`) | `absent` | No CDP dialog events are ever delivered. `on_event` keeps its signature and does nothing; `snapshot` returns the default; `accepts_browser` keeps delegating to `FrameNavigationState`. |

Two facts that close the emulation routes:

- `webContents.debugger.attach('1.3')` + `Page.enable` *would* deliver `Page.javascriptDialogOpening`
  if Chromium raised it — but Electron's renderer never raises it, because its
  `JavaScriptDialogManager` returns without showing anything.
- Electron permits **one** CDP session per `WebContents`; `debugger.attach()` rejects while DevTools
  is open ("Could not attach debugger. DevTools may already be open for this tab."). Any
  dialog-observation scheme would therefore also conflict with the runtime's own DevTools support.

The only fragment with a real Electron signal is `BeforeUnload`: `webContents.on('will-prevent-unload')`
fires when a page's `beforeunload` handler blocks the unload. It carries no dialog identity, no kind,
and no handler fact, and Electron's default is to close anyway, so it cannot be shaped into a
`NativeDialogSnapshot`. Document this in the module docs rather than faking it.

---

## 2. `src/downgrade.rs` (598 lines)

### 2.1 Purpose

Detects a launch on a Chromium profile last written by a *newer* Chromium, and applies the
application's `DowngradePolicy`. Chromium migrates a profile forward only: `Local State`, each
profile's `Preferences`, and the SQLite/LevelDB stores behind cookies, local storage and IndexedDB are
written in the running version's format, and an older version opening them gets undefined behaviour —
unknown preferences dropped on the next write, newer-schema databases left unused or discarded. Chrome
calls this an unsupported downgrade (`chrome/browser/downgrade/`) and keeps a `Last Version`
breadcrumb to recognise it. CEF's Chrome bootstrap writes no such breadcrumb, so this module writes it
on every launch and, when it names a higher milestone, either keeps the profile with a warning (Chrome's
behaviour) or moves the whole directory aside and starts empty. A downgrade *within* a milestone is
left alone, as Chrome leaves it (`DowngradeManager::kMinorDowngrade`). A reset never runs under another
live instance.

### 2.2 Public items

No `[ext]` items. Everything is `pub(crate)` or private.

`[int]` **`LAST_VERSION_FILE`** — `downgrade.rs:48`

```rust
pub(crate) const LAST_VERSION_FILE: &str = "Last Version";
```

Doc intent: same name, format and meaning as Chrome's downgrade-manager file in its user data
directory (`chrome/browser/downgrade/user_data_downgrade.cc`), so the two would agree if CEF ever
started writing it. Used at `downgrade.rs:180, 208, 214` and in tests.

`[int]` **`ChromiumVersion`** — `downgrade.rs:59-65`, impl `downgrade.rs:67-102`, `Display` `104-112`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ChromiumVersion {
  pub major: u32,
  pub minor: u32,
  pub build: u32,
  pub patch: u32,
}

impl ChromiumVersion {
  pub(crate) fn embedded() -> Self
  pub(crate) fn parse(text: &str) -> Option<Self>
}
```

`Display` writes `{major}.{minor}.{build}.{patch}` with no trailing newline. The derived `Ord` is
lexicographic over the field order, which is the version order.

`[int]` **`Outcome`** — `downgrade.rs:115-137`

```rust
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
```

`[int]` **`prepare_root_cache_path`** — `downgrade.rs:143`

```rust
pub(crate) fn prepare_root_cache_path(root: &Path, policy: DowngradePolicy) -> Outcome
```

Caller: `runtime.rs:3311`, immediately after `create_dir_all(&cache_path)` and before
`cef::initialize` — the first CEF call that opens anything inside the directory. Contract: browser
process only, directory exists, before CEF initializes.

Private support: `MOVED_ASIDE_SUFFIX` (`:51`, `".tauri-delete"`), `SINGLETON_LOCK_FILE` (`:56`,
`"SingletonLock"`, `#[cfg(unix)]`), `prepare` (`:147`), `report` (`:186`), `read_last_version`
(`:207`), `write_last_version` (`:213`), `move_aside` (`:218`), `moved_aside_siblings` (`:243`),
`delete_in_background` (`:267`), `delete_moved_aside` (`:279`), `singleton_holder` (`:297` unix /
`:315` non-unix), `hostname` (`:321`, unix).

### 2.3 Behavioural rules

1. **Order inside `prepare(root, current, policy)`:**
   1. `delete_in_background(moved_aside_siblings(root))` — finish a reset a previous launch started
      and did not finish deleting. This happens *before* the breadcrumb is read, so a stale
      `.tauri-delete` sibling never affects the decision.
   2. Read and classify.
   3. `report(&outcome, current)`.
   4. Write the breadcrumb — unless the outcome is `InUse` or `ResetFailed`.
2. **Classification** (`downgrade.rs:151-169`), in this order:
   - no breadcrumb, or a breadcrumb that does not parse → `Outcome::Unknown`
   - `last.major < current.major || last <= current` → `Outcome::Kept` (upgrade, or the same version)
   - `last.major == current.major` → `Outcome::MinorDowngrade { last }`
   - otherwise (higher milestone), by policy:
     - `DowngradePolicy::KeepProfile` → `Outcome::KeptNewer { last }`
     - `DowngradePolicy::ResetProfile` → `singleton_holder(root)`:
       - `Some(holder)` → `Outcome::InUse { last, holder }`
       - `None` → `move_aside(root)`:
         - `Ok(moved)` → `fs::create_dir_all(root)`, `delete_in_background(vec![moved])`,
           `Outcome::Reset { last }`
         - `Err(error)` → `Outcome::ResetFailed { last, error }`
3. **Breadcrumb write.** `fs::write(root.join(LAST_VERSION_FILE), version.to_string())` — UTF-8,
   `MAJOR.MINOR.BUILD.PATCH`, no trailing newline. Skipped for `InUse` and `ResetFailed` so the
   breadcrumb keeps naming the newer version and the next launch retries; an instance that does hold
   the profile is also not told it ran an older Chromium than it did. A failed write logs a warning
   naming the path and stating that a rollback to an older CEF will not be detected on the next launch.
4. **`ChromiumVersion::parse`.** `text.trim().split('.')`; the first component is mandatory; up to
   three more fill `minor`, `build`, `patch`, missing ones default to `0`; a fifth component makes the
   whole parse `None`; any non-`u32` component makes it `None` (so `"-1"`, `"abc"`, `"152.x"` fail).
   `"152"` → `152.0.0.0`; `" 151.0.7922.174\n"` parses.
5. **`ChromiumVersion::embedded()`** reads `cef::sys::CHROME_VERSION_{MAJOR,MINOR,BUILD,PATCH}`. Safe
   because `cef::initialize` refuses a `libcef` whose API hash differs from the compiled-in one, so the
   loaded library never disagrees with the constants.
6. **`move_aside`.** Requires both `root.parent()` and `root.file_name()`, else
   `io::Error::new(io::ErrorKind::InvalidInput, "the root cache path has no parent directory to move it within")`.
   Target name is `<name>.<millis-since-UNIX_EPOCH>.tauri-delete` in the same parent (same volume);
   on collision the stamp is incremented in a loop until the target does not exist. Returns the moved
   path.
7. **`moved_aside_siblings`.** Entries of `root.parent()` whose file name (a) starts with `<name>.`,
   (b) ends with `.tauri-delete`, (c) is a directory. Non-UTF-8 names, plain files, and unrelated
   siblings (`cef-backup`, `other.tauri-delete`) are ignored. A root without a parent yields `Vec::new()`.
8. **`delete_in_background`.** No-op on an empty list. Spawns a thread named
   `"tauri-cef-profile-cleanup"`; a spawn failure logs `log::warn!`. `delete_moved_aside` logs a
   warning per path whose `fs::remove_dir_all` fails and continues with the rest.
9. **`singleton_holder` (unix).** `fs::read_link(root.join("SingletonLock"))`; the target is
   `<hostname>-<pid>`; `rsplit_once('-')`, `pid.parse::<i32>()`. If `hostname()` differs from the
   recorded host, the lock counts as **held** (`Some("process {pid} on {host}")`) because it cannot be
   checked — which is also Chromium's treatment. Otherwise `kill(pid, 0) == 0 || errno == EPERM` →
   `Some("process {pid}")`, else `None` (stale). A malformed lock (`no-pid-here-`) yields `None`.
10. **`singleton_holder` (non-unix).** Always `None`. Windows has no lock to read: `ProcessSingleton`
    there is a named window plus a file opened without delete sharing, and the rename itself fails
    while another process holds files open inside the directory — which surfaces as `ResetFailed`.
11. **`hostname()`.** `libc::gethostname` into `[u8; 256]`, cut at the first NUL, lossy UTF-8; `None`
    on a non-zero return.
12. **Logging** (`report`, `downgrade.rs:186-205`): `Unknown | Kept` → nothing; `MinorDowngrade` →
    `log::info!` ("…a later build of the milestone this Chromium {current} belongs to; keeping it, as
    Chrome does"); `KeptNewer` → `log::warn!` naming both versions, stating that Chromium never
    migrates a profile backwards and pointing at `DowngradePolicy::ResetProfile`; `Reset` → `log::warn!`
    naming both versions and listing what is gone (cookies, local storage, IndexedDB, granted
    permissions); `InUse` → `log::warn!` naming the holder; `ResetFailed` → `log::warn!` with the
    error and the note that the reset is retried on the next launch.
13. Tests (`downgrade.rs:335-597`) pin: an unreadable breadcrumb counts as `Unknown`; a profile of
    unknown origin is never reset but does get a breadcrumb; upgrade and same-version keep the
    profile and move the breadcrumb forward; a minor downgrade keeps it and records the older
    version; `KeepProfile` keeps a newer milestone's profile and records the older version;
    `ResetProfile` moves it aside, recreates the root, and leaves nothing of the newer profile; the
    moved directory stays a sibling and is found again by `moved_aside_siblings`; unrelated siblings
    are not candidates; a root without a parent cannot be reset; a live singleton holder blocks the
    reset *and leaves the breadcrumb pending*; a lock from another host blocks it; a stale lock and a
    malformed lock do not.

### 2.4 Electron feasibility

| item | tag | mechanism / reason |
| --- | --- | --- |
| `LAST_VERSION_FILE` | `direct` | Pure constant. |
| `ChromiumVersion::parse` / `Display` / `Ord` | `direct` | Pure Rust. |
| `ChromiumVersion::embedded` | `emulated` | Electron's Chromium version is not a compile-time constant. Read it from the bridge: the Electron main process reports `process.versions.chrome` (the handshake already reports `params.electron`; add `chrome`). Fallback: the `electron-rs` prepared distribution metadata (`electron-rs.json`), which pins the Electron version, mapped to its Chromium version. |
| `Outcome` | `direct` | Pure data. |
| `prepare_root_cache_path` | `emulated` | All the filesystem logic ports unchanged. Two adaptations: (a) the "root cache path" is Electron's user-data directory (`app.getPath('userData')`, or the `--user-data-dir` the runtime passes), so the call must run **before the Electron child is spawned** — that is the only window in which the directory can be renamed; (b) `singleton_holder` keeps working on POSIX because Electron uses the same Chromium `ProcessSingleton` (`SingletonLock`/`SingletonSocket`/`SingletonCookie`), but the check is only meaningful if a *previous* Electron instance left the lock, which is exactly the case it is for. |

Notes for the implementer:

- The `cef::sys::CHROME_VERSION_*` constants disappear; do not hardcode a version. A wrong version
  here silently disables downgrade detection.
- Keep the `Last Version` file name and format byte-identical, so a profile migrated from the CEF
  runtime to the Electron runtime is recognised rather than treated as `Unknown`.
- `DowngradePolicy` itself lives in `runtime.rs` (reference `runtime.rs:440-460`) and is `[ext]`; its
  two variants and doc intent must be preserved verbatim apart from the `Cef::root_cache_path`
  reference.

---

## 3. `src/environment.rs` (202 lines)

### 3.1 Purpose

Owns the diagnostic environment variables Chromium and CEF read behind the application's back, and
the policy that decides whether they are honoured. A browser is expected to let whoever runs it
capture its own traffic and redirect its own crash reports; an application that embeds Chromium
inherits those hooks without asking, and in a shipped application they are not conveniences any more —
anyone who can set a variable in the application's environment can decrypt every TLS session it makes,
or point its minidumps (which carry process memory) at a server of their choosing. `DebugEnvironment`
is the switch. The two variable groups are answered in different places because neither is reachable
through a CEF setting: the TLS key log is answered on the command line, the crash-reporter overrides in
the environment.

### 3.2 Public items

`[ext]` **`DebugEnvironment`** — `environment.rs:33-48`

```rust
/// Whether Chromium and CEF may read their diagnostic environment variables.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DebugEnvironment {
  /// Honour them in development builds (`tauri::is_dev()`), refuse them in release
  /// builds.
  #[default]
  Auto,
  /// Always honour them.
  ///
  /// Appropriate for a build whose users are expected to debug it — an internal tool, a
  /// QA build — and wrong for one handed to the public.
  Allow,
  /// Always refuse them, in every build profile.
  Deny,
}
```

`[int]` **`neutralizes_tls_key_log`** — `environment.rs:105`

```rust
pub(crate) fn neutralizes_tls_key_log(policy: DebugEnvironment, is_dev: bool) -> bool
```

Caller: `runtime.rs:3285`. Answers whether the runtime should append an empty
`--ssl-key-log-file`, which is how `SSLKEYLOGFILE` is refused.

`[int]` **`remove_crash_reporter_overrides`** — `environment.rs:132`

```rust
pub(crate) fn remove_crash_reporter_overrides(policy: DebugEnvironment, is_dev: bool)
```

Caller: `runtime.rs:3127`, before the first CEF call. Documented `# Safety`: calls
`std::env::remove_var`, which is unsound while another thread reads or writes the environment, so the
caller must be the runtime's own initialization, on the main thread, before CEF exists and before the
runtime starts a thread of its own.

Private support: `SSL_KEY_LOG_FILE` (`:55`), `CRASH_REPORTER_VARIABLES` (`:68-81`),
`refuses_debug_variables` (`:87`), `is_set` (`:96`).

### 3.3 Behavioural rules

1. **Policy semantics** (`refuses_debug_variables`, `environment.rs:87-93`):

   | policy | refuses? |
   | --- | --- |
   | `Auto` | `!is_dev` |
   | `Allow` | never |
   | `Deny` | always |

   `is_dev` is `tauri::is_dev()` at both call sites. `Allow` short-circuits *before* any variable is
   read, which is asserted by a test that does not depend on the ambient environment.
2. **`is_set(name)`** = `std::env::var_os(name).is_some_and(|value| !value.is_empty())`. Set-but-empty
   counts as unset.
3. **TLS key log** (`neutralizes_tls_key_log`). Returns `false` when the policy does not refuse or the
   variable is not set — so the switch, and the warning Chromium logs for it, only appear when
   somebody was actually trying to log keys. Otherwise logs
   `"ignoring the SSLKEYLOGFILE environment variable: it asks Chromium to log the TLS session keys that decrypt this application's network traffic. Set DebugEnvironment::Allow to honour it."`
   and returns `true`. The caller then appends `--ssl-key-log-file` with an **empty** value
   (`runtime.rs:3286`), before the application's own switches so a deliberate
   `--ssl-key-log-file` still wins. Rationale: `content/browser/network_service_instance_impl.cc`
   consults `SSLKEYLOGFILE` only when `--ssl-key-log-file` is absent, and an empty switch value logs a
   warning and creates no key logger — a complete answer that needs no environment write.
4. **Crash-reporter overrides** (`remove_crash_reporter_overrides`). When the policy refuses, iterate
   `CRASH_REPORTER_VARIABLES`; for each variable that `is_set`, log
   `"ignoring the {name} environment variable: it asks CEF for {effect}. Set DebugEnvironment::Allow to honour it."`
   and `unsafe { std::env::remove_var(name) }`. The three variables and their effects:

   | variable | effect (goes into the warning) |
   | --- | --- |
   | `CEF_CRASH_REPORTER_SERVER_URL` | redirecting crash report uploads, which carry process memory |
   | `CEF_CRASH_REPORTER_RATE_LIMIT_ENABLED` | overriding the crash report upload rate limit |
   | `BREAKPAD_DUMP_LOCATION` | redirecting where minidumps are written |

   All three are only consulted when the application ships a `crash_reporter.cfg`, so removing them
   costs nothing when it does not. Must run before `ChromeMainDelegateCef::BasicStartupComplete`,
   which `cef::initialize` reaches; child processes inherit the browser process's environment, so a
   variable removed here is gone from the whole process tree.
5. **`SSLKEYLOGFILE` is deliberately not in `CRASH_REPORTER_VARIABLES`** — it is answered on the
   command line instead. A test (`environment.rs:194-201`) asserts the disjointness.
6. Tests (`:152-201`) pin: `Auto` follows the build profile; explicit policies ignore it; a
   permissive policy never touches the command line; every crash variable has a non-empty name and
   effect.

### 3.4 Electron feasibility

| item | tag | mechanism / reason |
| --- | --- | --- |
| `DebugEnvironment` | `direct` | Pure policy. Same three variants, same `Auto`/`Allow`/`Deny` semantics, same `#[default] Auto`. |
| `neutralizes_tls_key_log` | `direct` | Electron is Chromium: the same `--ssl-key-log-file` switch with an empty value suppresses the key logger. Deliver it on the spawned Electron command line (an extra argv element from the Rust side) or via `app.commandLine.appendSwitch('ssl-key-log-file', '')` in the main process. The variable is read by the Electron network service, so the rule is identical. |
| `remove_crash_reporter_overrides` | `emulated` | The mechanism (`std::env::remove_var` before the child is spawned, so the child inherits the sanitized environment) is the same. The **variable list must change**: `CEF_CRASH_REPORTER_SERVER_URL` and `CEF_CRASH_REPORTER_RATE_LIMIT_ENABLED` do not exist in Electron; `BREAKPAD_DUMP_LOCATION` still applies (Electron's crashpad handler reads it on Windows). Electron's own equivalents that must be refused instead: `ELECTRON_ENABLE_LOGGING`, `ELECTRON_ENABLE_STACK_DUMPING`, `ELECTRON_DEFAULT_ERROR_MODE`, `NODE_OPTIONS`, `ELECTRON_RUN_AS_NODE`. Keep the same warning shape, one line per variable naming the effect. |

Ordering constraints that carry over verbatim:

- Run both functions from the runtime's initialization, before the Electron child is spawned and
  before the IPC reader thread exists. The `unsafe` on `remove_var` is still required for soundness of
  the *Rust* process, and the safety comment must say so.
- `DEBUG`-style variables must not be re-read later: the Electron child sees the environment as it was
  at spawn time, so a removal after `launch()` has no effect.

---

## 4. `src/locale.rs` (233 lines)

### 4.1 Purpose

Derives the languages the user asked their operating system for, in the form Chromium wants. CEF
appends `--lang=en-US` whenever `CefSettings.locale` is empty, and with no `accept_language_list` of
its own the accept-language list falls back to that locale, so a CEF application sends
`Accept-Language: en-US,en` and reports `navigator.language === "en-US"` for every user on earth —
which silently breaks server-side localization and any page that branches on browser language.
`CefSettings.locale` cannot simply follow the system because it selects which `locales/*.pak`
Chromium loads its own strings from and the bundler ships only `en-US.pak`; `accept_language_list` has
no such constraint, so it is the one derived here. CEF then expands the list (`pt-BR` → `pt-BR,pt`
with quality values) through `net::HttpUtil::ExpandLanguageList`, so this module only has to produce
the ordered codes the user actually chose.

### 4.2 Public items

No `[ext]` items.

`[int]` **`system_accept_language_list`** — `locale.rs:27`

```rust
pub(crate) fn system_accept_language_list() -> Option<String>
```

Doc intent: the user's preferred languages as a comma-delimited list of BCP-47 codes, or `None` when
the system does not say. Feeds `CefSettings.accept_language_list`. Caller: `runtime.rs:3471`
(`accept_language_list.or_else(crate::locale::system_accept_language_list)` — an explicit
application value always wins).

Private support: `normalize_language_list` (`:42`), `normalize_language` (`:58`),
`preferred_languages` per platform (`:96` linux/BSD, `:124` macOS, `:134` windows, `:163` fallback).

### 4.3 Behavioural rules

1. **Output shape.** `normalize_language_list(preferred_languages())`, joined with `,`, or `None` when
   empty. No `q=` values, no expansion — CEF does that.
2. **`normalize_language(tag)`** (`locale.rs:58-86`):
   - `tag.split(['.', '@']).next()` — cut at the first `.` or `@` (`pt_BR.UTF-8@euro` → `pt_BR`),
     then `.trim()`, then `replace('_', "-")`.
   - Reject when empty, or case-insensitively equal to `C` or `POSIX` ("no preference"; asking a
     server for a language called `c` is worse than asking for nothing).
   - Split on `-`. The first part must be 2–8 chars, all ASCII alphabetic. Every following part must
     be non-empty, ≤ 8 chars, all ASCII alphanumeric. Otherwise `None`.
   - Consequences pinned by tests: `en-` and `en-US-` are rejected (empty trailing part); `1234` is
     rejected (first part not alphabetic); `en US` is rejected (space is not a separator, so the
     single part fails the alphanumeric check); `pt-BR`, `zh-Hans-CN`, `en` pass unchanged.
3. **`normalize_language_list`** drops `None`s and de-duplicates **keeping first position**, because
   the order is the user's preference order (`["pt_BR.UTF-8", "pt-BR", "en_US.UTF-8"]` →
   `["pt-BR", "en-US"]`).
4. **Linux / DragonFly / FreeBSD / NetBSD / OpenBSD** (`:96-120`):
   - `LANGUAGE` first — the only variable that carries a *list* (colon-separated), and gettext gives
     it priority over the `LC_*` variables for exactly this question. It is used only when at least
     one of its entries normalizes to something non-empty (the normalization is a probe), and when it
     is used the **raw, un-normalized** list is returned — `normalize_language_list` at the top level
     then cleans it.
   - Otherwise the first non-empty of `LC_ALL`, `LC_MESSAGES`, `LANG`, as a one-element vector.
   - Otherwise empty.
5. **macOS** (`:124-130`): `objc2_foundation::NSLocale::preferredLanguages()` in order — already
   BCP-47, already in the order set in System Settings.
6. **Windows** (`:134-152`): `GetUserDefaultLocaleName` into `[u16; 85]` (`LOCALE_NAME_MAX_LENGTH`);
   `written <= 0` → empty; the count includes the terminating NUL, so take `written - 1`; lossy UTF-16
   to `String`; empty name → empty vector, else a one-element vector.
7. **Any other target** (`:163-165`): empty vector.
8. Tests (`:167-232`) pin the POSIX-name conversion, BCP-47 passthrough, placeholder-locale dropping,
   order preservation with de-duplication, malformed-tag rejection, and that "nothing reported"
   produces no list.

### 4.4 Electron feasibility

| item | tag | mechanism / reason |
| --- | --- | --- |
| `system_accept_language_list` | `emulated` | Electron has no `accept_language_list` setting. The OS derivation is pure Rust and ports unchanged. Deliver the list as Chromium's `--accept-lang=<list>` switch — documented in Chromium as *"Specifies Accept-Language to send to servers and expose to JavaScript via the `navigator.language` DOM property"* — either as an argv element on the spawned Electron command line or through `app.commandLine.appendSwitch('accept-lang', list)` in the main process. This is the closest analogue of CEF's `accept_language_list` and it also drives `navigator.language`. Fallback if the pinned Electron version does not honour it end-to-end: rewrite the header with `session.webRequest.onBeforeSendHeaders`. |
| `normalize_language`, `normalize_language_list` | `direct` | Pure string logic; the validation rules (2–8 alphabetic, ≤ 8 alphanumeric parts, `C`/`POSIX` dropped) are the same requirement on any Chromium. |
| per-OS `preferred_languages` | `direct` | Pure OS API calls (`NSLocale::preferredLanguages`, `GetUserDefaultLocaleName`, `LANGUAGE`/`LC_*`). Keep the exact per-OS precedence. |

Caveats worth encoding in the module docs:

- Do **not** set `--lang` from this list. `--lang` selects the locale `.pak` bundle (the constraint
  that made CEF's `locale` unusable) and, if set to a single value, also determines
  `navigator.language`. `--accept-lang` is the list-shaped switch.
- Electron's main process can report `app.getPreferredSystemLanguages()` / `app.getLocale()`; the Rust
  derivation must agree with them, and a divergence is a bug in one of the two. Prefer the Rust
  derivation as the single source of truth and pass it down, rather than reading Electron's answer
  back.
- The `objc2-foundation` `NSLocale` feature and the `windows` `Win32_Globalization` feature are the
  only platform dependencies this module needs; both are already in the target's `Cargo.toml`.

---

## 5. `src/switches.rs` (268 lines)

### 5.1 Purpose

Helpers for the Chromium command line the runtime hands to CEF, covering two problems. First, a
switch value **replaces** rather than accumulates: Chromium's `CommandLine::AppendSwitchNative` stores
switches in a map keyed by name, so appending an existing switch overwrites the previous value and
nothing in Chrome, CEF or CEF's patches installs a `DuplicateSwitchHandler`. For most switches that is
what an embedder wants (the application's value wins over the runtime's default), but for the
comma-delimited ones it is a trap: CEF fills `--disable-features` before
`on_before_command_line_processing` with a list that keeps Chrome from crashing at startup and keeps
renderers from being killed on CEF's own requests, and an application appending its own
`--disable-features` silently drops all of it. `append_merged_switch` is the merging append those
switches need, and `REPLACING_SWITCHES` is the list an application is warned about. Second, some
switches turn security off: Chrome keeps a list of flags that "stability and security will suffer"
from and shows an infobar naming them, and since a shipped Tauri application ignores its process
command line entirely the only way one of those flags reaches Chromium is the application putting it
there — so `warn_about_dangerous_switches` says so out loud.

### 5.2 Public items

No `[ext]` items.

`[int]` **`REPLACING_SWITCHES`** — `switches.rs:37-41`

```rust
pub(crate) const REPLACING_SWITCHES: &[(&str, &str)] = &[
  ("disable-features", "Cef::disable_features"),
  ("enable-features", "Cef::enable_features"),
  ("js-flags", "Cef::javascript_flags"),
];
```

Doc intent: switches whose value Chromium or CEF has already set by the time the application's own
switches are appended, and which therefore replace rather than extend. Each entry names the API that
sets the same thing without dropping what is there. Used at `switches.rs:154-164` and in tests.

`[int]` **`append_merged_switch`** — `switches.rs:119-123`

```rust
pub(crate) fn append_merged_switch(
  command_line: &mut cef::CommandLine,
  name: &str,
  values: &[String],
)
```

Callers: `runtime.rs:2677` (`disable-features`), `runtime.rs:2682` (`enable-features`), both from
`TauriCefApp::on_before_command_line_processing` for the browser process only, *after* the
application's own switches.

`[int]` **`warn_about_replacing_switches`** — `switches.rs:154`

```rust
pub(crate) fn warn_about_replacing_switches(args: &[(String, Option<String>)])
```

Caller: `runtime.rs:3133`.

`[int]` **`warn_about_dangerous_switches`** — `switches.rs:167`

```rust
pub(crate) fn warn_about_dangerous_switches(args: &[(String, Option<String>)])
```

Caller: `runtime.rs:3132`.

Private support: `DANGEROUS_SWITCHES` (`:49-85`), `split_list` (`:88`), `merged_switch_value`
(`:102`), `switch_key` (`:149`).

### 5.3 Behavioural rules

1. **`DANGEROUS_SWITCHES` — 31 names**, mirroring `chrome/browser/ui/startup/bad_flags_prompt.cc`,
   grouped by comment:
   - *Web platform boundaries (12):* `disable-web-security`, `allow-running-insecure-content`,
     `ignore-certificate-errors`, `ignore-certificate-errors-spki-list`,
     `unsafely-treat-insecure-origin-as-secure`, `allow-insecure-localhost`,
     `disable-site-isolation-trials`, `enable-blink-features`, `disable-blink-features`,
     `enable-unsafe-webgpu`, `disable-hid-blocklist`,
     `unsafely-allow-protected-media-identifier-for-domain`.
   - *Process sandbox (10):* `no-sandbox`, `disable-gpu-sandbox`, `disable-setuid-sandbox`,
     `disable-seccomp-filter-sandbox`, `disable-namespace-sandbox`, `disable-landlock-sandbox`,
     `disable-webnn-compiler-sandbox`, `allow-sandbox-debugging`, `allow-third-party-modules`,
     `single-process`.
   - *Traffic capture and redirection (5):* `host-resolver-rules`, `host-rules`, `ssl-key-log-file`,
     `log-net-log`, `net-log-capture-mode`.
   - *Media and input (4):* `disable-webrtc-encryption`, `use-fake-ui-for-media-stream`,
     `enable-speech-dispatcher`, `enable-gpu-benchmarking`.

   Advisory only: the runtime still appends whatever the application asked for. A name a future
   Chromium renames simply stops matching, which costs a warning and nothing else.
2. **`split_list(value)`** — `split(',')`, `trim`, drop empty parts. `"A,,B , C,"` → `["A","B","C"]`.
3. **`merged_switch_value(existing, values) -> Option<String>`** — start from `split_list(existing)`;
   for every value, iterate its own `split_list` and push each entry not already present (so
   `["A,B","C"]` and `["A","B","C"]` give the same result); return `None` when the merged list is
   empty, so the caller leaves the command line alone rather than writing `--disable-features=` (which
   reads as "disable nothing named" and is *not* the same as an absent switch).
4. **`append_merged_switch`** — `return` when `values.is_empty()`; read the existing value when
   `has_switch == 1`; compute the merged value; `None` → `return`; otherwise
   `remove_switch(name)` **then** `append_switch_with_value(name, merged)`. The remove is not
   redundant: `append_switch_with_value` overwrites the map entry but leaves the old spelling in
   `argv`, so removing first keeps the two consistent for anything that re-parses it. Chromium reads
   the *last* value appended for a switch and there is no in-place edit API, so the merge is
   read → remove → append.
5. **`switch_key(argument)`** — `trim_start_matches('-')`, so `--disable-features`, `-disable-features`
   and `disable-features` are all recognised.
6. **`warn_about_replacing_switches`** — one `log::warn!` per matching argument, naming the switch and
   the replacement API: *"the --{key} switch replaces the value Chromium and CEF already set rather
   than adding to it, which drops entries the runtime depends on. Use {replacement} instead."*
7. **`warn_about_dangerous_switches`** — one `log::warn!` per matching argument: *"the --{key} switch
   turns off a Chromium security boundary. Chrome itself warns its users when it is set; do not ship
   it."*
8. **The two lists answer different questions.** `disable-features` replaces CEF's crash-avoidance
   list — a correctness problem, not a security one — so it is in `REPLACING_SWITCHES` only. The blink
   feature switches turn boundaries off but overwrite nothing the runtime set, so they are in
   `DANGEROUS_SWITCHES` only. A test asserts both directions.
9. Tests (`:179-267`) pin: Chromium's list splitting; merging keeps what CEF already disabled
   (`"GlicActorUi,LensOverlay"` + `["MediaRouter"]` → `"GlicActorUi,LensOverlay,MediaRouter"`);
   merging onto an unset switch writes only the new values; a value already present is not repeated;
   entries may themselves be comma-delimited; nothing-to-write leaves the command line alone; switch
   names are recognised with or without a prefix; every replacing switch names a replacement.

### 5.4 Electron feasibility

| item | tag | mechanism / reason |
| --- | --- | --- |
| `REPLACING_SWITCHES` | `direct` (with renamed replacement names) | The three switch names are Chromium's and are identical under Electron. Only the second column changes: `Cef::disable_features` → the Electron equivalent (`Electron::disable_features`, or the instruction "merge it into the existing value"). Keep the tuple shape and the "each entry names the API that sets the same thing" contract. |
| `append_merged_switch` | `emulated` | The merge algorithm is identical, but there is no live `cef::CommandLine` object. Electron's command line is a `Vec<String>`/`Vec<(String, Option<String>)>` the runtime builds and passes as argv to the spawned Electron process (or replays through `app.commandLine.appendSwitch`). Chromium's switch map is the same code, so both the replace-not-accumulate rule and the last-appended-wins rule still hold; only the read/remove/append primitives become vector operations. Keep the `None`-means-don't-write rule — an empty `--disable-features=` is still wrong. |
| `warn_about_replacing_switches` | `direct` | Pure logging over the application's switch list; identical signature shape (`&[(String, Option<String>)]`). |
| `warn_about_dangerous_switches` | `direct` | The list is Chromium's and Electron runs the same Chromium, so all 31 names port verbatim. |
| `DANGEROUS_SWITCHES` | `direct` | As above. Optionally add Electron-specific entries for the Node side (`--inspect`, `--inspect-brk`, `--remote-debugging-port`), which open a debugger that can execute arbitrary code in the main process. |

The target currently has a divergent `src/switches.rs` (a `&[&str]` `REPLACING_SWITCHES`, a
`--name=value` string-based `append_merged_switch`, and a 5-entry `DANGEROUS_SWITCHES`). It must be
rewritten to the reference's shape: the tuple list, the `values: &[String]` merging signature, the
`merged_switch_value`/`split_list` helpers, and the full 31-name dangerous list.

---

## 6. `src/devtools.rs` (127 lines)

### 6.1 Purpose

Allocates the process-local DevTools-protocol request identifiers, in two disjoint ranges: one for
callers of the crate, one reserved for the runtime itself. Observers see the whole browser, so every
`BrowserHost` message a caller sends must use the shared allocator or it will consume another caller's
result; and the runtime's own requests must come from a range no caller can reach, so internal
correlation (`pending_initial_loads`, the script-evaluation callbacks) only ever matches requests the
runtime sent — even when a caller ignores the allocator and hardcodes an `id`. Each range is bounded and
fails closed at its own boundary, so neither can ever grow into the other no matter how many
identifiers are allocated.

### 6.2 Public items

`[ext]` **`DevToolsMessageIdExhausted`** — `devtools.rs:19-29`

```rust
/// The process-local native DevTools request identifier space is exhausted.
#[derive(Clone, Copy, Debug)]
pub struct DevToolsMessageIdExhausted;

impl std::fmt::Display for DevToolsMessageIdExhausted {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    formatter.write_str("native CEF DevTools message identifiers are exhausted")
  }
}

impl std::error::Error for DevToolsMessageIdExhausted {}
```

`[ext]` **`allocate_devtools_message_id`** — `devtools.rs:31-42`

```rust
/// Allocates one native DevTools request ID for a caller of this crate.
///
/// Observers see the whole browser, so every BrowserHost message a caller sends
/// must use this allocator to avoid consuming another caller's result. The
/// runtime allocates its own requests from a reserved range this function never
/// returns, so a caller cannot answer an internal request by picking its number.
///
/// IDs are positive and never reused, including after cancellation or browser
/// teardown. Numeric correlation does not authorize a browser or document.
pub fn allocate_devtools_message_id() -> Result<i32, DevToolsMessageIdExhausted>
```

`[int]` **`allocate_runtime_devtools_message_id`** — `devtools.rs:44-52`

```rust
pub(crate) fn allocate_runtime_devtools_message_id() -> Result<i32, DevToolsMessageIdExhausted>
```

Doc intent: the identifiers come from the range reserved above every caller-visible one, so internal
correlation only ever matches requests the runtime sent, even when a caller hardcodes an `id`.
Callers: `webview.rs:1235, 2139, 2294, 2320, 2389`.

Private support: `RESERVED_RANGE_START` (`:14`), `NEXT_MESSAGE_ID` (`:16`),
`NEXT_RESERVED_MESSAGE_ID` (`:17`), `allocate_from` (`:57`).

### 6.3 Behavioural rules

1. **Ranges.** `RESERVED_RANGE_START: i32 = 1_000_000_000`. Callers allocate `1 ..= RESERVED_RANGE_START - 1`
   (i.e. `1 ..= 999_999_999`); the runtime allocates `RESERVED_RANGE_START ..= i32::MAX`. CDP identifiers
   are signed 32-bit, so both ranges stay inside what the DevTools agent accepts.
2. **Two independent process-global counters.** `NEXT_MESSAGE_ID` starts at `1`;
   `NEXT_RESERVED_MESSAGE_ID` starts at `RESERVED_RANGE_START`.
3. **`allocate_from(counter, last)`** — `counter.try_update(Relaxed, Relaxed, |value| if value > last { None } else { value.checked_add(1) })`
   mapped to `Err(DevToolsMessageIdExhausted)`. Returns the pre-increment value. `try_update` (not
   `fetch_add`) is what makes exhaustion detectable and concurrent producers non-overlapping.
4. **Exhaustion behaviour.** At the boundary the last successful allocation returns `last`; every
   later call returns `Err`, and the counter is left at `last + 1` and does not move again. Pinned by
   tests: `allocate_from(&AtomicI32::new(RESERVED_RANGE_START - 1), RESERVED_RANGE_START - 1)` returns
   `RESERVED_RANGE_START - 1` then errors twice, leaving the counter at `RESERVED_RANGE_START`; the
   same at `i32::MAX - 1` / `i32::MAX`. There is no wrap and no reuse.
5. **Disjointness.** A test allocates 1 000 caller ids and 1 000 runtime ids and asserts caller ids are
   `> 0 && < RESERVED_RANGE_START`, runtime ids are `>= RESERVED_RANGE_START`, and the sets are
   disjoint. Another asserts four concurrent threads × 1 000 allocations produce 4 000 distinct positive
   ids.
6. **Non-goals.** Numeric correlation authorizes nothing: an id matching a request does not authorize a
   browser or a document.

### 6.4 Electron feasibility

| item | tag | mechanism / reason |
| --- | --- | --- |
| `DevToolsMessageIdExhausted` | `direct` | Pure type. Change the `Display` string to name Electron rather than CEF. |
| `allocate_devtools_message_id` | `emulated` | Electron owns CDP numbering: `webContents.debugger.sendCommand` allocates and correlates its own ids, and `webContents.debugger.on('message', …)` delivers the response without exposing the id. So the runtime's own traffic needs no allocator. The function must still exist, return a unique positive `i32`, and keep the two-range scheme, because a caller that speaks CDP over `webContents.debugger` (or over `--remote-debugging-port`) still needs ids that cannot collide with the runtime's. Document that the id is meaningful only to such a caller. |
| `allocate_runtime_devtools_message_id` | `emulated` | Same; the reserved range stays reserved so the two ranges remain disjoint, even though the runtime's own `webContents.debugger` traffic numbers itself. |

Additional Electron constraint to document: **one CDP session per `WebContents`**. `debugger.attach()`
rejects while DevTools is open, and a second consumer is rejected. Any caller-facing CDP surface has to
share a single attachment (a lease), not attach per caller — which is exactly the "observers see the
whole browser" problem the allocator solves on CEF, in a different form.

---

## 7. `src/sandbox.rs` (539 lines)

### 7.1 Purpose

Decides whether Chromium's process sandbox has to be turned off. On every platform the answer normally
comes straight from `SandboxPolicy` and the answer is "keep it"; only Linux and the BSDs have a case
where keeping it means the application cannot start at all, and detecting that case is what the bulk of
the module is for. Chromium's zygote host picks the namespace sandbox when unprivileged user namespaces
work, else the root-owned setuid `chrome-sandbox` helper next to the executable (or at
`CHROME_DEVEL_SANDBOX`); when neither is available it calls `LOG(FATAL) << "No usable sandbox!"` and the
process dies before a window is shown. Tauri's deb/rpm bundlers install `chrome-sandbox` with mode 4755,
so packaged applications always have the helper; an AppImage cannot, because its payload is mounted
`nosuid`, which leaves AppImages relying on unprivileged user namespaces — exactly the combination
Ubuntu 23.10+ restricts through AppArmor. On Windows the module documents that the sandbox is currently
unavailable whatever the policy says, and reports that gap rather than promising something the platform
cannot keep. On macOS there is nothing to probe.

### 7.2 Public items

No `[ext]` items. `SandboxPolicy` itself is `[ext]` but lives in `runtime.rs` (`:162-190`).

`[int]` **`windows_sandbox_unavailable`** — `sandbox.rs:62-68`

```rust
pub(crate) const fn windows_sandbox_unavailable() -> bool {
  cfg!(windows)
}
```

Doc intent: whether this platform can actually sandbox, given how the runtime initializes CEF. Kept as
a function of a `cfg` rather than a `cfg` at every use site so the decision table stays testable on one
platform. Used at `sandbox.rs:222, 243` and by a test asserting it equals `cfg!(windows)`.

`[int]` **`SandboxDisableReason`** — `sandbox.rs:70-105`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SandboxDisableReason {
  /// The application asked for it through [`SandboxPolicy::Disabled`].
  Policy,
  /// AppImage, no setuid helper, and AppArmor restricts unprivileged user namespaces.
  AppImageUserNamespacesRestricted,
  /// AppImage, no setuid helper, and user namespaces are unavailable altogether.
  AppImageUserNamespacesUnavailable,
  /// Windows, where this runtime cannot supply CEF with a sandbox broker.
  WindowsBrokerUnavailable,
}

impl SandboxDisableReason {
  /// Message logged when the sandbox is dropped for this reason.
  pub(crate) fn message(self) -> &'static str
}
```

The four message strings, verbatim:

- `Policy` → `"the application set SandboxPolicy::Disabled"`
- `AppImageUserNamespacesRestricted` → `"running from an AppImage, which cannot ship the setuid chrome-sandbox helper, and unprivileged user namespaces are restricted (/proc/sys/kernel/apparmor_restrict_unprivileged_userns is 1)"`
- `AppImageUserNamespacesUnavailable` → `"running from an AppImage, which cannot ship the setuid chrome-sandbox helper, and unprivileged user namespaces are unavailable (/proc/sys/user/max_user_namespaces is 0)"`
- `WindowsBrokerUnavailable` → `"the Windows sandbox needs a broker this runtime cannot supply: CEF requires the application to be hosted by its bootstrap executable as a DLL, and a Tauri application is built as an executable"`

`[int]` **`SandboxDecision`** — `sandbox.rs:107-117`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SandboxDecision {
  /// Leave the sandbox alone.
  Keep,
  /// Append `--no-sandbox`, logging `reason`.
  Disable(SandboxDisableReason),
  /// [`SandboxPolicy::Required`] asked for a sandbox this platform cannot provide, so
  /// startup fails rather than silently running without one.
  Refuse(SandboxDisableReason),
}
```

`[int]` **`sandbox_decision`** — `sandbox.rs:131-138`

```rust
pub(crate) fn sandbox_decision(
  policy: SandboxPolicy,
  windows_broker_unavailable: bool,
  running_from_appimage: bool,
  sandbox_helper_available: bool,
  apparmor_restrict_unprivileged_userns: Option<u64>,
  max_user_namespaces: Option<u64>,
) -> SandboxDecision
```

Kept free of I/O so every combination is unit-testable. `None` for a sysctl means "could not be read",
which is treated as **no evidence of a restriction**, not as a restriction.

`[int]` **`resolve_sandbox_decision`** — two `cfg` variants, `sandbox.rs:218` (Linux/BSD) and
`sandbox.rs:240` (everything else)

```rust
pub(crate) fn resolve_sandbox_decision(policy: SandboxPolicy) -> SandboxDecision
```

Caller: `runtime.rs:3183`.

`[int]` **`launched_without_sandbox`** — `sandbox.rs:182-185`, `#[cfg(target_os = "macos")]`

```rust
pub(crate) fn launched_without_sandbox() -> bool
```

A child process inherits `--no-sandbox` from the browser process, so this is how a macOS helper learns
that entering the sandbox would be wrong. Read off the real command line rather than off
`SandboxPolicy` because a helper never sees the `Cef` builder. Callers: `runtime.rs:2748`, `runtime.rs:3031`.

`[int]` **`helper_stat_is_usable`** — `sandbox.rs:201`, `#[cfg(linux/BSD)]`

```rust
pub(crate) fn helper_stat_is_usable(uid: u32, mode: u32) -> bool
```

Private support: `running_from_appimage` (`:259`), `sandbox_helper_available` (`:279`),
`helper_path_is_usable` (`:302`), `read_sysctl` (`:327`), all `#[cfg(linux/BSD)]`.

### 7.3 Behavioural rules — the decision table

`sandbox_decision` evaluates in this order (`sandbox.rs:139-173`):

1. `policy == Disabled` → `Disable(Policy)`. Answered **before** the platform is consulted, so an
   application that asked for no sandbox is told what it asked for, not what the platform could not
   give it.
2. `windows_broker_unavailable` → `Required` gives `Refuse(WindowsBrokerUnavailable)`; anything else
   gives `Disable(WindowsBrokerUnavailable)`. Never `Keep`: CEF flips `no_sandbox` itself when it gets
   a null broker, so `Keep` would be a decision the platform overrules a moment later, and the rule
   that a lost sandbox is always named out loud would be broken.
3. `Required` → `Keep` (even where Chromium will abort — `Required` must keep the sandbox).
4. `Auto`:
   - `!running_from_appimage || sandbox_helper_available` → `Keep`. Everything but an AppImage can ship
     the setuid helper, so a missing sandbox there is a packaging or system problem that must not be
     papered over.
   - else `apparmor_restrict_unprivileged_userns == Some(1)` → `Disable(AppImageUserNamespacesRestricted)`
   - else `max_user_namespaces == Some(0)` → `Disable(AppImageUserNamespacesUnavailable)`
   - else `Keep` (unreadable sysctls are not evidence of a restriction; let Chromium have the last word).
   - When both sysctls point the same way, the AppArmor one is reported — it is the actionable message.

`resolve_sandbox_decision` (Linux/BSD) gathers:

- `running_from_appimage()` = `std::env::var_os("APPIMAGE").is_some_and(|path| !path.is_empty())`.
- `sandbox_helper_available(running_from_appimage)`:
  1. `CHROME_DEVEL_SANDBOX` (non-empty) wins on every layout — somebody deliberately pointing at a
     helper outside the application — but it is stat'ed like any other candidate.
  2. Running from an AppImage → `false`. The bundler always copies `chrome-sandbox` into an AppImage and
     the AppImage runtime mounts the payload `nosuid`, so the setuid bit `stat` still reports has no
     effect; finding a file by that name proves nothing.
  3. Else `<current_exe's dir>/chrome-sandbox`, stat'ed.
- `read_sysctl("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")` and
  `read_sysctl("/proc/sys/user/max_user_namespaces")` — `read_to_string(...).ok()?.trim().parse().ok()`.
- `helper_path_is_usable(path)` — `fs::metadata`, then `helper_stat_is_usable(metadata.uid(), metadata.mode())`;
  a rejected path logs at `debug` that it is "not a root-owned setuid binary executable by others".

`resolve_sandbox_decision` (everything else) passes `windows_sandbox_unavailable(), false, false, None, None`
— the policy alone decides.

`helper_stat_is_usable(uid, mode)` = `uid == 0 && mode & 0o4000 != 0 && mode & 0o0001 != 0`
(root-owned, `S_ISUID`, `S_IXOTH`). This mirrors `ZygoteHostImpl::Init`, which aborts with "The SUID
sandbox helper binary was found, but is not configured correctly" — so "present but wrong" must read as
**unavailable**. Pinned by tests: mode `0o104755` with uid 0 is usable; uid 1000, missing setuid
(`0o100755`), missing other-execute (`0o104750`), and `0o100644` are not.

`launched_without_sandbox()` = `std::env::args().any(|arg| arg == "--no-sandbox")`.

**Caller contract** (`runtime.rs:3182-3207`):

- `Keep` → no `--no-sandbox`.
- `Disable(reason)` → `log::warn!("running Chromium without a sandbox: {}. A compromised renderer process runs with the full privileges of the current user.", reason.message())`
  and append `--no-sandbox`.
- `Refuse(reason)` → `log::error!("refusing to start: SandboxPolicy::Required asked for a Chromium sandbox, but {}.", reason.message())`
  and return `Err(Error::CreateWebview("SandboxPolicy::Required cannot be honored: {message}"))`.

### 7.4 Electron feasibility

| item | tag | mechanism / reason |
| --- | --- | --- |
| `windows_sandbox_unavailable` | `absent` as a concept | Electron's Windows renderer sandbox does not need an embedder-supplied broker, so the function must return `false` on every platform. Keeping the function with a `false` body preserves the decision table and removes the Windows special case. |
| `SandboxDisableReason` | `emulated` | `Policy` ports verbatim. `WindowsBrokerUnavailable` becomes unreachable (keep the variant for shape parity, mark it dead). The two AppImage variants port only if the Electron runtime is itself shipped as an AppImage: the detection (`APPIMAGE`, `CHROME_DEVEL_SANDBOX`, the two sysctls) is pure Rust and unchanged, but Electron's `chrome-sandbox` lives inside the Electron distribution, not next to the app binary, so the helper lookup must be re-pointed at the Electron binary's directory. |
| `SandboxDecision` | `direct` | Pure data. |
| `sandbox_decision` | `emulated` | Same table, with `windows_broker_unavailable` forced `false`. |
| `resolve_sandbox_decision` | `emulated` | Same probing, plus the helper-path change above. |
| `helper_stat_is_usable` | `direct` | Pure `stat` arithmetic; identical conditions. |
| `launched_without_sandbox` | `absent` | There is no Electron helper process the Rust runtime sandboxes. Electron's own zygote/helper processes are launched and sandboxed by Electron; the function has no caller. Keep it (macOS `cfg`) returning `false` for shape parity, and document that the runtime does not sandbox anything itself. |

**What Electron cannot do at all** (must be stated in the module docs):

- **CEF's `SandboxPolicy` granularity is not reproducible.** CEF configures the sandbox per process type
  (browser, GPU, utility, zygote, renderer) and per platform, through a broker pointer plus settings.
  Electron exposes exactly two knobs: the process-wide `--no-sandbox` switch and the per-webview
  `webPreferences.sandbox` boolean (default `true` since Electron 20). There is no per-process-type
  control and no broker.
- **The Electron main process (Node) cannot be sandboxed at all.** It runs with the user's full
  privileges by design; only renderers and utility processes are sandboxed.
- **`Required` becomes *more* honourable, not less.** Because the Windows broker problem does not exist,
  `SandboxPolicy::Required` can succeed on Windows under Electron where it always fails under CEF. That
  is a behaviour improvement, not a parity break, but it must be documented: the reference's
  `WindowsBrokerUnavailable` arm is dead code in the target.

---

## 8. `src/macros.rs` (84 lines)

### 8.1 Purpose

A single `macro_rules!` that forwards a declaration to one of `cef`'s `wrap_*!` macros and adds a
named-args constructor next to the positional `new()` they generate. `new()` takes one argument per
struct field, so the wrappers carrying a lot of state — `TauriCefBrowserClient` most of all — are built
from a long positional list in which two same-typed neighbours (`devtools_enabled` and
`drag_drop_handler_enabled`, say) can be swapped without the compiler noticing. `build()` takes a single
args struct instead, so every value is named at the call site, and the forwarding to `new()` is
generated from the very field list the wrapper was declared with.

### 8.2 Public items

No `[ext]` items.

`[int]` **`wrap_with_args`** — `macros.rs:40-82`, exported at `macros.rs:84`

```rust
pub(crate) use wrap_with_args;
```

Accepted syntax (from the doc comment, `macros.rs:16-34`):

```rust
wrap_with_args! {
  wrap_client => TauriCefBrowserClientArgs;

  pub(crate) struct TauriCefBrowserClient<T: UserEvent> {
    pub(crate) context: RuntimeContext<T>,
    devtools_enabled: bool,
  }

  impl Client {
    // ... same method bodies as a plain `wrap_client!` block
  }
}

TauriCefBrowserClient::build(TauriCefBrowserClientArgs { context, devtools_enabled: true });
```

Callers: `cef_impl/client/mod.rs:77` (`wrap_client => TauriCefBrowserClientArgs`),
`client/command.rs:205`, `client/display.rs:19`, `client/drag.rs:212`, `client/life_span.rs:80`,
`request_handler.rs:168` and `:311`, `runtime.rs:2593` (`wrap_app => TauriCefAppArgs`).

### 8.3 Behavioural rules

The expansion has three parts, in this order:

1. **The args struct.** `$vis struct $args<generics> where … { $(pub(crate) $field_name: $field_type,)* }`.
   Every field is `pub(crate)` **whatever visibility the wrapper gave its own field**: anything that can
   name the args struct has to be able to fill in every field, while the wrapper keeps its own fields as
   private as it declared them. The where-clause is repeated on the args struct.
2. **The forwarded declaration.** The original `$vis struct $name<generics> { fields }` plus
   `impl $interface { $($methods)* }`, forwarded verbatim into `$wrap! { … }`.
3. **The named constructor.** `impl<generics> $name<generics> where … { $vis fn build(args: $args<generics>) -> $interface { Self::new($(args.$field_name),*) } }`
   — the forwarding argument list is generated from the same field list the struct was declared with, so
   it cannot drift.

Generic handling: `$generic_type: $first_generic_type_bound $(+ $generic_type_bound)*`, comma-separated,
trailing comma allowed; the bounds are re-emitted in the args struct's where-clause and in the `build`
impl. `$interface` is the cef wrapper trait name (`Client`, `App`, `RequestHandler`, `ResourceHandler`,
`CommandHandler`, `DisplayHandler`, `ResourceRequestHandler`, `LifeSpanHandler`).

### 8.4 Electron feasibility

| item | tag | mechanism / reason |
| --- | --- | --- |
| `wrap_with_args!` (args struct + `build()`) | `direct` | The args-struct + named-`build()` half is pure macro work and is worth keeping: the target's handler structs also carry many same-typed fields. |
| `wrap_with_args!` (the `$wrap!` forwarding arm) | `absent` | There is no `cef::wrap_*!` to forward to. Electron has no generated positional `new()` and no C-ABI wrapper types. |

Required adaptation: keep the file, the macro name and the `pub(crate) use wrap_with_args;` export so the
layout matches, but replace the forwarding arm. Two acceptable shapes:

- `$wrap` names an identity macro the crate defines (`macro_rules! wrap_client { … }` expanding to
  nothing), so call sites keep their exact spelling; or
- the macro emits the struct and the `build()` impl directly and `$wrap` becomes documentation-only.

Either way the observable contract must hold: **every field of the args struct is `pub(crate)`, the
wrapper's own field visibilities are unchanged, and `build()` forwards positionally in declaration
order.** Do not reorder fields.

---

## 9. `src/external_message_pump/`

### 9.1 `mod.rs` (199 lines)

#### Purpose

A self-contained port of cefclient's external message pump. With `cef::Settings::external_message_pump`
enabled, CEF does not run its own message loop; instead it asks the host to call
`cef::do_message_loop_work()` by invoking `OnScheduleMessagePumpWork(delay)` whenever it has work
pending. This module is the platform-independent scheduling and reentrancy logic; each platform supplies
a `PlatformPump` backend that drives a timer (Windows: a `WM_TIMER` on a message-only window; macOS: an
`NSTimer` in the common and event-tracking run-loop modes; Linux/BSD: a custom low-priority GLib source
with a wakeup pipe). On Windows and macOS the timer lives on the same native loop winit already runs, so
CEF keeps painting and processing IPC even while the OS spins a nested modal loop winit cannot observe
(window move/resize on Windows, menu and event tracking on macOS); on Linux/BSD the GLib source still
dispatches inside nested GLib loops (GTK menus/dialogs) for the same reason.

#### Public items

No `[ext]` items.

`[int]` **`CefExternalPump`** — `mod.rs:70-98`

```rust
/// Handle to the external message pump. Cloning shares the same underlying
/// state; the backing platform resources are released when the last clone drops.
#[derive(Clone)]
pub(crate) struct CefExternalPump {
  state: Arc<PumpState>,
}

impl CefExternalPump {
  pub(crate) fn new() -> Self
  /// Called from CEF's `OnScheduleMessagePumpWork`. May run on any thread.
  pub(crate) fn on_schedule_message_pump_work(&self, delay_ms: i64)
  /// Explicit tick, used to drive CEF before winit's loop is running (startup)
  /// and after winit processes a batch of events. Must run on the owner thread.
  pub(crate) fn do_work(&self)
}
```

Callers: `runtime.rs:1237` (field `pub(crate) cef_pump: CefExternalPump` on `RuntimeContext`),
`runtime.rs:2313` (`StartCause::Init`), `runtime.rs:3360` (construction), `runtime.rs:3549` (startup
wait loop), `runtime.rs:3710` (`run_iteration`, before `MainEventsCleared`);
`cef_impl/client/process.rs:31` (`BrowserProcessHandler::on_schedule_message_pump_work`).

Private support: `PumpState` (`:101-199`) with `on_schedule_message_pump_work`, `on_schedule_work`,
`on_timer_timeout`, `do_work`, `is_timer_pending`, `perform_message_loop_work`; constants
`K_TIMER_DELAY_PLACEHOLDER: i64 = i32::MAX as i64` (`:64`, intentionally 32-bit for Windows and macOS
platform API compatibility) and `K_MAX_TIMER_DELAY: i64 = 1000 / 30` (`:68`, 30 fps).

#### Behavioural rules

1. **Construction.** `Arc::new_cyclic` builds `PumpState { is_active: AtomicBool::new(false), reentrancy_detected: AtomicBool::new(false), platform: Mutex::new(PlatformPump::new(weak.clone())) }`,
   so the platform backend holds a `Weak<PumpState>` and there is no reference cycle.
2. **`on_schedule_message_pump_work(delay)` (any thread)** → `PumpState::on_schedule_message_pump_work`
   → lock the platform mutex and call `platform.on_schedule_message_pump_work(delay)`. Marshalling onto
   the owner thread is the platform backend's job; it lands back in `on_schedule_work`.
3. **`PumpState::on_schedule_work(delay)` (owner thread)** — mirrors cefclient's `OnScheduleWork`:
   - Under the lock: if `delay == K_TIMER_DELAY_PLACEHOLDER && platform.is_timer_pending()`, **return
     without touching the timer** (do not override the maximum timer requested from `do_work()` with a
     pending timer event). Otherwise `platform.kill_timer()`.
   - `delay <= 0` → `do_work()` immediately.
   - else clamp `delay` to `K_MAX_TIMER_DELAY` and `platform.set_timer(delay)`.
4. **`on_timer_timeout()`** — kill the timer, then `do_work()`.
5. **`do_work()`** — `was_reentrant = perform_message_loop_work()`; if re-entrant →
   `on_schedule_message_pump_work(0)` (execute the remaining work as soon as possible); else if no timer
   is pending → `on_schedule_message_pump_work(K_TIMER_DELAY_PLACEHOLDER)` (schedule at the maximum
   allowed time; may be dropped by `on_schedule_work` if another timer event is already in flight).
6. **`perform_message_loop_work()`** — if `is_active` (SeqCst) → set `reentrancy_detected = true` and
   return `false`; else clear the flag, set `is_active`, call `cef::do_message_loop_work()`, clear
   `is_active`, and return `reentrancy_detected` (which may have been set by re-entrant calls *during*
   the pump). The comment explains why: paint and IPC callbacks can call back into this method, and a
   discarded call must be reposted so it is not lost.
7. **`is_timer_pending()`** treats a poisoned mutex as pending (`unwrap_or(true)`), so a poisoned lock
   does not cause an unbounded immediate-work loop.

### 9.2 `windows.rs` (177 lines)

`PlatformPump { main_thread_target: HWND, timer_pending: bool }` with `unsafe impl Send` (the HWND is
created on and its timer armed/disarmed only from the main winit thread; the sole cross-thread use is
`PostMessageW`, which Win32 permits from any thread).

- `new(state: Weak<PumpState>)` — `GetModuleHandleW(None)`, `RegisterClassExW` for class
  `TauriCEFMainTargetHWND` with `wnd_proc`, then `CreateWindowExW(0, class, null, WS_OVERLAPPEDWINDOW, 0,0,0,0, Some(HWND_MESSAGE), …)`
  — a **message-only** window. Stores `Box::into_raw(Box::new(state))` in `GWLP_USERDATA`.
- `on_schedule_message_pump_work(delay)` — `PostMessageW(hwnd, K_MSG_HAVE_WORK /* WM_USER + 1 */, 0, delay as isize)`.
  May be called from any thread.
- `set_timer(delay)` — `debug_assert!(!timer_pending)`, `debug_assert!(delay > 0)`, `SetTimer(hwnd, 1, delay as u32, None)`,
  `timer_pending = true`.
- `kill_timer()` — `KillTimer` only when pending, then clear the flag.
- `is_timer_pending()` — the bool.
- `Drop` — kill the timer if pending, clear `GWLP_USERDATA` and drop the boxed `Weak`, `DestroyWindow`.
- `wnd_proc` — on `WM_TIMER` or `K_MSG_HAVE_WORK`: read `GWLP_USERDATA`, upgrade the `Weak`, and call
  `on_schedule_work(lparam as i64)` for `K_MSG_HAVE_WORK` or `on_timer_timeout()` for `WM_TIMER`; always
  `DefWindowProcW`.

### 9.3 `macos.rs` (134 lines)

`EventHandler` is an objc2 class (`define_class!`, `#[ivars = Weak<PumpState>]`) with
`#[unsafe(method(scheduleWork:))] fn handle_schedule_work(&self, delay_ms: &NSNumber)` →
`state.on_schedule_work(delay_ms.as_i64())`, and `#[unsafe(method(timerTimeout:))] fn handle_timer_timeout(&self, _: &NSTimer)`
→ `state.on_timer_timeout()`.

`PlatformPump { owner_thread: Retained<NSThread>, event_handler: Retained<EventHandler>, timer: Option<Retained<NSTimer>> }`
with `unsafe impl Send`.

- `new` captures `NSThread::currentThread()`.
- `on_schedule_message_pump_work(delay)` — box the delay as `NSNumber::new_i32(delay as i32)` and
  `performSelector_onThread_withObject_waitUntilDone(sel!(scheduleWork:), &owner_thread, Some(&delay), false)`.
- `set_timer(delay)` — `debug_assert!(delay > 0)`, `debug_assert!(timer.is_none())`; create a
  **non-repeating** `NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(delay as f64 / 1000.0, &event_handler, sel!(timerTimeout:), None, false)`
  and add it to **both** `NSRunLoopCommonModes` and `NSEventTrackingRunLoopMode` on
  `NSRunLoop::currentRunLoop()` — the tracking mode is what keeps the pump alive while AppKit spins a
  nested menu/tracking loop.
- `kill_timer()` — `invalidate()` on the taken timer.
- `is_timer_pending()` — `timer.is_some()`.

### 9.4 `linux.rs` (294 lines)

A custom `GSource` attached to the default GLib `MainContext`.

- `WorkSource { source: ffi::GSource, source_state: *mut SourceState }` (`#[repr(C)]`).
- `SourceState { state: Weak<PumpState>, delayed_work_time: Option<Instant>, wakeup_pipe_read: c_int, wakeup_pipe_write: c_int, wakeup_gpollfd: Box<ffi::GPollFD> }`
  — the `GPollFD` is boxed to keep its address stable while registered.
- `new(state)` — `libc::pipe`, `g_source_new(&WORK_SOURCE_FUNCS, size_of::<WorkSource>())`,
  `g_source_add_poll(source, &mut *wakeup_gpollfd)` (`events: G_IO_IN`),
  `g_source_set_priority(source, G_PRIORITY_DEFAULT_IDLE)` (low priority so other queued events go
  first), `g_source_set_can_recurse(source, GTRUE)` (needed to allow `Run` inside `Dispatch`),
  `g_source_attach(source, default_context)`.
- `on_schedule_message_pump_work(delay)` — write the 8-byte `delay_ms` into the wakeup pipe with
  `retry_eintr`; log `log::error!("could not write to the CEF message pump wakeup pipe")` on a short
  write. Deliberately touches no shared state, so it is safe from any thread and guarantees the GLib
  poll wakes up.
- `set_timer(delay)` — `delayed_work_time = Some(Instant::now() + Duration::from_millis(delay))`.
- `kill_timer()` — `delayed_work_time = None`.
- `is_timer_pending()` — `get_time_interval_milliseconds(delayed_work_time) > 0`.
- `Drop` — `g_source_destroy`, `g_source_unref`, close both pipe ends.
- `get_time_interval_milliseconds(Option<Instant>) -> c_int` — `-1` when there is no deadline; else
  `ceil(remaining_ms)` (rounding **up** so delayed work never runs early), clamped to `0` when negative.
- `work_source_prepare` — write the timeout and always return `GFALSE`, so the timeout is honoured
  (returning `GTRUE` would make the timeout 0 and the poll would never block).
- `work_source_check` — when `G_IO_IN` is set, read up to **two** 8-byte delays from the pipe (a task
  may have posted a task), calling `state.on_schedule_work(delay_ms[0])` for one complete value and
  `on_schedule_work(delay_ms[1])` for two; log an error on a short read; return `GTRUE` when the
  deadline has expired (`get_time_interval_milliseconds(...) == 0`), else `GFALSE`.
- `work_source_dispatch` — `on_timer_timeout()`, always return `GTRUE` so the source stays registered.
- `retry_eintr` — retry while the result is `-1` and `errno == EINTR` (`base/posix/eintr_wrapper.h`).

### 9.5 Electron feasibility

**`absent`, in its entirety — `mod.rs` and all three platform backends.**

Electron owns its message loop. The Electron main process runs Chromium's `MessageLoop` plus Node's
`uv` loop, and there is no embedder hook equivalent to `OnScheduleMessagePumpWork`; there is no
`external_message_pump` setting, no `do_message_loop_work` to call, no timer to own, no wake signal to
service and no reentrancy to detect. The reason the module exists — winit and CEF both wanting to own
the same native loop — does not arise, because the Rust side and the Electron side are separate
processes communicating over a socket.

| item | tag | note |
| --- | --- | --- |
| `CefExternalPump` → `ElectronExternalPump` | `absent` | Keep the type and its three methods for layout parity, with `new()`/`do_work()`/`on_schedule_message_pump_work()` as documented no-ops. The runtime must not call `do_work()` from its event loop. |
| `PlatformPump` (all three) | `absent` | No platform timer is needed. If the files are kept, each may be an empty struct with the same four methods. |
| `K_TIMER_DELAY_PLACEHOLDER`, `K_MAX_TIMER_DELAY` | `absent` | Constants with no consumer. |

The "wake signal" concept does survive in a different place: the runtime must still wake its own event
loop when an Electron event arrives. That is the target's existing IPC reader thread + event channel
(`src/electron_impl/ipc.rs`), not this module. Do not conflate the two in the ported docs.

---

## 10. `src/cef_impl/` → `src/electron_impl/` role map

`cef_impl/mod.rs` (10 lines) is a pure module declaration:

```rust
pub(crate) mod client;
pub(crate) mod cookie;
pub(crate) mod ipc;
pub(crate) mod preferences;
pub(crate) mod request_context;
pub(crate) mod request_handler;
```

On the Electron side the directory is `electron_impl/` and the same six names should exist where a
responsibility exists. The target already has `electron_impl/mod.rs` (declaring `ipc`) and
`electron_impl/ipc.rs` (the JSON-RPC transport). The table below is the mapping contract.

| reference file | CEF interface implemented | Electron responsibility |
| --- | --- | --- |
| `mod.rs` | module list | same module list under `electron_impl/` |
| `cookie.rs` | `CookieVisitor`, `CookieManager` | `webview.cookies` / `webview.setCookie` / `webview.deleteCookie` RPC |
| `ipc.rs` | `RenderProcessHandler`, `Client::on_process_message_received` | preload `contextBridge` + bridge event carrying `{webviewId, url, body}` |
| `preferences.rs` | `PreferenceManager`, `RequestContext::set_content_setting` | mostly absent; permission handlers + per-webview `devtools` flag |
| `request_context.rs` | `RequestContextHandler`, `request_context_create_context` | `session.fromPartition` |
| `request_handler.rs` | `RequestHandler`, `ResourceHandler`, `SchemeHandlerFactory` | navigation events, `protocol.register` + `protocol.request`/`respond`, `setCertificateVerifyProc` |
| `client/mod.rs` | `Client` aggregate | per-webview event routing + `webview.newWindow` |
| `client/command.rs` | `CommandHandler` | `window.event`/`appCommand` + `before-input-event` |
| `client/context_menu.rs` | `ContextMenuHandler` | `webContents` `context-menu` + `Menu.buildFromTemplate` |
| `client/display.rs` | `DisplayHandler` | `titleChanged` / `navigationCommitted` / fullscreen events / `console-message` |
| `client/download.rs` | `DownloadHandler` | `download.requested` / `download.finished` / `webview.downloadResponse` |
| `client/drag.rs` | `DragHandler`, `ResourceRequestHandler` | renderer-side drag bridge (runtime-agnostic) |
| `client/frame.rs` | `FrameHandler` | `frame-created` / `did-frame-navigate` (partial) |
| `client/keyboard.rs` | `KeyboardHandler` | `before-input-event` + `preventDefault` |
| `client/life_span.rs` | `LifeSpanHandler` | `webview.newWindow` / `webview.newWindowResponse` / `webview.closed` |
| `client/load.rs` | `LoadHandler` | `loadStarted` / `loadFinished` / `loadFailed` / `navigationCommitted` |
| `client/permission.rs` | `PermissionHandler` | `permission.requested` / `webview.permissionResponse` + `setPermissionRequestHandler` |
| `client/process.rs` | `BrowserProcessHandler` | handshake `ready`; no child-process hook |

### 10.1 `cef_impl/cookie.rs` (169 lines)

**Purpose.** `CookieManager` access for one webview: two `CookieVisitor` wrappers that collect cookies
into a shared vector and deliver the result over an `mpsc::Sender`, bidirectional conversion between
`cef::Cookie` and `tauri_runtime::Cookie`, and the `AppWebview::cookie_manager()` accessor.

**Public items.** No `[ext]` items.

`[int]` `pub(crate) fn visit_url_cookies(manager: CookieManager, url: Url, tx: CookieResultSender)`
(`:125`), `pub(crate) fn visit_all_cookies(manager: CookieManager, tx: CookieResultSender)` (`:135`),
`pub(crate) fn set_cookie(manager: CookieManager, url: Option<String>, cookie: Cookie<'static>)`
(`:142`), `pub(crate) fn delete_cookie(manager: CookieManager, url: Option<String>, cookie: Cookie<'static>)`
(`:152`), and `impl AppWebview { pub fn cookie_manager(&self) -> Option<CookieManager> }` (`:162-168`,
`[int]` because `AppWebview` is `pub(crate)`). Private: `CookieResultSender`, `CollectedCookies`,
`cookie_from_cef` (`:16`), `cef_cookie_from_cookie` (`:39`), `CollectUrlCookiesVisitor` (`:62`),
`CollectAllCookiesVisitor` (`:84`).

**Behavioural rules.**

- `cookie_from_cef`: name/value always; `domain` and `path` only when non-empty; `secure(true)` when
  `cookie.secure == 1`; `http_only(true)` when `cookie.httponly == 1`. Returns an owned `Cookie<'static>`.
- `cef_cookie_from_cookie`: name/value always; `domain`/`path` when the Tauri cookie has them;
  `secure = 1` / `httponly = 1` when the Tauri cookie says `true`.
- The visitor's `visit` pushes and returns `1` (continue). `_count`, `_total` and `_delete_cookie` are
  ignored — no cookie is ever deleted from a visitor.
- **The result is delivered from the visitor's `Drop`**, not from `visit`. CEF never invokes `visit`
  when the cookie store is empty and releases the visitor without a final callback, so a query against
  a URL/store with no matching cookies would otherwise never send and the dispatcher's blocking `recv`
  would hang forever. `Drop` sends `Ok(collected.clone())` and covers the empty, non-empty and
  "visit failed to start" cases uniformly (when `visit_url_cookies` fails to start, the visitor is
  dropped right there and sends the empty result).
- `visit_url_cookies` passes `includeHttpOnly = 1`; `set_cookie`/`delete_cookie` pass no completion
  callback (`Option::<&mut …>::None`), so they are fire-and-forget.
- `cookie_manager()` = `self.host.request_context().and_then(|rc| rc.cookie_manager(None))`.

**Electron feasibility.** `emulated` for all five functions. `webview.cookies { webview_id, url? }`
returns `Vec<rpc::webview::Cookie>`; `webview.setCookie { webview_id, CookieInit }`;
`webview.deleteCookie { webview_id, url, name }`. The `CookieManager` handle is replaced by the webview
id (or its partition). Field mapping is direct: `domain`, `path`, `secure`, `httpOnly`, plus
`expirationDate` and `sameSite`, which the reference has no place for and the target should carry
through rather than drop. `cookie_manager()` becomes `absent` as a native handle — keep the accessor
returning `None`, or replace it with a webview-id accessor, and document the change.

### 10.2 `cef_impl/ipc.rs` (168 lines)

**Purpose.** Tauri's `window.ipc.postMessage` channel: a `V8Handler` installed into every V8 context
that turns one string argument into a `tauri:ipc` process message, and the browser-side receiver that
turns that message into an `http::Request` handed to the application's `IpcHandler`.

**Public items.** No `[ext]` items.

`[int]` `pub struct TauriRenderProcessHandler` (`:109-122`, a `wrap_render_process_handler!` wrapper whose
only callback is `on_context_created` → `install_ipc_post_message`), and
`pub(crate) fn on_process_message_received<T: UserEvent>(client: &TauriCefBrowserClient<T>, frame: Option<&mut Frame>, source_process: ProcessId, message: Option<&mut ProcessMessage>) -> c_int`
(`:124`). Also `pub(crate) type IpcHandler<T> = dyn Fn(DetachedWebview<T, CefRuntime<T>>, http::Request<String>) + Send`
(`:17`). Private: `IPC_MESSAGE_NAME = "tauri:ipc"` (`:14`), `IPC_POST_MESSAGE_FUNCTION = "postMessage"`
(`:15`), `IpcPostMessageV8Handler` (`:20`), `install_ipc_post_message` (`:77`).

**Behavioural rules.**

- Injection (`install_ipc_post_message`): build `window.ipc = { postMessage: <function> }` with property
  attributes `READONLY | DONTENUM | DONTDELETE` on both `window.ipc` and `window.ipc.postMessage`. A
  missing context, global object, object value or function value aborts silently.
- `execute`: ignore any name other than `postMessage`; require **exactly one** argument that is a
  string, otherwise set the V8 exception `"window.ipc.postMessage expects a string argument"` and
  return `1` (handled). Require a current V8 context and a frame, else return `1` with no message.
  Build a `tauri:ipc` process message with argument 0 = the frame URL and argument 1 = the body, and
  `frame.send_process_message(ProcessId::BROWSER, …)`. Set `retval` to `undefined` and return `1`.
- Receiving: ignore any `source_process` other than `RENDERER`; ignore any message whose name is not
  `tauri:ipc`; return `1` when the client has no `ipc_handler` (handled, nothing to do); take argument
  0 as the URL, falling back to the frame URL when it is empty; argument 1 as the body; build
  `http::Request::builder().uri(url).body(body)` and call the handler with a `DetachedWebview` carrying
  the client's label and a `CefWebviewDispatcher` over the client's window id, webview id and context.
  A URL that `http::Request` refuses drops the message.
- `Client::on_process_message_received` (`client/mod.rs:278-289`) gates this: a client with an `opener`
  (a popup), a browser that `is_popup() != 0`, or a browser the frame-navigation state does not own
  returns `0` — a CEF popup, including a DevTools window, never inherits the root IPC identity.

**Electron feasibility.** `emulated` throughout. Electron's equivalent is the preload script:
`contextBridge.exposeInMainWorld('ipc', { postMessage })` over `ipcRenderer.send`, forwarded to Rust by
the Electron main process as a bridge event carrying `{webviewId, url, body}`. `electron-rs` already
ships `assets/preload.cjs` and a `webview.eval`/`webview.create` `preload_scripts` hook, so the
injection point exists. `TauriRenderProcessHandler` is `absent` (no render-process-handler interface);
the injection point is the preload, not a V8 context callback. The URL fallback, the exactly-one-string
rule, the exception text, and the popup gate must all be preserved — the popup gate becomes "only the
webview's own `webContents` may send".

### 10.3 `cef_impl/preferences.rs` (312 lines)

**Purpose.** Chromium preferences and content settings applied to every webview's request context and
to the browser-wide preference store. The CEF runtime creates Chrome-style browsers, so each webview is
backed by a real Chrome profile and inherits browser-shaped behaviour: a "Save password?" bubble, address
and credit-card save bubbles, a "Translate this page?" bubble, and background requests to Google. None
of that belongs in an application webview, so it is turned off per request context right after the
profile finishes initializing. Two stores, two lifetimes: a *profile* preference lives on the request
context and can only be written after `on_request_context_initialized`; a *global* preference lives in
Chromium's local state and is reachable through `cef::preference_manager_get_global()` once the CEF
context is initialized. Safe Browsing deliberately stays on.

**Public items.** No `[ext]` items.

`[int]` `pub(crate) fn set_preference_error_slot() -> CefString` (`:52`),
`pub(crate) const DEVTOOLS_AVAILABILITY: &str = "devtools.availability"` (`:65`),
`pub(crate) const DEVTOOLS_ALLOWED: i64 = 0` (`:71`),
`pub(crate) const REMOTE_DEBUGGING_ALLOWED: &str = "devtools.remote_debugging.allowed"` (`:76`),
`pub(crate) fn apply_app_webview_preferences(request_context: &RequestContext, overrides: &[(String, serde_json::Value)])`
(`:196`), `pub(crate) fn apply_default_content_settings(request_context: &RequestContext, settings: &[(ContentSettingTypes, ContentSettingValues)])`
(`:218`), `#[must_use] pub(crate) fn set_preference<M: ImplPreferenceManager>(manager: &M, name: &str, value: &serde_json::Value) -> bool`
(`:235`), `pub(crate) fn apply_global_preferences(overrides: &[(String, serde_json::Value)])` (`:264`).
Private: `PREFERENCES` (`:102-113`), `SESSION_RESTORE_ON_STARTUP` (`:125`), `to_cef_value` (`:134`).

Callers: `runtime.rs:3233` (inserts `devtools.availability = 0` at index 0 of `profile_preferences`),
`runtime.rs:3293` (inserts `devtools.remote_debugging.allowed = false` at index 0 of
`global_preferences`), `runtime.rs:3557` (`apply_global_preferences`), and
`request_context.rs:369-370` (`apply_app_webview_preferences`, `apply_default_content_settings`).

**Behavioural rules.**

- `PREFERENCES` — 10 entries, **all `false`**: `credentials_enable_service`,
  `profile.password_manager_leak_detection`, `autofill.profile_enabled`, `autofill.credit_card_enabled`,
  `translate.enabled`, `alternate_error_pages.enabled`, `search.suggest_enabled`,
  `privacy_sandbox.m1.topics_enabled`, `privacy_sandbox.m1.fledge_enabled`,
  `privacy_sandbox.m1.ad_measurement_enabled`. `safebrowsing.enabled` is deliberately absent (a Tauri
  webview routinely loads third-party content, so it is not the closed world that would make the
  protection pointless; an app that only loads its own content can opt out with
  `Cef::safe_browsing(false)`). A test asserts every entry is `false` and that `safebrowsing.enabled`
  is not present.
- `SESSION_RESTORE_ON_STARTUP = ("session.restore_on_startup", 5)` — `1` ("continue where you left off")
  makes `SessionService::RestoreIfNecessary` reopen every tab the profile's last session had, in Chrome
  windows of its own; those tabs are the application's own webviews, so each launch would bring back
  hidden copies the runtime does not own, never closes, and that keep the process alive. `5` is Chrome's
  "open the New Tab page", which restores nothing.
- `DEVTOOLS_AVAILABILITY = "devtools.availability"`, `DEVTOOLS_ALLOWED = 0` (Chromium's
  `kDisallowedForForceInstalledExtensions`). The runtime **never** writes the disallowing value (`2`):
  CEF gates `SendDevToolsMessage` on this preference and refuses it silently, which would leave every
  window stuck on its blank placeholder. It is written as the default anyway because Chromium persists
  it in the profile, so a profile an earlier version wrote `kDisallowed` into stays broken until
  something writes it back.
- `REMOTE_DEBUGGING_ALLOWED = "devtools.remote_debugging.allowed"` — the local-state preference
  `RemoteDebuggingServer::GetInstance` consults before starting a server for `--remote-debugging-port`
  or `--remote-debugging-pipe`. Pinned `false` whenever no transport was configured.
- `set_preference_error_slot()` returns `CefString::from("")` — the **owned** variant, which converts to
  a real writable pointer. `CefString::default()` is the borrowed-none variant and converts to null,
  which makes CEF's `DCHECK(error); if (!error) return 0;` fail before the preference service is
  consulted.
- `to_cef_value`: JSON → `base::Value` for every shape. Integral numbers must stay integral
  (`number.as_i64()` first; `i32::try_from` with a `log::debug!` and `None` on overflow; only a
  non-integral number becomes a double) because Chromium refuses a double in an int preference's place.
  `None` only when CEF will not allocate.
- `apply_app_webview_preferences`: `PREFERENCES` (as `Bool`) chained with `SESSION_RESTORE_ON_STARTUP`
  (as an integer), then `overrides` — so naming a preference this module disables turns it back on, and
  a repeated name keeps its last value. Failures are ignored (`let _ =`).
- `apply_default_content_settings`: `set_content_setting(None, None, type, value)` — a null URL pair is
  CEF's spelling of "the default for every origin", which is the application-wide policy an embedder
  wants: a page cannot ask for a permission whose default is `BLOCK`, and one whose default is `ALLOW`
  never prompts.
- `set_preference`: `can_set_preference` first (a refused preference is `log::debug!`, not a warning,
  because which preferences a build registers as writable varies and there is one request context per
  webview); then build the value; then `set_preference(name, value, error)` and check `== 1`. Returns
  whether it was written; `#[must_use]` because a caller for whom the write is the whole point — the
  proxy is the standing case — must check.
- `apply_global_preferences`: no-op on an empty override list; `cef::preference_manager_get_global()`
  once the CEF context is initialized, else `log::debug!` and skip.

**Electron feasibility.**

| item | tag | mechanism / reason |
| --- | --- | --- |
| `PREFERENCES` (the 10 Chrome-profile preferences) | `absent` | Electron does not create Chrome-style profiles: there is no password manager, no autofill, no translate bubble, no alternate-error-page service, no omnibox suggest, and no Privacy Sandbox consent flow to keep off. Nothing to write. |
| `SESSION_RESTORE_ON_STARTUP` | `absent` | Electron never restores a Chrome session into Chrome windows. |
| `DEVTOOLS_AVAILABILITY` / `DEVTOOLS_ALLOWED` | `absent` | The equivalent is the per-webview `devtools` boolean in `webview.create`'s `Config` (`rpc.rs:557`), already used by `electron-rs`'s `webview.ts` (`devtoolsAllowed`). |
| `REMOTE_DEBUGGING_ALLOWED` | `absent` | Electron's equivalent is not appending `--remote-debugging-port` / `--inspect`; there is no preference to pin. |
| `set_preference_error_slot` | `absent` | No CEF error slot. |
| `to_cef_value` | `absent` | No `base::Value` bridge. |
| `set_preference` | `absent` | No preference-manager API. |
| `apply_app_webview_preferences` | `absent` | Keep the function with the same signature (minus CEF types) and an empty body, or drop it and document why in the module header. |
| `apply_default_content_settings` | `emulated` | The closest Electron mechanism is `session.setPermissionRequestHandler` + `session.setPermissionCheckHandler` per partition, which is what `permission.requested`/`webview.permissionResponse` already drive. A default-`BLOCK`/default-`ALLOW` table can be expressed there. |
| `apply_global_preferences` | `absent` | No local-state preference store. |

### 10.4 `cef_impl/request_context.rs` (424 lines)

**Purpose.** Creates a per-webview CEF `RequestContext`, registers Tauri's custom URI scheme handler
factories on it, and arranges for a continuation to fire once the underlying Chromium `Profile` is fully
created. CEF only initializes a request context synchronously when its `cache_path` equals
`Settings.root_cache_path` (it reuses the global "Default" profile) or is empty (off-the-record); any
other path takes `ChromeBrowserContext::InitializeAsync`'s `CreateProfileAsync` branch, and calling
`browser_host_create_browser_sync` straight after `request_context_create_context` would fail
`CefRequestContextImpl::VerifyBrowserContext()` and return a null browser. Routing browser creation
through `on_request_context_initialized` gives one code path for every layout.

**Public items.** No `[ext]` items.

`[int]` `pub(crate) fn apply_theme_scheme(request_context: Option<&RequestContext>, theme: Option<Theme>)`
(`:32`), `pub(crate) type RequestContextInitContinuation = Box<dyn FnOnce(Option<RequestContext>) + 'static>`
(`:115`), `pub(crate) fn deferred_init_continuation<F>(work: F) -> (Arc<AtomicBool>, RequestContextInitContinuation)`
(`:122`), `pub(crate) fn wait_for_deferred_init(flag: &Arc<AtomicBool>)` (`:176`),
`pub(crate) fn request_context_from_webview_attributes<'a>(…) -> Option<RequestContext>` (`:308-318`).
Private: `theme_to_color_variant` (`:24`), `resolve_request_context_cache_path` (`:59`),
`data_store_directory_name` (`:93`), `AllowNestableTasks` (`:200`), `NESTABLE_TASKS_DEPTH` (`:227`),
`WebviewRequestContextHandler` (`:231`), `apply_proxy` (`:253`).

**Behavioural rules.**

- **Cache-path resolution** (`resolve_request_context_cache_path(global_cache_path, data_directory)`),
  because CEF requires `cache_path` to be empty or an absolute path equal to / a child of
  `Settings.root_cache_path`:
  1. absolute and `starts_with(global_cache_path)` → used as-is (callers that opt into a path under the
     app cache get the exact location they asked for);
  2. relative with **no** `Component::ParentDir` → `global_cache_path.join(data_directory)`;
  3. otherwise → `<root>/Profile-<suffix>`, where `suffix` is
     `base64::URL_SAFE_NO_PAD` of `Sha256(data_directory.as_os_str().as_encoded_bytes())[..16]`. Distinct
     `data_directory` values produce distinct profiles and the same value maps to the same on-disk
     profile across runs. Both fallback paths log a warning; the derived path is logged at `info`.
- **`data_store_directory_name(identifier: &[u8; 16])`** → `DataStore-<8>-<4>-<4>-<4>-<12>` from the hex
  of the identifier, i.e. the UUID spelling WKWebView's `WKWebsiteDataStore(forIdentifier:)` would show.
- **Cache-path selection** in `request_context_from_webview_attributes` (`:329-348`), in order:
  `incognito` → `CefStringUtf16::from("")` (off-the-record); `data_directory` → the resolved path with
  `create_dir_all` first (a failure is `log::error!`); `data_store_identifier` → `<root>/DataStore-…`
  (`data_directory` wins when both are set); otherwise the **global context's own** `cache_path` (the
  `global_cache_path` argument does not work here).
- **`apply_theme_scheme`** → `request_context.set_chrome_color_scheme(ColorVariant::{DARK,LIGHT,SYSTEM}, 0)`;
  `None` theme maps to `SYSTEM`.
- **The deferred-init machinery.** `WebviewRequestContextHandler::on_request_context_initialized` takes
  the stored continuation (once) and calls it with a cloned `RequestContext`.
  `deferred_init_continuation` wraps a continuation so a shared `Arc<AtomicBool>` is flipped on **every**
  exit path — normal return, early `return`, or panic — via a `Drop` guard. `wait_for_deferred_init`
  blocks until the flag flips:
  - **on the CEF UI thread** (`cef::currently_on(TID_UI) != 0`): enter `AllowNestableTasks` and pump
    `cef::do_message_loop_work()` in a loop. Nestable tasks must be enabled because the code may already
    be inside another CEF task, and without `CefSetNestableTasksAllowed(true)` Chromium's
    `RunLoop::RunUntilIdle` refuses to dispatch to the UI thread and the deferred init never fires.
  - **on any other thread**: `sleep(1ms)` in a loop; `do_message_loop_work` cannot be called from there
    (it asserts on the init thread) and the UI thread's own pump will pick up the queued task.
  - `AllowNestableTasks` is an RAII guard over a thread-local depth counter, so only the outermost call
    on a thread toggles the flag — CEF requires balanced enable/disable and forbids reentrancy
    (`CHECK(allowed != has_value())`), and an `on_initialized` continuation that creates another webview
    must be safe.
- **The wrapped callback** (`:362-378`) applies, in order: `apply_app_webview_preferences`,
  `apply_default_content_settings`, `apply_proxy` (when the webview set `proxy_url`), then the caller's
  `on_initialized`, then releases the strong `RequestContext` reference held in `rc_holder`.
- **`apply_proxy`** writes the Chromium `proxy` preference as
  `{ "mode": "fixed_servers", "server": "<scheme>://<host>[:<port>]" }`; scheme mapping
  `socks5|socks5h → socks5`, `socks4|socks4a → socks4`, `https → https`, anything else → `http`; a URL
  with no host is ignored with a warning; a failed write is a **warning**, not a debug log, because a
  webview that asked for a proxy and did not get one would silently send its traffic straight out.
- **Scheme factories** are registered synchronously after `request_context_create_context` returns and
  before the `OnRequestContextInitialized` task is dispatched, one per `custom_schemes` entry, under the
  scheme `{scheme}.localhost` with the custom protocol scheme.

**Electron feasibility.** Mostly `absent`; the responsibility moves to partitions.

| item | tag | mechanism / reason |
| --- | --- | --- |
| `request_context_from_webview_attributes` | `emulated` | `session.fromPartition('persist:<name>')` is the request-context equivalent and is created **synchronously**, so the whole deferred-init protocol is unnecessary. `incognito` → a non-persistent partition (`'<name>'`); `data_directory` → a partition name derived by the same `Profile-<hash>` rule; `data_store_identifier` → the same `DataStore-<uuid>` name. |
| `resolve_request_context_cache_path` | `direct` | Pure path/hash logic; keep it, and keep the `Profile-<base64url(sha256[..16])>` derivation so a profile keeps its identity across runtimes. |
| `data_store_directory_name` | `direct` | Pure formatting. |
| `apply_theme_scheme` | `emulated` | Electron: `nativeTheme.themeSource` per app, or the webview's `background_color`; there is no per-request-context Chrome colour scheme. `app.theme` exists in the bridge. |
| `deferred_init_continuation` | `absent` | No asynchronous profile initialization. |
| `wait_for_deferred_init` | `absent` | No CEF UI thread, no `do_message_loop_work`, no nestable tasks. |
| `apply_proxy` | `emulated` | Electron: `session.setProxy({ mode: 'fixed_servers', proxyRules: '<scheme>://<host>:<port>' })` — same semantics, same warning-on-failure rule. |

### 10.5 `cef_impl/request_handler.rs` (710 lines)

**Purpose.** The `RequestHandler` / `ResourceHandler` / `SchemeHandlerFactory` trio: certificate-error
policy, renderer-termination reporting, main-frame navigation policy, and serving the application's
custom URI schemes (with init-script injection, CSP hash injection and `Origin` repair) from the main
thread.

**Public items.** No `[ext]` items.

`[int]` `pub struct WebRequestHandler<T: UserEvent>` (`:171-182`, `wrap_with_args!` with
`WebRequestHandlerArgs`), `pub struct WebResourceHandler` (`:314-335`, with `WebResourceHandlerArgs`),
`pub struct UriSchemeHandlerFactory` (`:521-525`), `pub(crate) type MainThreadDispatcher = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>`
(`:43`), `pub(crate) type SchemeRegistry = Arc<Mutex<HashMap<(i32, String), (String, Arc<Box<UriSchemeProtocolHandler>>, Arc<Vec<CefInitScript>>)>>>`
(`:44-55`). Private: `HttpResponse` (`:40`), `csp_inject_initialization_scripts_hashes` (`:57`),
`inject_scripts_into_html_body` (`:88`), `keep_encoding_declaration_first` (`:134`), `ThreadSafe`
(`:600`), `read_request_body` (`:611`), `get_request_headers` (`:651`), `termination_reason` (`:673`).

**Behavioural rules.**

- **`WebRequestHandler::on_certificate_error`** — `CertificateErrorPolicy::ChromeInterstitial` → `0`
  (Chromium shows the SSL interstitial with its "proceed anyway" link);
  `CertificateErrorPolicy::Cancel` → `1` (CEF reads a `false` return as "cancel"; the page gets a
  network error it cannot click past). The URL can carry credentials and tokens, so only its **origin**
  (`url.origin().ascii_serialization()`, or `"an unknown origin"`) is logged, with the Chromium error.
- **`on_render_process_terminated`** — emits `FrameEventKind::RendererTerminated` and calls the
  application's `OnWebContentProcessTerminateHandler` with
  `WebContentProcessTermination { reason, error_code: Some(code), error_string }`.
  `termination_reason` maps `ABNORMAL_TERMINATION→Abnormal`, `PROCESS_WAS_KILLED→Killed`,
  `PROCESS_CRASHED→Crashed`, `PROCESS_OOM→OutOfMemory`, `LAUNCH_FAILED→LaunchFailed`,
  `INTEGRITY_FAILURE→IntegrityFailure`, everything else (including `NUM_VALUES`) → `Unknown`.
- **`on_before_browse`** — `INITIAL_LOAD_URL` passes; an unparseable URL passes; for the **main frame**
  only, a `navigation_handler` returning `false` cancels the navigation (`return 1`); otherwise emit
  `FrameEventKind::NavigationStarted { url }` and return `0`. Native observers therefore only see
  admitted navigations, so a denied navigation cannot strand their barrier.
- **`resource_request_handler`** — `None` when the drag-drop bridge is disabled (the handler would pass
  everything through, and CEF calls it for every subresource/fetch/XHR); otherwise a
  `WebDragDropResourceRequestHandler`.
- **`WebResourceHandler::process_request`** — runs the application's `UriSchemeProtocolHandler` on the
  **main thread** via `MainThreadDispatcher`, because that is where wry's handlers run and what Tauri
  relies on: the `ipc` handler runs the invoked command while holding the plugin store lock, and the
  event loop takes that lock to deliver `RunEvent`s, so a command that waits on the main thread (a menu
  or tray command) would deadlock from any other thread; and the objects those commands create or drop
  (an `NSStatusItem`) are main-thread only. The responder is safe to call from any thread, so an
  asynchronous handler keeps working. Before dispatch: read the body (`PDE_TYPE_BYTES` copied,
  `PDE_TYPE_FILE` read from disk), copy the headers, and **repair `Origin`** — when the initiator is the
  non-opaque main frame and the renderer sent no `Origin` or a literal `null`, insert the captured
  initiator origin; a correct renderer-sent origin always wins. Method falls back to `GET` when
  `http::Method::from_bytes` fails.
- **The responder** — when the response is `text/html`, inject the init scripts into `<head>` (see
  below); then, when a `Content-Security-Policy` header is present, inject the init-script hashes into
  its `script-src` directive (defaulting to `'self'` when absent) and replace the header; store the
  response and `callback.cont()`.
- **`inject_scripts_into_html_body`** — parse with `tauri_utils::html::parse`, find or create `<head>`,
  prepend each matching init script (iterated **in reverse** so declaration order is preserved) as a
  `<script>` element with the script text as a text node, then
  `keep_encoding_declaration_first`. Non-UTF-8 bodies are returned unchanged.
- **`keep_encoding_declaration_first`** — Chromium only pre-scans the first 1024 bytes for a
  `<meta charset>`, and the prepended scripts are much larger than that, so an existing declaration is
  moved back to the front of `<head>`; when there is none, `<meta charset="utf-8">` is prepended. Without
  this the document is decoded as windows-1252, which mojibakes the page and breaks the CSP hashes
  computed over the UTF-8 source.
- **`read`** — `usize::try_from(bytes_to_read)`, read from the stored `Cursor`, write `bytes_read`, return
  `1` when bytes were read and `0` at EOF.
- **`response_headers`** — set the status, copy every header whose value is valid UTF-8, remember
  `Content-Type`, force `Cache-Control: no-store` (replace), set the MIME type from
  `Content-Type`'s first `;`-segment (default `text/plain`), set `response_length = -1` (unknown) and
  clear any `redirect_url`.
- **`UriSchemeHandlerFactory::create`** — look the handler up in the `SchemeRegistry` by
  `(browser.identifier(), scheme)`. When `browser` is **null** (service-worker script/update fetches and
  `CefURLRequest`), fall back to **any** registry entry for that scheme and use an **empty** init-script
  list — returning `None` would fall through to the network service, where `{scheme}.localhost` resolves
  to loopback and the connection is refused, which is how service-worker registration on a custom
  protocol origin used to fail. `is_main_frame` is `frame.is_main() == 1`, defaulting to `true` when the
  frame is null. `initiator_origin` is the main frame's URL origin, filtered to non-`null`.
- **`SchemeRegistry`** is keyed by `(browser identifier, scheme)` and each entry holds the webview label,
  the app-level protocol handler and the webview's init scripts.

**Electron feasibility.** `emulated`, split across three mechanisms:

| reference responsibility | Electron mechanism |
| --- | --- |
| navigation policy (`on_before_browse`) | `webview.navigationRequested` + `webview.navigationResponse` (allow/deny), and `will-navigate` in `webview.ts` |
| certificate policy | `session.setCertificateVerifyProc` (return `0` = use Chromium's verdict / show the interstitial, or `-2` = fail the request) |
| renderer termination | `webview.processGone` (`{ reason, exitCode }`), mapped to the same `WebContentProcessTerminationReason` values |
| custom schemes | `protocol.register` + `protocol.request` / `protocol.respond` (base64 body, status, headers), or `protocol.handle` in the main process |
| init-script injection | `webview.create`'s `init_scripts` (`rpc::webview::InitScript::{Source, Detailed{main_frame_only}}`) — Electron injects them itself, so **the HTML rewriting, CSP hash injection and encoding-declaration fix are `absent` and must not be ported** |
| `Origin` repair | `session.webRequest.onBeforeSendHeaders` rewriting the `Origin` header, or the `webHost.authenticate`/`bind` flow electron-rs already provides |
| `MainThreadDispatcher` | unnecessary: the Electron main process is a different process, so a protocol handler running on the bridge cannot deadlock the Rust event loop the way a CEF IO-thread handler could. Keep the type if other modules reference it. |

### 10.6 `cef_impl/client/mod.rs` (290 lines)

**Purpose.** The `Client` aggregate: `TauriCefBrowserClient<T>` (the state one webview's CEF client
needs) plus `TauriCefBrowserClientHandlers<T>` (the application callbacks), and the `Client` trait
methods that hand out each sub-handler and gate `on_process_message_received`.

**Public items.** No `[ext]` items.

`[int]` `pub(crate) struct TauriCefBrowserClientHandlers<T: UserEvent>` (`:44-58`) with a manual `Clone`
(`:60-75`), and `pub(crate) struct TauriCefBrowserClient<T: UserEvent>` (`:80-99`, `wrap_with_args!` with
`TauriCefBrowserClientArgs`). Re-exports at `:33-42`: `DragDropEventTarget`, `DragDropScriptEvent`,
`DragDropState`, `WebDragDropResourceRequestHandler`, `WebDragDropResourceRequestHandlerArgs`,
`drag_drop_initialization_script`, `event_from_script_event`, `PermissionRequestHandler`,
`TauriCefBrowserProcessHandler`.

**Behavioural rules.**

- **Which handler is installed when** (`Client` impl, `:101-289`): `render_handler` from the stored
  optional; `frame_handler` only when a frame-event handler exists; `drag_handler` only when
  `drag_drop_handler_enabled`; `request_handler` **always**; `life_span_handler` always (it carries the
  popup factory); `load_handler`, `display_handler`, `context_menu_handler`, `keyboard_handler`,
  `command_handler`, `permission_handler` always; `download_handler` only when the application has one
  (CEF cancels every download of a client whose download handler is NULL).
- **The popup client factory** (`:152-211`): a CEF popup is a real Chrome window, not an app window, so
  it keeps the opener's allowances and a reduced handler set. It keeps `navigation_handler`,
  `new_window_handler` and `download_handler`; it **drops** `ipc_handler`,
  `console_message_handler`, `on_page_load_handler`, `document_title_changed_handler` and
  `web_content_process_terminate_handler`; its `frame_event_handler` is the **internal** navigation
  observer only (never the opener's app observer, because every `FrameEvent` carries the full URL and a
  popup navigates wherever its own content goes — an SSO/OAuth window is the standing case); its
  `permission_request_handler` maps the opener's `Allow` to `Default` while passing `Deny` through (a
  `PermissionKind` names no origin, so a grant for the app's own content is no answer about the popup's
  content, but a refusal must not be obtainable by opening a popup). `drag_drop_handler_enabled: false`,
  `drag_drop_state: Arc::default()`, `initial_url: None`, `render_handler: None`, `cursor_handler: None`.
- **`on_process_message_received`** (`:278-289`): return `0` when the client has an `opener`, when the
  browser `is_popup() != 0`, or when the browser id is not in the frame-navigation state; otherwise
  delegate to `ipc::on_process_message_received`.

**Electron feasibility.** `emulated`. There is no `Client` aggregate; the equivalent is the bridge's
per-webview event routing. The popup handler-set rules must be reproduced at the `webview.newWindow`
decision point: a popup guest keeps navigation and download handling, gets no Tauri IPC identity, no
console/title/page-load observers, and its permission answers default to `Default` unless the opener
denied. `NewWindowDecision { action: Allow|Deny, window_id: Option<u32> }` is the wire shape.

### 10.7 `cef_impl/client/command.rs` (344 lines)

**Purpose.** Chrome's accelerator table, filtered down to what an application window can meaningfully do.
A Chrome-style browser keeps its whole accelerator table live even when hosted as a child view with no
browser UI, so Ctrl+N opens a real Chrome window next to the app's and Ctrl+T a tab in a window the app
does not own.

**Public items.** No `[ext]` items. `[int]` `pub struct TauriCefCommandHandler<T: UserEvent>` (`:208-215`,
`wrap_with_args!` with `TauriCefCommandHandlerArgs`). Private: the six command tables
(`WINDOW_AND_TAB_COMMANDS` 22 names `:22-45`, `DOCUMENT_COMMANDS` 7 `:52-60`, `BROWSER_CHROME_COMMANDS`
9 `:68-78`, `HISTORY_COMMANDS` 2 `:88`, `BROWSER_SURFACE_COMMANDS` 15 `:96-112`, `DEVTOOLS_COMMANDS` 5
`:123-129`, `ZOOM_COMMANDS` 3 `:141-145`), `group_commands` (`:148`), `BlockedCommands` (`:164`),
`blocked_commands` (`:170`), `fullscreen_command` (`:182`), `command_ids` (`:196`).

**Behavioural rules.**

- **IDC names, not numeric ids.** Chrome command ids are build-specific integers, so they are resolved
  from their IDC names — which do not change between builds — once per process via
  `cef::sys::cef_id_for_command_id_name`, dropping `-1` (a name this build does not know). Resolving
  lazily keeps the lookups after CEF initialization. `blocked_commands()` and `fullscreen_command()`
  cache in `OnceLock`s.
- **`on_chrome_command`** — first `owns(browser)` (`frame_navigation_state.has_browser_id(browser.identifier())`);
  a browser this webview does not own returns `0` so its commands are theirs to run (a DevTools window
  is the standing case: `ChromeBrowserDelegate` reuses the opener's client). Then, if the command is
  `IDC_FULLSCREEN` **and** the browser is not a popup, send `Message::ToggleFullscreen { webview_id }`,
  wake the event loop and return `1`. Otherwise return `1` when `blocks(command_id)` else `0`.
- **`blocks(command_id)`** — any group **not** named in `allowed_chrome_commands` whose id table contains
  the command; or, when `!devtools_enabled`, a devtools command; or, when `!zoom_hotkeys_enabled`, a zoom
  command. Anything not named runs unchanged: clipboard, find in page, text selection, undo/redo and
  reload are all legitimate.
- **The four `is_chrome_*` predicates** return `1` (visible/enabled) because `wrap_command_handler!`
  installs every callback and the trait's default body returns `0` — "hide" and "disable" — so *not*
  overriding them would silently strip Chrome UI. The one exception is
  `is_chrome_app_menu_item_enabled`, which returns `0` for a command this handler swallows, so a blocked
  entry does not draw as a working one and Chromium's `DCHECK` that a dispatched command was enabled is
  satisfied. That predicate does not gate the accelerator table or `HandleCommand`.
- `owns()` answers `false` when the identity cannot be established, so blocking is best-effort rather
  than a security boundary; what must not be reachable (the renderer sandbox, the command-line lockdown,
  DevTools when disabled) is enforced elsewhere.
- `ZOOM_COMMANDS` does not reach Ctrl+wheel zoom (Chromium applies it in the render widget) nor the zoom
  polyfill Tauri injects on Linux/macOS, which makes a keyboard zoom step twice there.

**Electron feasibility.** `emulated`. The `IDC_*` id resolution is `absent` (no command-id table).
The equivalent mechanisms are `webContents.on('before-input-event')` (already used by
`electron-rs` for F12) plus `window.event`'s `appCommand` (backed by Electron's `BrowserWindow`
`app-command`). The five `ChromeCommandGroup` tables become key/command predicates. `IDC_FULLSCREEN` →
`window.fullscreen` / `setSimpleFullscreen`. Keep `ChromeCommandGroup` and its `ALL` list exactly as
they are — they are `[ext]` API from `webview.rs:312-362`.

### 10.8 `cef_impl/client/context_menu.rs` (209 lines)

**Purpose.** Context-menu policy for the webview. The runtime creates Chrome-style browsers, so the
model handed to `on_before_context_menu` is Chrome's own page context menu — back/forward/reload, save
as, print, translate, view page source, "Search the web for…", open link in a new tab/window/incognito
window and Inspect. Alloy style would have handed a small menu addressed by `MENU_ID_*` constants; a
Chrome-style model carries IDC command ids, so entries are matched by IDC id. This file filters the model
down to what means something inside an application window.

**Public items.** No `[ext]` items. `[int]` `pub struct TauriCefContextMenuHandler { devtools_enabled: bool }`
(`:174-176`). Private: `BROWSER_ONLY_COMMANDS` (42 names, `:37-84`), `DEVTOOLS_COMMANDS` (8 names,
`:88-97`), `UNKNOWN_COMMAND_ID = -1` (`:104`), `CommandIds` (`:106`), `command_ids` (`:118`),
`resolve_command_ids` (`:127`), `remove_redundant_separators` (`:144`).

**Behavioural rules.**

- Remove every `BROWSER_ONLY_COMMANDS` id, in four documented groups: navigation and page lifecycle (6),
  saving/printing/looking behind the page (10), opening a browser surface the app does not own (12),
  sending the page/selection/frame to a web service (14). Removing a command the menu does not carry is
  a no-op, so the whole policy applies to every menu without first asking `params` which kind it is.
- Remove every `DEVTOOLS_COMMANDS` id when `!devtools_enabled`.
- `remove_redundant_separators` — a menu must not open or end with a separator and two in a row draw as
  a double rule. The loop starts as if a separator had just been seen (which drops leading ones), and
  **every loop advances on a failed `remove_at`**: a removal CEF refuses does not shrink `count()`, so
  retrying it would spin CEF's UI thread — which this runtime drives with an external message pump —
  hanging the whole application rather than just the menu. Trailing separators are removed in a second
  loop.
- An empty model is left empty on purpose: CEF then shows no menu at all, which is the right outcome.
- Everything not listed is kept: undo/redo, cut/copy/paste (including paste as plain text), delete,
  select all, spellcheck suggestions and add-to-dictionary, emoji, and the copy-link-address /
  copy-image family.

**Electron feasibility.** `emulated`. Electron's `webContents.on('context-menu', (event, params))` gives
`params.editFlags`, `params.isEditable`, `params.x/y`, `params.frame`, `params.menuSourceType`, and the
runtime builds the menu with `Menu.buildFromTemplate` + `menu.popup`. `electron-rs` already does exactly
this in `assets/runtime/webview.ts` (`onContextMenu`), so the responsibility exists but the policy is
written in TypeScript. The reference's IDC tables have no Electron counterpart; the equivalent is to
build the *reduced* template directly (edit roles + copy + selectAll + Inspect) rather than to build
Chrome's menu and delete entries. Keep the "empty menu means no menu" rule and the separator hygiene.

### 10.9 `cef_impl/client/display.rs` (137 lines)

**Purpose.** `DisplayHandler`: cursor changes, document title, address changes, console messages and
HTML fullscreen.

**Public items.** No `[ext]` items. `[int]` `pub struct TauriCefDisplayHandler<T: UserEvent>`
(`:22-30`, `wrap_with_args!` with `TauriCefDisplayHandlerArgs`). Private: the platform-dependent
`CefCursorHandle` alias (`:14-17`).

**Behavioural rules.**

- `on_cursor_change` — only when a `cursor_handler` exists **and** the browser is owned by this webview;
  then call the handler and return `1` (claim the cursor). Otherwise `0`.
- `on_title_change` — forward the title to `document_title_changed_handler`; no handler or no title is a
  no-op.
- `on_address_change` — skip `INITIAL_LOAD_URL`; parse and emit `FrameEventKind::AddressChanged { url }`.
- `on_console_message` — forward `ConsoleMessage::from_cef(level, message, source, line)` **only** for
  owned browsers (a DevTools window's frontend is itself a page that logs), and **always return `0`** so
  CEF's own logging of the message is left exactly as it was.
- `on_fullscreen_mode_change` — ignore popups and unowned browsers; send
  `Message::PageFullscreen { webview_id, fullscreen }` and wake the event loop.

**Electron feasibility.** `emulated`. `webview.titleChanged` → title; `webview.navigationCommitted` →
address; `webview.enter-html-full-screen` / `leave-html-full-screen` → fullscreen;
`webContents.on('console-message')` → console; cursor via `webview.setCursorIcon` /
`window.setCursorIcon` or the native cursor module. Two details for the implementer:

- The pinned Electron is `44.7.0` (`electron_rs::DEFAULT_VERSION`), so `console-message` fires with the
  **modern** signature `(event, details)` where `details` is
  `{ message, level: 'info'|'warning'|'error'|'debug', lineNumber, sourceId, frame }` — not the legacy
  positional `(event, level, message, line, sourceId)`. `level` is a string there and must be mapped onto
  `ConsoleMessageLevel`.
- `console-message` is **not** currently forwarded by `electron-rs`'s runtime (`assets/runtime/webview.ts`
  binds `context-menu`, `before-input-event`, the navigation/load/title/process events and `destroyed`,
  but not `console-message`), so the target must add the listener and a bridge event for it before the
  Rust side can observe anything.
- The "always return 0" rule becomes "do not call `event.preventDefault()`", so Electron's own console
  handling is untouched.

### 10.10 `cef_impl/client/download.rs` (116 lines)

**Purpose.** `DownloadHandler`: forward the application's `DownloadHandler` the two events it cares about.

**Public items.** No `[ext]` items. `[int]` `pub struct TauriCefDownloadHandler { download_handler: Arc<tauri_runtime::webview::DownloadHandler> }`
(`:10-12`).

**Behavioural rules.**

- `can_download` → `1` unconditionally; `on_before_download` is the one that validates.
- `on_before_download` — require the item and the callback; parse the item URL (unparseable → `0`);
  `suggested_path` from the suggested name (empty `PathBuf` when absent); call the handler with
  `DownloadEvent::Requested { url, destination: &mut destination }`. When it returns `true`, continue
  with `callback.cont(destination, show_dialog)` where `show_dialog = (destination == suggested_path)` —
  i.e. the save dialog is shown only when the application did not choose a path. Always return `1`.
- `on_download_updated` — parse the URL; `is_complete = is_complete() != 0`, `is_canceled = is_canceled() != 0`,
  `success = is_complete && !is_canceled`; read `full_path()` only when complete or cancelled and only
  when non-empty; call the handler with `DownloadEvent::Finished { url, path, success }` **only** when
  complete or cancelled (never on progress ticks).

**Electron feasibility.** `direct`. `download.requested { id, webviewId, url, destination }` →
`DownloadEvent::Requested`; `webview.downloadResponse { id, allow, destination? }` → the continue/save
decision (`destination: None` = show the dialog, matching `show_dialog`); `download.finished { webviewId, url, path, success }`
→ `DownloadEvent::Finished`. This is the closest one-to-one mapping in the whole scope.

### 10.11 `cef_impl/client/drag.rs` (260 lines)

**Purpose.** File drag-and-drop into a webview. CEF's `DragHandler` only reports `on_drag_enter`; the
over/drop/leave positions come from an injected script that reports them over a private fetch URL, which
a `ResourceRequestHandler` intercepts and cancels.

**Public items.** No `[ext]` items. `[int]` `pub(crate) fn drag_drop_initialization_script() -> InitializationScript`
(`:94`), `pub(crate) struct DragDropState { pub(crate) paths: Option<Vec<PathBuf>>, pub(crate) native_entered: bool, pub(crate) entered: bool }`
(`:101-106`), `pub(crate) enum DragDropEventTarget { Window, Webview }` (`:108-112`),
`pub(crate) struct DragDropScriptEvent { pub(crate) kind: String, pub(crate) x: f64, pub(crate) y: f64 }`
(`:114-120`), `pub(crate) fn event_from_script_event(drag_drop_state: &Arc<Mutex<DragDropState>>, script_event: DragDropScriptEvent) -> Option<DragDropEvent>`
(`:170`), `pub(crate) struct TauriCefDragHandler` (`:145-147`),
`pub(crate) struct WebDragDropResourceRequestHandler<T: UserEvent>` (`:215-222`, `wrap_with_args!` with
`WebDragDropResourceRequestHandlerArgs`).

**Behavioural rules.**

- The injected script (`DRAG_DROP_INIT_SCRIPT`, `:26-92`) is idempotent (`window.__TAURI_CEF_DRAG_DROP__`
  guard, `configurable: false`), listens in the **capture** phase for `dragenter`/`dragover`/`drop`/
  `dragleave`, tracks an `entered` flag, and reports `{type, x, y}` where `x/y` are
  `clientX * devicePixelRatio` / `clientY * devicePixelRatio`, via `fetch(PATH?payload=…)` with
  `cache: "no-store"`, `credentials: "omit"` and `.catch(() => {})`. `dragleave` only reports when the
  pointer is actually outside the viewport. `for_main_frame_only: false`.
- `on_drag_enter` — reset `entered`, collect paths from `DragData::file_paths` (filtering empties) or
  fall back to `file_name`; `native_entered = paths.is_some()`; **return `0`** so Chromium continues the
  drag and the script can report accurate viewport positions.
- `event_from_script_event` — `PhysicalPosition::new(x, y)`; `None` when `!native_entered`; then
  - `"enter"` → `None` if already entered, else take the paths and set `entered`, yielding
    `DragDropEvent::Enter { paths, position }`;
  - `"over"` → `Over { position }` only while entered;
  - `"drop"` → take the paths (only while entered), clear `entered` and `native_entered`, yield
    `Drop { paths, position }`;
  - `"leave"` → clear `native_entered` and `paths`; yield `Leave` only when it had entered;
  - anything else → `None`.
- `WebDragDropResourceRequestHandler::on_before_resource_load` — when the URL's path equals
  `/__tauri_cef_drag_drop__`, parse the `payload` query parameter as a `DragDropScriptEvent`, send
  `Message::DragDropScriptEvent { window_id, webview_id, target, drag_drop_state, event }`, and return
  `RV_CANCEL`; otherwise `RV_CONTINUE`.

**Electron feasibility.** `emulated`, and the *same* design is the only one available — Electron has no
native drag event to replace it with.

- **`webContents` exposes no `drag-enter` / `drag-over` / `drop` event.** Dragging files *into* web
  content is a standard web capability: the renderer receives ordinary HTML5 `dragenter`/`dragover`/
  `drop` events with a `DataTransfer`. Electron adds only the *outbound* direction
  (`webContents.startDrag(item)` in response to `ondragstart`). So the reference's injected-script bridge
  is not a CEF workaround that Electron improves on — it is the mechanism on both runtimes.
- **`on_drag_enter` / `DragData::file_paths` is `absent`.** There is no native drag callback and no
  native file-path list. The paths must come from the renderer instead: `webUtils.getPathForFile(file)`
  in the preload (the modern replacement for the removed `File.path`), because a sandboxed renderer's
  `File` objects carry no path. That is exactly the split the reference already has — the native side
  supplies paths, the script supplies viewport positions — with the two halves arriving from different
  places.
- **The script ports verbatim**, renamed to `__TAURI_ELECTRON_DRAG_DROP__`, but it must additionally
  report the file paths it got from `webUtils.getPathForFile`, since nothing else can supply them.
- **The interception point changes**: `session.webRequest.onBeforeRequest` (cancel the private
  `/__tauri_electron_drag_drop__` URL) instead of a `ResourceRequestHandler`, or `protocol.register` for
  a dedicated scheme.
- Keep the `DragDropState` machine (`native_entered` becomes "a drop carried paths"), the
  `DragDropEventTarget` distinction, and the `event_from_script_event` state transitions exactly.

### 10.12 `cef_impl/client/frame.rs` (37 lines)

**Purpose.** `FrameHandler`: turn CEF's five frame callbacks into `FrameEventKind` values.

**Public items.** No `[ext]` items. `[int]` `pub struct TauriCefFrameHandler { handler: Option<Arc<FrameEventHandler>> }`
(`:12-14`).

**Behavioural rules.** `on_frame_created` → `Created`; `on_frame_attached` → `Attached` (the `reattached`
flag is ignored); `on_frame_detached` → `Detached`; `on_frame_destroyed` → `Destroyed`;
`on_main_frame_changed` → `MainFrameChanged` emitted with the **new** frame. All go through
`frame::emit_frame_event`, which is what binds a browser id into `FrameNavigationState`.

**Electron feasibility.** `emulated` for `Created`/`MainFrameChanged` (Electron's `webContents`
`frame-created` event, and the main frame is the webContents itself); `absent` for `Attached`, `Detached`
and `Destroyed` — Electron exposes no attach/detach/destroy notification for out-of-process iframes.
Document the reduced event set; do not synthesise events that were never observed.

### 10.13 `cef_impl/client/keyboard.rs` (88 lines)

**Purpose.** Block the DevTools keyboard shortcuts when the webview disabled DevTools.

**Public items.** No `[ext]` items. `[int]` `pub struct TauriCefKeyboardHandler { devtools_enabled: bool }`
(`:15-17`). Private: the platform-dependent `CefOsEvent` alias (`:7-12`).

**Behavioural rules.** Only when `!devtools_enabled`: require a key event whose `type_` is
`KEYEVENT_RAWKEYDOWN`; block F12 (`windows_key_code == 123`) on every platform; on non-macOS block
Ctrl+Shift+I (`windows_key_code == 73` with `EVENTFLAG_CONTROL_DOWN` and `EVENTFLAG_SHIFT_DOWN`); on
macOS block Cmd+Opt+I (`73` with `EVENTFLAG_COMMAND_DOWN` and `EVENTFLAG_ALT_DOWN`). Each block sets
`*is_keyboard_shortcut = 1` and returns `1`. Everything else returns `0`.

**Electron feasibility.** `direct`. `webContents.on('before-input-event', (event, input))` +
`event.preventDefault()`; `electron-rs` already does this for F12 in `webview.ts` (`onBeforeInput`).
Extend it with the `Control+Shift+I` / `Meta+Alt+I` cases. Keep the "only when DevTools are disabled"
gate and the `is_keyboard_shortcut` equivalent (Electron has no such out-parameter; the
`preventDefault()` is the whole answer).

### 10.14 `cef_impl/client/life_span.rs` (257 lines)

**Purpose.** Browser lifetime and popups: `on_after_created` (popup registration + the
`check_and_reload_if_blank` safety net), `on_before_popup` (the application's `NewWindowHandler` and the
three `NewWindowResponse` outcomes), `on_before_popup_aborted`, `do_close` (take over the close so a
webview close does not close the whole window) and `on_before_close`.

**Public items.** No `[ext]` items. `[int]` `pub struct TauriCefChildLifeSpanHandler<T: UserEvent>`
(`:83-94`, `wrap_with_args!` with `TauriCefChildLifeSpanHandlerArgs`). Private:
`PopupClientFactory` (`:19-20`), `check_and_reload_if_blank` (`:52`).

**Behavioural rules.**

- `check_and_reload_if_blank` — a workaround for a CEF race where a network-service crash leaves the
  browser on a blank page. Skipped for `about:blank`. Spawns a thread that waits 1 s, then for up to 5 s
  at 100 ms intervals reloads `initial_url` whenever the main frame's URL is empty or `about:blank`, and
  returns as soon as it sees any other URL.
- `on_after_created` — for a popup (`opener` set): send `Message::PopupCreated(opener, browser_id, family)`,
  wake the event loop, and call `family.created(...)`; when the family is gone, close the browser
  (`host.close_browser(1)`). For a non-popup with an `initial_url`, start `check_and_reload_if_blank`.
- `on_before_popup` — force `window_info.{windowless_rendering_enabled, shared_texture_enabled, external_begin_frame_enabled} = 0`;
  build `NewWindowFeatures` from `popup_features` (`width_set && height_set` → `LogicalSize`,
  `x_set && y_set` → `LogicalPosition`; `window.open` features are CSS pixels = logical units) and the
  opener's main-frame URL as `NewWindowOpener`; call the application's `NewWindowHandler`, defaulting to
  `Allow` when there is none; an unparseable target URL cancels (`1`). Then:
  - `Allow` → reserve a `PopupRequest` in the family (`family.reserve(frame_navigation_state, browser_id, popup_id)`),
    send `Message::PopupPending(request, family)` (a send failure aborts the reservation and cancels),
    wake the event loop, replace `*client` with the popup client from `create_popup`, and return `0` so
    CEF keeps its popup creation and JavaScript opener relationship — only the client changes.
  - `Create { window_id }` → CEF cannot transplant a popup's contents into an existing browser, so cancel
    the popup and send `Message::NavigateFirstWebview { window_id, url }`; return `1`. Note
    `window.opener` is not linked to the new document.
  - `Deny` → `1`.
- `on_before_popup_aborted` — only for an owned browser; `family.abort(...)`; on a reservation, send
  `Message::PopupAborted(request)` and wake.
- `do_close` — `0` for a popup or a browser with no host; `0` when window rendering is disabled
  (offscreen: nothing to destroy); on macOS and Windows send `Message::DestroyWebviewHostWindow(webview_id)`,
  wake, and return `1` (CEF's default would send the close notification to the browser's *top-level
  parent window*, turning "close this webview" into "close the window and every sibling webview");
  elsewhere (`Linux`) return `0`, because the default only closes the browser's own X11 child window.
- `on_before_close` — `family.closed(...)`; for a popup with an `opener`, send
  `Message::PopupClosed(browser_id)` and wake; otherwise send `Message::BrowserClosed(window_id, webview_id)`
  and wake.

**Electron feasibility.** `emulated`. `webview.newWindow` → the `NewWindowHandler` call;
`webview.newWindowResponse { id, response: NewWindowDecision { action, window_id } }` → the three
outcomes (`Allow`, `Create` via `window_id`, `Deny`); `webview.closed` → `on_before_close`;
`window.closeRequested`/`window.closed` → `do_close`. The `check_and_reload_if_blank` workaround is
`absent` (it exists for a CEF network-service race; Electron has no equivalent failure mode — do not
port it). The popup family reservation protocol has no Electron counterpart and must be replaced by the
bridge's own popup lifetime bookkeeping.

### 10.15 `cef_impl/client/load.rs` (101 lines)

**Purpose.** `LoadHandler`: loading-state, document-committed, navigation-failed and page-load events.

**Public items.** No `[ext]` items. `[int]` `pub struct TauriCefLoadHandler { on_page_load_handler: …, frame_event_handler: … }`
(`:10-13`).

**Behavioural rules.** `on_loading_state_change` → `FrameEventKind::LoadingStateChanged { is_loading }`
with the **main frame**; `on_load_start` → `FrameEventKind::DocumentCommitted { url }` for every frame,
plus `PageLoadEvent::Started` for the main frame only; `on_load_error` → `FrameEventKind::NavigationFailed { url }`
when the failed URL parses; `on_load_end` → `PageLoadEvent::Finished` for the main frame only.

**Electron feasibility.** `direct`. `webview.loadStarted` → `Started` + `DocumentCommitted`;
`webview.loadFinished` → `Finished`; `webview.loadFailed` → `NavigationFailed` (note `electron-rs`
already filters `code === -3`, `ERR_ABORTED`); `webview.navigationCommitted` → `DocumentCommitted`.
Loading-state changes have no separate event and can be derived from `loadStarted`/`loadFinished`.

### 10.16 `cef_impl/client/permission.rs` (475 lines)

**Purpose.** The application's permission policy, as CEF asks for it. CEF has two entry points that
behave very differently once answered: `on_show_permission_prompt` is reached only while Chromium's
stored content setting still says "ask", and `cont` persists the answer into the on-disk profile, so
Chromium never asks again — a handler whose answer depends on application state is ignored from its
second request onwards, including across restarts. `on_request_media_access_permission` is reached on
every `getUserMedia()` call, so a changing answer is honoured, but `MediaAccessCallback` persists
nothing, which is why this path writes the content settings itself. Unmapped request types are
`PermissionKind::Other`, deliberately failing closed.

**Public items.** No `[ext]` items. `[int]` `pub(crate) type PermissionRequestHandler = dyn Fn(PermissionKind) -> PermissionResponse + Send + Sync`
(`:61-62`), `[int]` `pub struct TauriCefPermissionHandler { permission_request_handler: Option<Arc<PermissionRequestHandler>> }`
(`:183-185`). Private: `PERMISSION_KINDS` (15 entries, `:90-151`), `MEDIA_PERMISSION_KINDS` (4, `:157-162`),
`AppDecision` (`:172-180`), `decide` (`:324`), `grants_desktop_capture` (`:371`),
`requested_permissions` (`:376`), `permission_kind`/`media_permission_kind`/`lookup_permission_kind`
(`:383-398`), `media_content_settings` (`:404`), `browser_host` (`:416`), `is_alloy_style` (`:425`),
`allow_content_settings` (`:454`), and the media bit constants (`:64-77`).

**Behavioural rules.**

- **Kind mapping.** `PERMISSION_KINDS` maps `CEF_PERMISSION_TYPE_{CAMERA_PAN_TILT_ZOOM, CAMERA_STREAM}`
  → `Camera`, `MIC_STREAM` → `Microphone`, `CAPTURED_SURFACE_CONTROL` → `DisplayCapture`,
  `GEOLOCATION` → `Geolocation`, `NOTIFICATIONS` → `Notifications`, `CLIPBOARD` → `ClipboardRead`,
  `MIDI_SYSEX` → `Midi`, `SENSORS` → `Sensors`, `LOCAL_FONTS` → `LocalFonts`,
  `WINDOW_MANAGEMENT` → `WindowManagement`, `POINTER_LOCK` → `PointerLock`,
  `MULTIPLE_DOWNLOADS` → `AutomaticDownloads`, `FILE_SYSTEM_ACCESS` → `FileSystemAccess`,
  `PROTECTED_MEDIA_IDENTIFIER` → `MediaKeySystemAccess`. Everything else — storage access, FedCM,
  protocol handler registration, idle detection, local and loopback network access, web app
  installation, the WebXR sessions, hand tracking, keyboard lock, disk quota, and any type a future CEF
  adds — is `PermissionKind::Other`. `PermissionKind::Autoplay` has no entry (Chromium gates autoplay
  through its media engagement policy, not a permission request).
  `MEDIA_PERMISSION_KINDS` maps audio capture → `Microphone`, video capture → `Camera`, and both desktop
  capture bits → `DisplayCapture`.
- **`decide(requested, kind)`** — no handler → `NoOpinion`. Otherwise iterate the set bits of the
  request; a `Deny` **short-circuits immediately** (`Denied`), so a handler that logs or keeps state
  sees only a prefix of a denied request — deliberate, the handler is a policy predicate, not an event
  feed. `Allow` continues; `Default` sets `allowed_all = false`. `asked && allowed_all` → `Allowed`,
  else `NoOpinion`.
- **`on_request_media_access_permission`** — `Denied` → `callback.cont(0)`, return `1`. `Allowed` →
  when `grants_desktop_capture(requested)` return `0` (defer the *whole* request to CEF); otherwise
  `allow_content_settings(host, origin, media_content_settings(requested))`, `callback.cont(requested)`,
  return `1`. `NoOpinion` → Chrome style: return `0` (CEF shows the prompt and records the content
  setting, which is what keeps `enumerateDevices()` from returning a redacted list); Alloy style: return
  `0` when desktop capture is requested, otherwise `allowed = requested & ALLOY_MEDIA_PERMISSIONS`, `0`
  when that is zero, else write the content settings, `callback.cont(allowed)`, return `1`.
- **`on_show_permission_prompt`** — `Denied` → `callback.cont(DENY)`, `1`. `Allowed` →
  `callback.cont(ACCEPT)`, `1`, and **no content setting is written**, because `cont(ACCEPT)` reaches
  `PermissionRequestManager::Accept()` which persists the grant itself, and a secondary wildcard setting
  would be over-broad for anything Chromium scopes to an (embedded origin, top-level site) pair.
  `NoOpinion` → Chrome style: `0`; Alloy style: `callback.cont(ACCEPT)`, `1` (Alloy's default is
  `CEF_PERMISSION_RESULT_IGNORE`, which can leave the page's promise unresolved).
- **`grants_desktop_capture`** — `requested & (DESKTOP_AUDIO_CAPTURE | DESKTOP_VIDEO_CAPTURE) != 0`.
  CEF builds the granted stream straight from the mask: a set `DESKTOP_VIDEO_CAPTURE` bit with no
  requested device id synthesises a `DesktopMediaID(TYPE_SCREEN, kFullDesktopScreenId)` and returns it,
  so `getDisplayMedia()` resolves with the whole desktop and **no picker at all**. A `PermissionKind`
  names no screen, window or tab, so an app writing `.on_permission_request(|_| Allow)` would silently
  give any page a full-desktop stream. The whole request is deferred, never part of it: CEF requires
  `allowed_permissions == required_permissions` for a request carrying the device capture bits.
- **`allow_content_settings`** — no-op when the origin is unknown (a `set_content_setting` with no URL
  changes the default for *every* origin) or the settings list is empty, or when CEF handed out no host.
  Writes `MEDIASTREAM_MIC` for audio and `MEDIASTREAM_CAMERA` for video, both `ALLOW`, scoped to the
  requesting origin. The setting outlives the answer that wrote it: a later `Deny` still refuses the
  stream, but `navigator.permissions.query()` keeps reporting `granted` and `enumerateDevices()` keeps
  returning unredacted labels; an application that revokes access for good must rewrite the setting.
- **`is_alloy_style`** — `host.runtime_style() == RuntimeStyle::ALLOY`; a missing host reports `false`,
  deferring to CEF, which is correct for the Chrome-style webviews Tauri uses by default.

**Electron feasibility.** `emulated`.

| reference responsibility | Electron mechanism |
| --- | --- |
| `on_show_permission_prompt` | `session.setPermissionRequestHandler` + `permission.requested` / `webview.permissionResponse` with `PermissionDecision::{Allow, Deny, Default}` — the three `AppDecision` outcomes map directly |
| `on_request_media_access_permission` | the same handler (Electron routes `getUserMedia` through `setPermissionRequestHandler` with `media`) |
| `PERMISSION_KINDS` table | Electron's permission strings (`media`, `geolocation`, `notifications`, `clipboard-read`, `midiSysex`, `sensors`, `local-fonts`, `window-management`, `pointerLock`, `fullscreen`, `openExternal`, …); `PermissionKind::Other` for anything unmapped |
| `grants_desktop_capture` carve-out | `absent` — Electron routes `getDisplayMedia` through `session.setDisplayMediaRequestHandler`, a separate handler that names the source. Keep the rule (never grant desktop capture from a `PermissionKind` answer) and implement it by *not* answering display-media requests from the permission handler. |
| `allow_content_settings` | `absent` — Electron persists the permission decision itself; there is no content-setting store to write. |
| `is_alloy_style` | `absent` — no runtime styles. |

### 10.17 `cef_impl/client/process.rs` (72 lines)

**Purpose.** `BrowserProcessHandler`: context-initialized signalling, the external-pump callback, the
CEF API-version switch for child processes, and deep-link forwarding for a second launch.

**Public items.** No `[ext]` items. `[int]` `pub(crate) struct TauriCefBrowserProcessHandler<T: UserEvent>`
(`:16-22`).

**Behavioural rules.**

- `on_context_initialized` — set the shared `context_initialized` flag and wake the event loop. The
  runtime's startup blocks on this flag (`runtime.rs:3548-3551`).
- `on_schedule_message_pump_work(delay)` — forward into `context.cef_pump` and wake the event loop.
- `on_before_child_process_launch` — append `API_VERSION_SWITCH` with the overridden CEF API version, so
  a child declares its version before any other CEF call (a child never sees the `Cef` builder).
- `on_already_running_app_relaunch` — parse the first argument as a URL; when its scheme is in
  `deep_link_schemes`, send `Message::Opened(vec![url])`; always return `1`.

**Electron feasibility.** `emulated` for the first and last, `absent` for the middle two.
`on_context_initialized` → the bridge handshake (`{"event":"ready"}`); `on_schedule_message_pump_work` →
`absent` (see §9.5); `on_before_child_process_launch` → `absent` (Electron launches its own children);
`on_already_running_app_relaunch` → the runtime's own argv handling plus Electron's
`app.requestSingleInstanceLock` / `second-instance`, or the bridge's `app.reopen` event.

---

## 11. `build.rs` (17 lines)

### 11.1 Purpose

Emits the two `cargo:` keys that let a dependent crate's build script find the CEF runtime and its
binary distribution, and nothing else.

### 11.2 Public items

No Rust items; the file's entire surface is its `cargo:` output:

```rust
fn main() {
  println!("cargo:rerun-if-changed=build.rs");
  // exposed to the build scripts of crates depending on this one as `DEP_TAURI_RUNTIME_CEF_RUNTIME`,
  // which is how `tauri-build` detects that the application uses the CEF runtime.
  println!("cargo:runtime=cef");
  // The CEF binary distribution `cef-dll-sys` resolved for this build, downloading it when it
  // was missing, passed on to the build scripts of crates depending on this one as
  // `DEP_TAURI_RUNTIME_CEF_CEF_DIR`: where the framework, `libcef` and the resources are.
  println!("cargo:rerun-if-env-changed=DEP_CEF_DLL_WRAPPER_CEF_DIR");
  if let Ok(cef_dir) = std::env::var("DEP_CEF_DLL_WRAPPER_CEF_DIR") {
    println!("cargo:cef_dir={cef_dir}");
  }
}
```

### 11.3 Behavioural rules

- `cargo:rerun-if-changed=build.rs` — the script reruns when it changes.
- `cargo:runtime=cef` — becomes `DEP_TAURI_RUNTIME_CEF_RUNTIME` in a dependent's build script. `tauri-build`
  reads it to detect which runtime the application uses. The value `cef` is load-bearing.
- `cargo:rerun-if-env-changed=DEP_CEF_DLL_WRAPPER_CEF_DIR` — rerun when the CEF distribution path changes.
- `cargo:cef_dir=<path>` — emitted **only** when the variable is set; becomes
  `DEP_TAURI_RUNTIME_CEF_CEF_DIR` in a dependent's build script.
- The `DEP_*` names exist because `Cargo.toml` declares `links = "tauri_runtime_cef"`; without that key
  Cargo exposes no `DEP_` variables at all.

### 11.4 Electron feasibility

| output | tag | mechanism / reason |
| --- | --- | --- |
| `cargo:rerun-if-changed=build.rs` | `direct` | Unchanged. |
| `cargo:runtime=cef` | `emulated` | The key must still be emitted, but `cef` is the wrong value. Emit `cargo:runtime=electron` and make `tauri-build` recognise it; the `links` key becomes `links = "tauri_runtime_electron"`, so the variable is `DEP_TAURI_RUNTIME_ELECTRON_RUNTIME`. **This is a cross-crate coordination point** — if `tauri-build` only knows `cef`/`wry`, the runtime cannot complete this alone, and the spec's recommendation is to emit `runtime=electron` and raise the `tauri-build` change separately rather than lie with `runtime=cef`. |
| `cargo:cef_dir` | `emulated` | Becomes `cargo:electron_dir=<path>`, sourced from the Electron distribution directory. `electron-rs`'s `ElectronBuilder::build()` already returns a `PreparedDistribution` with `directory` and `binary`, so the target's existing build script has the value in hand. `rerun-if-env-changed` should track whatever variable the distribution path comes from. |

**The target already has a `build.rs`, and it does a different job.** It is 53 lines: it acquires and
brands the Electron distribution (`electron_rs::ElectronBuilder::new(DEFAULT_VERSION)` →
`app_name`/`identifier`/`icon` from the Tauri config → `build()`), prunes foreign distributions, and
emits `cargo:rustc-env=ELECTRON_RS_STAGED_BINARY=…`, `ELECTRON_RS_TAURI_PRODUCT_NAME=…`,
`ELECTRON_RS_TAURI_IDENTIFIER=…`, plus `rerun-if-changed` for `build_support.rs`, the app inputs and
the icon, and `rerun-if-env-changed` for `CARGO_TARGET_DIR`, `INIT_CWD`, `PWD`, `TAURI_CONFIG`.

Parity work here is therefore **additive, not a rewrite**: keep the acquisition logic and add the two
keys the reference emits — `cargo:runtime=electron` and `cargo:electron_dir=<prepared.directory>` —
plus the `links` key in `Cargo.toml` (see §12) that makes them visible to dependents as
`DEP_TAURI_RUNTIME_ELECTRON_RUNTIME` / `DEP_TAURI_RUNTIME_ELECTRON_ELECTRON_DIR`. Do not remove the
existing `rustc-env` keys; the runtime reads `ELECTRON_RS_STAGED_BINARY` to launch.

---

## 12. `Cargo.toml` (133 lines)

### 12.1 Purpose

Package identity, the `links` key that enables `DEP_*` propagation, docs.rs configuration, the
dependency set the modules above need, and the two features.

### 12.2 Public items (package metadata that other crates observe)

- `name = "tauri-runtime-cef"`, `version = "3.0.0-alpha.7"`,
  `description = "Tauri runtime interface for Chromium Embedded Framework"`.
- `links = "tauri_runtime_cef"` — `Cargo.toml:6`, with the comment "lets `tauri-build` detect the CEF
  runtime through the `DEP_TAURI_RUNTIME_CEF_RUNTIME` env var".
- `[package.metadata.docs.rs]` — `features = ["cef/dox", "devtools", "unstable"]`,
  `default-target = "x86_64-unknown-linux-gnu"`, three `targets`. Comment: docs.rs builds without
  network access, `dox` skips downloading the CEF binary distribution.
- `[[test]] name = "macos-application-bootstrap" harness = false`.
- `[features]`:

```toml
[features]
default = []
# also enables devtools on `tauri` so the feature only needs to be enabled on this crate
devtools = ["tauri-runtime/devtools", "tauri/devtools"]
# exposes the extension traits for `tauri::webview::WebviewBuilder`
unstable = ["tauri/unstable"]
```

### 12.3 Behavioural rules

- **`links`** is what makes `DEP_TAURI_RUNTIME_CEF_*` exist for dependents; it must be present and must
  match `build.rs`'s key names.
- **The two features must keep their exact names and their forwarding.** `devtools` also enables
  `tauri/devtools` so the feature only needs enabling on this crate; `unstable` exposes the extension
  traits for `tauri::webview::WebviewBuilder`. `RuntimeInitAttrs`'s `DevToolsPolicy::Auto` reads
  `cfg!(feature = "devtools")` (`runtime.rs:3216`), so the feature name is load-bearing.
- **The dependency set encodes the platform APIs the modules need**, and is the checklist for the port:
  `windows` with `Win32_Globalization` (locale), `objc2-foundation` with `NSLocale` (locale), `libc` on
  macOS (`kill`/`gethostname` for the downgrade singleton check), `gtk4` + `libc` on Linux/BSD (external
  pump + sandbox sysctls), `sha2` + `base64` (request-context profile hash), `http` + `url` (IPC and
  scheme handling), `dirs` (cache path), `serde`/`serde_json` (preferences), `log`.
- **Dependencies that exist only for CEF-specific work** and have no Electron counterpart: `cef`,
  `cef-dll-sys`, `dioxus-debug-cell`, `kuchiki`, `html5ever`, `tauri-utils` with `html-manipulation`
  (the HTML/CSP injection in `request_handler.rs`), `winit` (the event loop the external pump plugs
  into), `softbuffer`, `raw-window-handle`, `objc2-app-kit`/`objc2-quartz-core`/`objc2-application-services`.

### 12.4 Electron feasibility

| item | tag | note |
| --- | --- | --- |
| `name` / `version` / `description` | `direct` | Renamed to the Electron crate; the version should track the reference's `3.0.0-alpha.7` for a 1:1 mirror. |
| `links` | `emulated` | `links = "tauri_runtime_electron"`; must pair with `build.rs` (§11). |
| `[features] devtools`, `unstable` | `direct` | Keep verbatim, including the forwarding. `devtools` is read by `DevToolsPolicy::Auto`. |
| `[package.metadata.docs.rs]` | `emulated` | `cef/dox` disappears; the Electron equivalent is whatever the `electron-rs` build feature is that skips downloading the distribution. Keep `devtools`/`unstable` and the three targets. |
| `[[test]] macos-application-bootstrap` | `absent` | It exercises the CEF macOS helper bootstrap (`tests/macos-application-bootstrap.rs`, out of this scope). Electron has no such helper; drop the entry. |
| dependency set | mixed | Keep `sha2`, `base64`, `http`, `url`, `dirs`, `serde`, `serde_json`, `log`, `raw-window-handle`; keep the `windows`/`objc2-foundation`/`libc` platform features the ported modules still need; drop `cef`, `cef-dll-sys`, `dioxus-debug-cell`, `kuchiki`, `html5ever`, `tauri-utils/html-manipulation`, `winit`, `softbuffer`; add `electron-rs` (dependency **and** build-dependency). |

The target's current `Cargo.toml` diverges in several of these respects: no `links` key, no `[[test]]`,
`electron-rs` pinned to a git rev (`c97c052`) as both dependency and build-dependency, `tokio` instead
of `winit`, `cookie = "0.18"`, `gtk = "0.18"` (the reference uses `gtk4 0.11`), and
`tauri = "3.0.0-alpha.6"` / `tauri-runtime = "3.0.0-alpha.5"` where the reference tracks the workspace
at `3.0.0-alpha.7`. The parity work is to add `links`, add the two `cargo:` keys to the existing
`build.rs` (§11.4), keep the two feature names and their forwarding, align the version numbers with the
reference, and align the platform dependency features with the modules that survive the port.

---

## 13. Cross-cutting: what Electron cannot do at all

Consolidated list, for the module docs and for whoever signs off the support levels.

1. **CEF client handler callbacks.** There is no `Client`, `RequestHandler`, `ResourceHandler`,
   `SchemeHandlerFactory`, `LifeSpanHandler`, `LoadHandler`, `DisplayHandler`, `ContextMenuHandler`,
   `KeyboardHandler`, `CommandHandler`, `PermissionHandler`, `DownloadHandler`, `DragHandler`,
   `FrameHandler`, `RenderProcessHandler` or `BrowserProcessHandler` interface. Every one of them is
   replaced by either an Electron `webContents`/`session` event, a bridge RPC, or nothing.
2. **`external_message_pump`.** Electron owns its message loop; there is no `OnScheduleMessagePumpWork`,
   no `do_message_loop_work`, no wake signal, no reentrancy detection, and no reason for any of them
   (the Rust side and the browser side are separate processes). The whole `external_message_pump/`
   directory is `absent`.
3. **CEF's `SandboxPolicy` granularity.** CEF configures the sandbox per process type (browser, GPU,
   utility, zygote, renderer) and per platform, through a broker pointer plus settings. Electron exposes
   one process-wide switch (`--no-sandbox`) and one per-webview boolean (`webPreferences.sandbox`), and
   the Electron main process (Node) cannot be sandboxed at all.
4. **Native JavaScript dialog observation.** Electron's `JavaScriptDialogManager` never shows a dialog,
   so there is nothing to observe; `Page.javascriptDialogOpening` never fires. `dialog.rs` is `absent`
   end to end.
5. **Chrome-style profiles and their preference store.** No `PreferenceManager`, no `base::Value`
   preferences, no `devtools.availability`, no `session.restore_on_startup`, no autofill/password-manager/
   translate surfaces to disable. `preferences.rs` is almost entirely `absent`.
6. **Asynchronous request-context initialization.** `session.fromPartition` is synchronous, so the whole
   `deferred_init_continuation` / `wait_for_deferred_init` / nestable-tasks protocol is unnecessary and
   `absent`.
7. **CDP message-id ownership.** Electron's `webContents.debugger` numbers its own requests, and only
   one CDP session per `WebContents` is permitted (DevTools open blocks `attach`). The allocator survives
   only for callers that speak CDP themselves.
8. **Init-script injection into served HTML.** Electron injects `init_scripts` itself, so the
   `kuchiki`/`html5ever` rewriting, the CSP hash injection and the 1024-byte encoding-declaration fix are
   `absent` and must not be ported.
9. **CEF's `IDC_*` command table.** Chrome command ids are resolved from IDC names through
   `cef_id_for_command_id_name`; Electron has no such table. The command *policy* survives (as key
   predicates), the id resolution does not.
10. **Out-of-process iframe attach/detach/destroy notifications.** Electron exposes `frame-created` and
    `did-frame-navigate` only; `FrameEventKind::{Attached, Detached, Destroyed}` have no source.
11. **`chrome-sandbox` next to the application binary.** The setuid helper lives inside the Electron
    distribution, not beside the app executable, so the AppImage helper probe must be re-pointed.
12. **`Cef::*` builder methods named in user-facing strings.** `REPLACING_SWITCHES`' second column and
    several warnings name `Cef::disable_features`, `Cef::enable_features`, `Cef::javascript_flags`,
    `Cef::profile_preference`, `Cef::safe_browsing`, `Cef::root_cache_path`, `Cef::command_line_arg`,
    `Cef::cef_api_version`. Every one must be renamed to its `Electron::*` counterpart — a warning that
    tells the user to call a method that does not exist is worse than no warning.

---

## 14. Implementation checklist

Ordered by dependency, so each step compiles against the previous ones.

1. `Cargo.toml`: add `links = "tauri_runtime_electron"`, keep `devtools`/`unstable` verbatim, align the
   platform dependency features.
2. `build.rs`: keep the existing Electron acquisition/branding logic and add the two reference keys —
   `cargo:runtime=electron` and `cargo:electron_dir=<prepared.directory>` — from §11.4.
3. `environment.rs`: port verbatim, then replace `CRASH_REPORTER_VARIABLES` with the Electron list
   (§3.4). `DebugEnvironment` must keep its exact declaration.
4. `locale.rs`: port verbatim; wire `--accept-lang` at the call site (§4.4).
5. `switches.rs`: rewrite to the reference shape (tuple `REPLACING_SWITCHES`, `values: &[String]`,
   `split_list`/`merged_switch_value`, the 31-name `DANGEROUS_SWITCHES`), then re-point the merge target
   at the argv vector (§5.4).
6. `devtools.rs`: port verbatim; keep both allocators and both ranges; change the `Display` string.
7. `sandbox.rs`: port the decision table with `windows_sandbox_unavailable()` returning `false`;
   re-point the helper lookup at the Electron distribution (§7.4).
8. `downgrade.rs`: port verbatim; replace `ChromiumVersion::embedded()` with the bridge-reported
   Chromium version; run before the Electron child is spawned (§2.4).
9. `dialog.rs`: port the four `[ext]` types and `DialogState` verbatim in shape; every body documents and
   returns the "absent" answer (§1.4).
10. `macros.rs`: keep the macro and the export; replace the `$wrap!` forwarding arm (§8.4).
11. `external_message_pump/`: keep the four files; every method a documented no-op (§9.5).
12. `electron_impl/`: create the six module names; implement `cookie.rs`, `ipc.rs`, `request_handler.rs`
    and `client/` against the bridge events listed in §0.3; `preferences.rs` and `request_context.rs`
    are mostly documented absences.
13. Verify: `cargo check` on the three targets, then confirm the three `[ext]` re-export groups in
    `lib.rs` still resolve to the same names and shapes as the reference.
