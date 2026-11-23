// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

/// Marks the application entry point.
///
/// The reference re-exports `tauri_macros::cef_entry_point`, an attribute macro that rewrites
/// `fn main` into a dispatcher for CEF's helper processes: CEF launches renderer, GPU and
/// utility processes by re-executing the application binary with a `--type=` switch, so
/// `main` has to recognise that and hand the process to `CefExecuteProcess` before any Tauri
/// code runs.
///
/// Electron needs no such dispatcher. Its helper processes are launched by the Electron
/// binary itself — the zygote forks them from the distribution's own executable — and the
/// application is a script the main process loads, so there is no re-exec of the application
/// binary to intercept and nothing for `main` to branch on. The macro therefore expands to
/// nothing at all.
///
/// This crate cannot define an attribute macro either: only a `proc-macro` crate can, and
/// `tauri-macros` has no Electron equivalent. The marker is a function-like macro, called as
///
/// ```ignore
/// tauri_runtime_electron::electron_entry_point!();
/// ```
///
/// rather than written as `#[electron_entry_point]` above `fn main`. The name and the place
/// in `lib.rs` are the reference's; the syntax is not, and the difference is the mechanism
/// described above rather than a missing feature.
#[doc(hidden)]
#[macro_export]
macro_rules! electron_entry_point_impl {
  () => {};
}

/// The path form of [`electron_entry_point_impl!`], so `lib.rs` can re-export it under the
/// reference's name.
///
/// A `macro_rules!` macro is only visible outside the crate when it is `#[macro_export]`ed,
/// and that puts it in the crate root's macro namespace, where a second `pub use` of the same
/// name would collide. Exporting under one name and re-exporting under the other keeps both
/// `crate::electron_entry_point!()` and `macros::electron_entry_point` valid.
pub use crate::electron_entry_point_impl as electron_entry_point;

/// Forwards a declaration to one of the reference's handler macros and adds a named-args
/// constructor next to the positional `new()` it generates.
///
/// The reference forwards to `cef`'s `wrap_*!` macros, which generate the C-ABI wrapper types
/// CEF requires. Electron has no such types — a handler here is an ordinary struct with an
/// ordinary inherent `impl` — so there is nothing to forward to. The macro keeps the
/// reference's call-site syntax, with `$wrap` accepted and ignored as a note of which CEF
/// wrapper the struct used to stand in for, and emits the struct, its methods and both
/// constructors directly.
///
/// `new()` takes one argument per struct field, so the wrappers that carry a lot of state are
/// built from a long positional list in which two same-typed neighbours (`devtools_enabled`
/// and `drag_drop_handler_enabled`, say) can be swapped without the compiler noticing.
/// `build()` takes a single args struct instead, so every value is named at the call site, and
/// the forwarding to `new()` is generated from the very field list the wrapper was declared
/// with.
///
/// ```ignore
/// wrap_with_args! {
///   wrap_client => TauriElectronBrowserClientArgs;
///
///   pub(crate) struct TauriElectronBrowserClient<T: UserEvent> {
///     pub(crate) context: RuntimeContext<T>,
///     devtools_enabled: bool,
///   }
///
///   impl Client {
///     // ... the handler's methods
///   }
/// }
///
/// TauriElectronBrowserClient::build(TauriElectronBrowserClientArgs {
///   context,
///   devtools_enabled: true,
/// });
/// ```
///
/// The args fields are `pub(crate)` whatever visibility the wrapper gives its own fields:
/// anything that can name the args struct has to be able to fill in every one of them, while
/// the wrapper keeps its own fields as private as it declared them.
#[allow(unused_macros)]
macro_rules! wrap_with_args {
  (
    $wrap:ident => $args:ident;

    $vis:vis struct $name:ident$(<
      $($generic_type:ident : $first_generic_type_bound:tt $(+ $generic_type_bound:tt)*),+ $(,)?
    >)? {
      $($field_vis:vis $field_name:ident: $field_type:ty),* $(,)?
    }

    impl $interface:ident {
      $($methods:tt)*
    }
  ) => {
    $vis struct $args$(<$($generic_type,)+>)?
    $(where
      $($generic_type: $first_generic_type_bound $(+ $generic_type_bound)*,)+
    )?
    {
      $(pub(crate) $field_name: $field_type,)*
    }

    $vis struct $name$(<$($generic_type: $first_generic_type_bound $(+ $generic_type_bound)*),+>)? {
      $($field_vis $field_name: $field_type,)*
    }

    impl$(<$($generic_type,)+>)? $name$(<$($generic_type,)+>)?
    $(where
      $($generic_type: $first_generic_type_bound $(+ $generic_type_bound)*,)+
    )?
    {
      $($methods)*
    }

    impl$(<$($generic_type,)+>)? $name$(<$($generic_type,)+>)?
    $(where
      $($generic_type: $first_generic_type_bound $(+ $generic_type_bound)*,)+
    )?
    {
      $vis fn new($($field_name: $field_type),*) -> Self {
        Self { $($field_name),* }
      }

      $vis fn build(args: $args$(<$($generic_type,)+>)?) -> Self {
        Self::new($(args.$field_name),*)
      }
    }
  };
}

#[allow(unused_imports)]
pub(crate) use wrap_with_args;
