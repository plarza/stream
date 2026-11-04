//! Decoding local files with symphonia.

use std::sync::OnceLock;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

use crate::audio::{Output, TrackWriter};

/// Symphonia's codecs plus libopus for Opus.
fn codecs() -> &'static CodecRegistry {
    static REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut r = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut r);
        r.register_audio_decoder::<symphonia_adapter_libopus::OpusDecoder>();
        r
    })
}

pub struct OpenTrack {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    pub sample_rate: u32,
}

/// `ext` is the file's extension, a hint for working out its format.
pub fn open(source: Box<dyn MediaSource>, ext: Option<&str>) -> anyhow::Result<OpenTrack> {
    let mss = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = ext {
        hint.with_extension(ext);
    }
    let format = symphonia::default::get_probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())?;
    let track = format.default_track(TrackType::Audio).ok_or_else(|| anyhow::anyhow!("no audio track"))?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| anyhow::anyhow!("not an audio track"))?;
    let sample_rate = params.sample_rate.unwrap_or(48_000);
    let decoder = codecs().make_audio_decoder(params, &AudioDecoderOptions::default())?;
    Ok(OpenTrack { format, decoder, track_id, sample_rate })
}

pub enum Outcome {
    Finished,
    Cancelled,
}

/// Decodes a song from `start_ms` into the output. Blocks until the song is
/// fully written or a jump cancels it.
pub fn play(
    source: Box<dyn MediaSource>,
    ext: Option<&str>,
    start_ms: u32,
    out: std::sync::Arc<Output>,
    generation: u64,
    gain: f32,
) -> anyhow::Result<Outcome> {
    let mut t = open(source, ext)?;
    if start_ms > 0 {
        let _ = t.format.seek(
            SeekMode::Accurate,
            SeekTo::Time { time: Time::from_millis(start_ms as i64), track_id: Some(t.track_id) },
        );
        t.decoder.reset();
    }
    let mut writer = TrackWriter::new(out, generation, t.sample_rate, gain);
    let mut samples: Vec<f32> = Vec::new();
    loop {
        if !writer.is_current() {
            return Ok(Outcome::Cancelled);
        }
        let packet = match t.format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => {
                t.decoder.reset();
                continue;
            }
            Err(e) => {
                log::warn!("read error: {e}");
                break;
            }
        };
        if packet.track_id != t.track_id {
            continue;
        }
        let decoded = match t.decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(e)) => {
                log::debug!("decode error (skipped): {e}");
                continue;
            }
            Err(e) => {
                log::warn!("decode failed: {e}");
                break;
            }
        };
        let channels = decoded.spec().channels().count();
        samples.resize(decoded.samples_interleaved(), 0.0);
        decoded.copy_to_slice_interleaved(&mut samples);
        if !writer.write(&samples, channels) {
            return Ok(Outcome::Cancelled);
        }
    }
    if !writer.finish() {
        return Ok(Outcome::Cancelled);
    }
    Ok(Outcome::Finished)
}

/// Linear gain for ReplayGain, never boosting past the track's peak.
pub fn replaygain(gain_db: Option<f32>, peak: Option<f32>) -> f32 {
    let Some(db) = gain_db else { return 1.0 };
    let mut g = 10f32.powf(db / 20.0);
    if let Some(p) = peak.filter(|p| *p > 0.0) {
        g = g.min(1.0 / p);
    }
    g
}
