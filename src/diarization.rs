//! Compatibility facade for diarization adapters now owned by `margins-media`.

pub use margins_media::diarization::*;
pub use margins_media::providers::PublicDiarizationBackend;

pub trait DiarizationBackend {
    fn diarize(&mut self, mono_16k: &[f32]) -> anyhow::Result<Vec<SpeakerSegment>>;
}

#[cfg(feature = "polyvoice-diarization")]
pub mod polyvoice_backend {
    pub use margins_media::providers::polyvoice::PolyvoiceDiarization;
}

#[cfg(feature = "polyvoice-diarization")]
impl DiarizationBackend for polyvoice_backend::PolyvoiceDiarization {
    fn diarize(&mut self, mono_16k: &[f32]) -> anyhow::Result<Vec<SpeakerSegment>> {
        self.diarize_pcm(mono_16k)
    }
}
