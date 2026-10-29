//! Core library for WineWrap, a CrossOver-style front end for Wine on macOS.
//!
//! The crate is split into small modules that mirror the concepts a user sees:
//!
//! * [`runtime`]: discovering Wine builds installed on the machine
//!   (Homebrew, Gcenx's `Wine *.app` bundles, Apple's Game Porting Toolkit, …).
//! * [`bottle`]: isolated Wine prefixes with their own settings.
//! * [`launch`]: turning a bottle + runtime + program into a process with the
//!   right environment.
//! * [`steam`]: installing and launching Steam and listing installed games.
//! * [`system`]: host health checks (Rosetta 2, architecture, …).
//!
//! Most operations are expressed as [`launch::LaunchSpec`] values first and only
//! executed afterwards, which keeps them testable and lets front ends offer a
//! dry-run mode.

pub mod bottle;
pub mod config;
pub mod download;
pub mod error;
pub mod launch;
pub mod paths;
pub mod runtime;
pub mod steam;
pub mod system;
pub mod vdf;

mod manager;

pub use bottle::{Bottle, BottleConfig, GraphicsBackend, SyncMode, WindowsVersion};
pub use config::AppConfig;
pub use error::{Error, Result};
pub use launch::LaunchSpec;
pub use manager::WineWrap;
pub use paths::AppPaths;
pub use runtime::{RuntimeKind, WineRuntime};
