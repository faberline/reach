//! macOS adapter: Core Graphics window inventory, ImageIO/CommonCrypto
//! helpers, the Swift-helper backed `MacBackend`, and the capture/stream tools.
pub mod backend;
pub(crate) mod capture_lock;
pub(crate) mod codec;
pub mod stream_image;
pub mod stream_status;
mod windows;
