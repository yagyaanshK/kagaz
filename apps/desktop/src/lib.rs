//! The app as a library: Android and iOS load this and call `run`. It is
//! the same code as the desktop program; this file only includes main.rs.

// main.rs carries the desktop's crate-level `windows_subsystem` setting,
// which means nothing inside a module.
#![allow(unused_attributes)]

#[path = "main.rs"]
mod app;

pub use app::run;
