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
use symphonia::core::units::{Time, Timestamp};

pub struct Source {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    pub rate: u32,
    pub channels: usize,
    pub bits: Option<u32>,
    pub codec: String,
    /// The length the file's header promises, in frames (when it says).
    pub total_frames: Option<u64>,
    /// Packets that couldn't be decoded and were skipped.
    pub errors: u32,
    scratch: Vec<f32>,
    /// Frames still to drop from the start of the decoded audio: a seek lands on the packet that
    /// holds the wanted frame, up to a whole packet early.
    skip: u64,
    /// Where in `scratch` the next chunk starts (after a skip, or what `give_back` returned).
    start: usize,
    /// The rest of the last chunk is to be returned again (see `give_back`).
    replay: bool,
    last_format: (u32, usize),
}

impl Source {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with(path, false)
    }

    /// Opens a file to check it: the decoder verifies the audio against the checksum in the file
    /// where it has one (FLAC), see `finalize`.
    pub fn open_verified(path: &Path) -> Result<Self> {
        Self::open_with(path, true)
    }

    fn open_with(path: &Path, verify: bool) -> Result<Self> {
        match Self::open_from(path, None, verify) {
            Err(e) if format!("{e:#}").contains("more than one stream info") => {
                // A FLAC with a repeated STREAMINFO block: read it with the extra copy left out.
                let patched = super::flacfix::PatchedFlac::open(path)?
                    .ok_or_else(|| anyhow!("can't repair {}", path.display()))?;
                log::debug!("reading {} without its duplicate STREAMINFO block", path.display());
                Self::open_from(path, Some(Box::new(patched)), verify)
            }
            other => other,
        }
    }

    /// After decoding everything: whether the audio matched the file's checksum (None: the
    /// format has none, or it wasn't filled in).
    pub fn finalize(&mut self) -> Option<bool> {
        self.decoder.finalize().verify_ok
    }

    fn open_from(path: &Path, source: Option<Box<dyn symphonia::core::io::MediaSource>>, verify: bool) -> Result<Self> {
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
            .make_audio_decoder(&params, &AudioDecoderOptions::default().verify(verify))
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
            total_frames: track.num_frames,
            errors: 0,
            format,
            decoder,
            scratch: Vec::new(),
            skip: 0,
            start: 0,
            replay: false,
            last_format: (0, 0),
        })
    }

    /// Decodes the next packet. Returns `None` at the end of the stream.
    /// The rate/channels of a decoded buffer can differ from the header, so they're returned with the samples.
    pub fn next_chunk(&mut self) -> Result<Option<(&[f32], u32, usize)>> {
        if self.replay {
            self.replay = false;
            if self.start < self.scratch.len() {
                let (rate, channels) = self.last_format;
                return Ok(Some((&self.scratch[self.start..], rate, channels)));
            }
        }
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
                    // Frames before the seek target (the start of the packet it landed in).
                    let frames = (self.scratch.len() / channels.max(1)) as u64;
                    let dropped = self.skip.min(frames);
                    self.skip -= dropped;
                    self.start = dropped as usize * channels.max(1);
                    if self.start >= self.scratch.len() {
                        continue;
                    }
                    self.last_format = (rate, channels);
                    return Ok(Some((&self.scratch[self.start..], rate, channels)));
                }
                // Corrupt packet: skip it rather than stopping the song.
                Err(SymError::DecodeError(_)) | Err(SymError::IoError(_)) => {
                    self.errors += 1;
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Only the first `frames` of the last chunk were used: the next `next_chunk` returns the rest
    /// of it (a song of a CUE sheet ends partway into a packet, and the next one starts there).
    pub fn give_back(&mut self, frames: usize) {
        let ch = self.last_format.1.max(1);
        self.start = (self.start + frames * ch).min(self.scratch.len());
        self.replay = self.start < self.scratch.len();
    }

    /// Seeks to `secs` and returns the position actually reached.
    pub fn seek(&mut self, secs: f64) -> Result<f64> {
        let frame = (secs.max(0.0) * f64::from(self.rate)).round() as u64;
        let reached = self.seek_frame(frame)?;
        Ok(reached as f64 / f64::from(self.rate))
    }

    /// Seeks to exactly `frame` (counted at the file's own rate) and returns the frame reached:
    /// `frame` itself whenever the format can find it. The demuxer lands on the start of the
    /// packet holding it (FLAC frame, WAV block), so the frames in between are decoded and dropped.
    pub fn seek_frame(&mut self, frame: u64) -> Result<u64> {
        let rate = u64::from(self.rate.max(1));
        let time_base = self
            .format
            .tracks()
            .iter()
            .find(|t| t.id == self.track_id)
            .and_then(|t| t.time_base);
        // Frames are the timestamps when the time base is one sample (FLAC, WAV, most PCM):
        // exact, no detour through seconds.
        let exact = time_base.is_some_and(|tb| tb.numer.get() == 1 && u64::from(tb.denom.get()) == rate);
        let to = if exact {
            SeekTo::Timestamp {
                ts: Timestamp::new(frame as i64),
                track_id: self.track_id,
            }
        } else {
            SeekTo::Time {
                time: Time::try_from_secs_f64(frame as f64 / rate as f64).unwrap_or(Time::ZERO),
                track_id: Some(self.track_id),
            }
        };
        let seeked = self.format.seek(SeekMode::Accurate, to).context("seek failed")?;
        self.decoder.reset();
        self.replay = false;
        self.start = 0;
        let to_frame = |ts: Timestamp| -> Option<u64> {
            if exact {
                return u64::try_from(ts.get()).ok();
            }
            let t = time_base?.calc_time(ts)?;
            Some((t.as_secs_f64() * rate as f64).round().max(0.0) as u64)
        };
        let actual = to_frame(seeked.actual_ts).unwrap_or(frame);
        let required = to_frame(seeked.required_ts).unwrap_or(frame);
        // Never more than a few seconds (a broken index would otherwise mean a long silent decode).
        self.skip = required.saturating_sub(actual).min(rate * 10);
        Ok(actual + self.skip)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Writes a 16-bit stereo 44.1 kHz WAV whose samples say where they are: left = frame % 30000,
    /// right = its negative.
    pub(crate) fn position_wav(path: &Path, frames: u32) {
        let mut data = Vec::with_capacity(frames as usize * 4);
        for i in 0..frames {
            let v = (i % 30_000) as i16;
            data.extend_from_slice(&v.to_le_bytes());
            data.extend_from_slice(&(-v).to_le_bytes());
        }
        let mut f = Vec::new();
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        f.extend_from_slice(b"WAVEfmt ");
        f.extend_from_slice(&16u32.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes()); // PCM
        f.extend_from_slice(&2u16.to_le_bytes());
        f.extend_from_slice(&44_100u32.to_le_bytes());
        f.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
        f.extend_from_slice(&4u16.to_le_bytes());
        f.extend_from_slice(&16u16.to_le_bytes());
        f.extend_from_slice(b"data");
        f.extend_from_slice(&(data.len() as u32).to_le_bytes());
        f.extend_from_slice(&data);
        std::fs::write(path, f).unwrap();
    }

    /// Writes the same signal as `position_wav` as a FLAC (16-bit stereo 44.1 kHz, fixed 4096-frame
    /// blocks with verbatim subframes, no seek table, no MD5), made here so no binary fixture is
    /// needed. `frames` must be a whole number of blocks.
    pub(crate) fn position_flac(path: &Path, frames: u32) {
        const BLOCK: u32 = 4096;
        assert!(frames > 0 && frames.is_multiple_of(BLOCK));
        fn crc8(b: &[u8]) -> u8 {
            b.iter().fold(0u8, |mut c, &x| {
                c ^= x;
                for _ in 0..8 {
                    c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
                }
                c
            })
        }
        fn crc16(b: &[u8]) -> u16 {
            b.iter().fold(0u16, |mut c, &x| {
                c ^= u16::from(x) << 8;
                for _ in 0..8 {
                    c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
                }
                c
            })
        }
        let mut f = b"fLaC".to_vec();
        // STREAMINFO, the last metadata block, 34 bytes.
        f.extend_from_slice(&[0x80, 0, 0, 34]);
        f.extend_from_slice(&(BLOCK as u16).to_be_bytes());
        f.extend_from_slice(&(BLOCK as u16).to_be_bytes());
        f.extend_from_slice(&[0; 6]); // frame sizes unknown
        let packed: u64 = (44_100u64 << 44) | (1 << 41) | (15 << 36) | u64::from(frames);
        f.extend_from_slice(&packed.to_be_bytes());
        f.extend_from_slice(&[0; 16]); // MD5 not set
        for n in 0..frames / BLOCK {
            let start = f.len();
            // Sync + fixed blocking; 4096 frames, 44.1 kHz; stereo, 16 bits.
            f.extend_from_slice(&[0xFF, 0xF8, 0xC9, 0x18]);
            // The frame number, UTF-8 coded.
            if n < 0x80 {
                f.push(n as u8);
            } else {
                f.extend_from_slice(&[0xC0 | (n >> 6) as u8, 0x80 | (n & 0x3F) as u8]);
            }
            let c = crc8(&f[start..]);
            f.push(c);
            for channel in 0..2 {
                f.push(0x02); // verbatim subframe, no wasted bits
                for i in n * BLOCK..(n + 1) * BLOCK {
                    let v = (i % 30_000) as i16;
                    f.extend_from_slice(&(if channel == 0 { v } else { -v }).to_be_bytes());
                }
            }
            let c = crc16(&f[start..]);
            f.extend_from_slice(&c.to_be_bytes());
        }
        std::fs::write(path, f).unwrap();
    }

    #[test]
    fn seeking_a_flac_lands_on_the_exact_frame() {
        let dir = std::env::temp_dir().join(format!("cadence-flac-seek-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("positions.flac");
        position_flac(&path, 20 * 4096);
        let mut s = Source::open(&path).unwrap();
        assert_eq!((s.codec.as_str(), s.rate, s.channels, s.total_frames), ("FLAC", 44_100, 2, Some(20 * 4096)));
        // From the start: frame 0 first.
        assert_eq!(at(s.next_chunk().unwrap().unwrap().0), 0);
        // Mid-block, on a block boundary, one after it, near the end, back near the start.
        for target in [50_000u64, 8_192, 4_097, 81_000, 7] {
            assert_eq!(s.seek_frame(target).unwrap(), target, "seek to {target}");
            let (x, rate, ch) = s.next_chunk().unwrap().unwrap();
            assert_eq!((rate, ch), (44_100, 2));
            assert_eq!(at(x), (target % 30_000) as i64, "seek to {target}");
            assert_eq!((x[1] * 32_768.0).round() as i64, -((target % 30_000) as i64));
            // Decoding carries on from there without a gap.
            let n = (x.len() / 2) as u64;
            if target + n < 20 * 4096 {
                assert_eq!(at(s.next_chunk().unwrap().unwrap().0), ((target + n) % 30_000) as i64, "after {target}");
            }
        }
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The frame number the first sample of `chunk` carries (see `position_wav`).
    fn at(chunk: &[f32]) -> i64 {
        (chunk[0] * 32_768.0).round() as i64
    }

    #[test]
    fn seeking_lands_on_the_exact_frame() {
        let dir = std::env::temp_dir().join(format!("cadence-seek-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("positions.wav");
        position_wav(&path, 100_000);
        let mut s = Source::open(&path).unwrap();
        // WAV packets are 1152 frames: none of these is on a packet boundary, so the demuxer
        // lands up to 1151 frames early.
        for target in [50_000u64, 1_153, 98_999, 7] {
            assert_eq!(s.seek_frame(target).unwrap(), target);
            let (x, rate, ch) = s.next_chunk().unwrap().unwrap();
            assert_eq!((rate, ch), (44_100, 2));
            assert_eq!(at(x), (target % 30_000) as i64, "seek to {target}");
            assert_eq!((x[1] * 32_768.0).round() as i64, -((target % 30_000) as i64));
        }
        // Seconds too (a CUE position): 2.5 s = frame 110 250 of a longer file is out of range
        // here, 1.25 s = 55 125 isn't.
        let reached = s.seek(1.25).unwrap();
        assert_eq!(reached, 1.25);
        assert_eq!(at(s.next_chunk().unwrap().unwrap().0), 55_125 % 30_000);
        // A chunk used only partly hands the rest to the next call.
        s.seek_frame(10_000).unwrap();
        let n = s.next_chunk().unwrap().unwrap().0.len() / 2;
        s.give_back(100);
        let (rest, _, _) = s.next_chunk().unwrap().unwrap();
        assert_eq!(at(rest), 10_100);
        assert_eq!(rest.len() / 2, n - 100);
        // ...and then decoding carries on after it.
        let rest_len = rest.len() / 2;
        assert_eq!(at(s.next_chunk().unwrap().unwrap().0), 10_100 + rest_len as i64);
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
