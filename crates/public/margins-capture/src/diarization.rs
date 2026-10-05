//! Compatibility facade for diarization adapters now owned by `margins-media`.

pub use margins_media::diarization::*;
pub use margins_media::providers::PublicDiarizationBackend;

#[cfg(feature = "polyvoice-diarization")]
pub mod polyvoice_backend {
    pub use margins_media::providers::polyvoice::PolyvoiceDiarization;
}
