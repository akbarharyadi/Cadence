//! Decodes one audio file into interleaved f32 frames with Symphonia.
//!
//! 16- and 24-bit PCM fit exactly in f32's 24-bit mantissa, so the conversion itself is lossless.

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

pub struct Source {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    pub rate: u32,
    pub channels: usize,
    pub bits: Option<u32>,
    pub codec: String,
    scratch: Vec<f32>,
}

impl Source {
    pub fn open(path: &Path) -> Result<Self> {
        match Self::open_from(path, None) {
            Err(e) if format!("{e:#}").contains("more than one stream info") => {
                // A FLAC with a repeated STREAMINFO block: read it with the extra copy left out.
                let patched = super::flacfix::PatchedFlac::open(path)?
                    .ok_or_else(|| anyhow!("can't repair {}", path.display()))?;
                log::debug!("reading {} without its duplicate STREAMINFO block", path.display());
                Self::open_from(path, Some(Box::new(patched)))
            }
            other => other,
        }
    }

    fn open_from(path: &Path, source: Option<Box<dyn symphonia::core::io::MediaSource>>) -> Result<Self> {
        let source = match source {
            Some(s) => s,
            None => Box::new(File::open(path).with_context(|| format!("can't open {}", path.display()))?),
        };
        let mss = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .context("unsupported format")?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| anyhow!("no audio track"))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| anyhow!("no audio codec parameters"))?
            .clone();
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .context("unsupported codec")?;
        let codec = symphonia::default::get_codecs()
            .get_audio_decoder(params.codec)
            .map(|d| d.codec.info.short_name.to_uppercase())
            .unwrap_or_else(|| "AUDIO".into());
        Ok(Self {
            track_id: track.id,
            rate: params.sample_rate.unwrap_or(44_100),
            channels: params.channels.as_ref().map(|c| c.count()).unwrap_or(2),
            bits: params.bits_per_sample,
            codec,
            format,
            decoder,
            scratch: Vec::new(),
        })
    }

    /// Decodes the next packet. Returns `None` at the end of the stream.
    /// The rate/channels of a decoded buffer can differ from the header, so they're returned with the samples.
    pub fn next_chunk(&mut self) -> Result<Option<(&[f32], u32, usize)>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return Ok(None),
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(None);
                }
                Err(SymError::ResetRequired) => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(buf) => {
                    let spec = buf.spec();
                    let (rate, channels) = (spec.rate(), spec.channels().count());
                    self.scratch.resize(buf.samples_interleaved(), 0.0);
                    buf.copy_to_slice_interleaved(&mut self.scratch[..]);
                    if self.scratch.is_empty() {
                        continue;
                    }
                    return Ok(Some((&self.scratch, rate, channels)));
                }
                // Corrupt packet: skip it rather than stopping the song.
                Err(SymError::DecodeError(_)) | Err(SymError::IoError(_)) => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Seeks to `secs` and returns the position actually reached.
    pub fn seek(&mut self, secs: f64) -> Result<f64> {
        let time = Time::try_from_secs_f64(secs.max(0.0)).unwrap_or(Time::ZERO);
        let seeked = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .context("seek failed")?;
        self.decoder.reset();
        let reached = self
            .format
            .tracks()
            .iter()
            .find(|t| t.id == self.track_id)
            .and_then(|t| t.time_base)
            .and_then(|tb| tb.calc_time(seeked.actual_ts))
            .map(|t| t.as_secs_f64())
            .unwrap_or(secs);
        Ok(reached)
    }
}
