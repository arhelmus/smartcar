//! Video configuration types.

/// Configuration parameters for the Android Auto video channel.
///
/// Passed to [`crate::VideoService::new`] before connection setup begins.
/// The values are used to build the `AVChannel` descriptor advertised in the
/// `ServiceDiscoveryResponse`.
#[derive(Debug, Clone)]
pub struct VideoConfig {
    /// Horizontal resolution in pixels (default: 1280).
    pub width: u32,
    /// Vertical resolution in pixels (default: 720).
    pub height: u32,
    /// Target frame rate (default: 30).
    pub fps: u32,
    /// Display margin in pixels (default: 0).
    pub margin: u32,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fps: 30,
            margin: 0,
        }
    }
}
