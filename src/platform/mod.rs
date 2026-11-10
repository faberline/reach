//! The only platform boundary of this crate.
//!
//! `Backend` (crate root) is the platform-neutral contract. Everything that
//! talks to an operating system lives in one adapter module below.
//! macOS is the only adapter today; no other platform has been verified, and
//! no stub adapters are provided. See docs/PLATFORM.md.

#[cfg(target_os = "macos")]
pub mod macos;
