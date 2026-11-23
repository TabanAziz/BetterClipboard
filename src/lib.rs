//! BetterClipboard: a fast, native clipboard history for Windows 11.

// Window, font and device-context handles are raw pointers in windows-sys, but they are
// opaque values that Rust code never dereferences.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod app;
pub mod bench;
pub mod clipboard;
pub mod clipfmt;
pub mod image;
pub mod install;
pub mod model;
pub mod paste;
pub mod settings;
pub mod store;
pub mod system;
pub mod tray;
pub mod ui;
pub mod util;
