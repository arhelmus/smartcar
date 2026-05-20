//! [`FlutterSink`]: a [`FrameSink`] that decodes H.264 NAL units and pushes
//! the resulting RGBA frames into the Flutter engine's external texture.

use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;

use aap_video::FrameSink;

use crate::decoder::H264Decoder;
use crate::engine::FlutterEngineHandle;
use crate::lib_loader::FlutterLib;
use crate::texture::{self, PixelStore, SharedPixelStore};

/// Decodes Android Auto H.264 video and renders it via Flutter's external
/// texture API.
///
/// # Lifecycle
///
/// Constructed once per connection via [`FlutterSink::new`].  The Flutter
/// engine is started immediately and keeps running until this struct is
/// dropped.  The engine displays whatever is in the shared [`PixelStore`];
/// a `Texture(textureId: 0)` widget in the Flutter app reads it every frame.
pub struct FlutterSink {
    decoder: H264Decoder,
    store: SharedPixelStore,
    engine: FlutterEngineHandle,
}

impl FlutterSink {
    /// Start the Flutter engine and prepare the video pipeline.
    ///
    /// `assets_dir` is the Flutter bundle directory produced by
    /// `flutter build bundle` (contains `flutter_assets/`).
    /// `icu_data` is the path to `icudtl.dat`.
    pub fn new(lib: Arc<FlutterLib>, assets_dir: &Path, icu_data: &Path) -> anyhow::Result<Self> {
        let store = texture::new_store();
        let engine = FlutterEngineHandle::launch(lib, assets_dir, icu_data, store.clone())?;
        let decoder = H264Decoder::new()?;
        Ok(Self {
            decoder,
            store,
            engine,
        })
    }
}

impl FrameSink for FlutterSink {
    fn push_nal(&mut self, _timestamp_us: Option<u64>, data: Bytes) {
        match self.decoder.decode_nal(&data) {
            Ok(Some(frame)) => {
                {
                    let mut store = self.store.write();
                    *store = PixelStore {
                        rgba: frame.rgba,
                        width: frame.width,
                        height: frame.height,
                    };
                }
                self.engine.mark_texture_dirty();
            }
            Ok(None) => {
                // Non-picture NAL (SPS, PPS, …) — engine state updated, no frame output.
            }
            Err(err) => {
                tracing::warn!(%err, "H.264 decode error — NAL dropped");
            }
        }
    }
}
