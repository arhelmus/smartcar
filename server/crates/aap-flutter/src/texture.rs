//! Shared pixel buffer for the Flutter external texture.
//!
//! The `VideoService` (Tokio task) writes decoded RGBA frames into
//! [`PixelStore`] under a write lock; the Flutter render thread reads from it
//! via the texture frame callback under a read lock.  The two sides never block
//! each other for more than a few microseconds.

use std::sync::Arc;

use parking_lot::RwLock;

/// One decoded video frame held in CPU memory.
///
/// Written by the Tokio video task; read by the Flutter texture frame callback
/// (P1: wire up the OpenGL external texture path).
#[allow(dead_code)]
pub struct PixelStore {
    /// RGBA8888, row-major, `width × height × 4` bytes.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Default for PixelStore {
    fn default() -> Self {
        // 1×1 opaque-black placeholder so the texture is never empty.
        Self {
            rgba: vec![0, 0, 0, 255],
            width: 1,
            height: 1,
        }
    }
}

/// Reference-counted shared pixel store.
pub type SharedPixelStore = Arc<RwLock<PixelStore>>;

/// Create a new [`SharedPixelStore`] initialised with a 1×1 black placeholder.
pub fn new_store() -> SharedPixelStore {
    Arc::new(RwLock::new(PixelStore::default()))
}
