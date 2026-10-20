//! WASAPI exclusive mode: Cadence takes the device directly, bypassing the Windows mixer, and runs it
//! at the song's own sample rate. With no processing on, every sample reaches the DAC unchanged.
//!
//! A dedicated thread (MMCSS "Pro Audio" priority comes with event-driven exclusive streams) pulls from
//! the same [`Renderer`] as shared mode and converts to the device's integer format.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use anyhow::{Result, anyhow};
use ringbuf::traits::Split;
use ringbuf::{HeapProd, HeapRb};
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

use super::output::{Output, Renderer, Shared};

/// Keeps the exclusive stream alive; dropping it stops playback and releases the device.
pub struct ExclusiveStream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ExclusiveStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Clone, Copy)]
struct Format {
    store_bits: usize,
    valid_bits: usize,
}

/// Best first: 24 bits in a 32-bit container (what most DACs want), 32-bit, packed 24-bit, 16-bit.
const FORMATS: [Format; 4] = [
    Format {
        store_bits: 32,
        valid_bits: 24,
    },
    Format {
        store_bits: 32,
        valid_bits: 32,
    },
    Format {
        store_bits: 24,
        valid_bits: 24,
    },
    Format {
        store_bits: 16,
        valid_bits: 16,
    },
];

/// Opens the default device exclusively at `rate`. Fails (so the caller can fall back to shared mode)
/// when the device is busy, exclusive mode is disallowed, or the rate isn't supported.
/// What the stream thread reports once the device is running.
struct Ready {
    device: String,
    channels: usize,
    format: Format,
    prod: HeapProd<f32>,
}

pub fn open(shared: Arc<Shared>, rate: u32) -> Result<Output> {
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<Ready>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread = std::thread::Builder::new()
        .name("cadence-exclusive".into())
        .spawn(move || {
            if let Err(e) = run(shared, rate, thread_stop, &ready_tx) {
                let _ = ready_tx.try_send(Err(e));
            }
        })?;
    let Ready {
        device,
        channels,
        format,
        prod,
    } = match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            let _ = thread.join();
            return Err(e);
        }
        Err(_) => return Err(anyhow!("the device didn't respond")),
    };
    Ok(Output {
        backend: super::output::Backend::Exclusive(ExclusiveStream {
            stop,
            thread: Some(thread),
        }),
        prod,
        rate,
        channels,
        device,
        bits: Some(format.valid_bits as u32),
    })
}

fn run(
    shared: Arc<Shared>,
    rate: u32,
    stop: Arc<AtomicBool>,
    ready: &crossbeam_channel::Sender<Result<Ready>>,
) -> Result<()> {
    let _ = wasapi::initialize_mta();
    let device = DeviceEnumerator::new()?.get_default_device(&Direction::Render)?;
    let name = device
        .get_friendlyname()
        .unwrap_or_else(|_| "Speakers".into());
    let mut client = device.get_iaudioclient()?;
    let channels = (client.get_mixformat()?.get_nchannels() as usize).clamp(1, 8);

    let (wave, format) = FORMATS
        .iter()
        .find_map(|f| {
            let wanted = WaveFormat::new(
                f.store_bits,
                f.valid_bits,
                &SampleType::Int,
                rate as usize,
                channels,
                None,
            );
            client
                .is_supported_exclusive_with_quirks(&wanted)
                .ok()
                .map(|w| (w, *f))
        })
        .ok_or_else(|| anyhow!("the device can't play {} Hz in exclusive mode", rate))?;

    let (default_period, _min) = client.get_device_period()?;
    let period = client
        .calculate_aligned_period_near(default_period, Some(128), &wave)
        .unwrap_or(default_period);
    let mode = StreamMode::EventsExclusive { period_hns: period };
    if client
        .initialize_client(&wave, &Direction::Render, &mode)
        .is_err()
    {
        // Usually "buffer size not aligned": retry with the size the device asked for (Microsoft's recipe).
        let frames = client.get_buffer_size().unwrap_or(0) as i64;
        let aligned = wasapi::calculate_period_100ns(frames.max(1), rate as i64);
        client = device.get_iaudioclient()?;
        client
            .initialize_client(
                &wave,
                &Direction::Render,
                &StreamMode::EventsExclusive {
                    period_hns: aligned,
                },
            )
            .map_err(|e| anyhow!("exclusive mode unavailable: {e}"))?;
    }
    let event = client.set_get_eventhandle()?;
    let render = client.get_audiorenderclient()?;
    let block = wave.get_blockalign() as usize;

    // Half a second of buffer, as in shared mode.
    let (prod, cons) = HeapRb::<f32>::new((rate as usize / 2) * channels).split();
    let mut renderer = Renderer::new(cons, shared.clone(), channels, rate, true);
    let mut floats: Vec<f32> = Vec::new();
    let mut bytes: Vec<u8> = Vec::new();
    let mut dither = Dither::default();
    let mut fill = |frames: usize, renderer: &mut Renderer| -> Result<()> {
        floats.resize(frames * channels, 0.0);
        renderer.render(&mut floats);
        bytes.resize(frames * block, 0);
        encode(&floats, &mut bytes, format, &mut dither);
        render.write_to_device(frames, &bytes, None)?;
        Ok(())
    };

    // Hand the ring to the worker first and give it a moment to fill it: starting the device on an
    // empty ring plays a short gap at the start of the song (heard as a dip after a rate change).
    let _ = ready.send(Ok(Ready {
        device: name,
        channels,
        format,
        prod,
    }));
    let want = rate as usize * 15 / 100; // 150 ms
    let waited = std::time::Instant::now();
    while renderer.buffered_frames() < want
        && waited.elapsed() < std::time::Duration::from_millis(600)
        && !stop.load(Ordering::Acquire)
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // Prime the device buffer before starting, as Microsoft recommends.
    fill(
        client.get_available_space_in_frames()? as usize,
        &mut renderer,
    )?;
    client.start_stream()?;
    while !stop.load(Ordering::Acquire) {
        if event.wait_for_event(1000).is_err() {
            log::warn!("exclusive stream stalled");
            shared.failed.store(true, Ordering::Release);
            break;
        }
        let frames = client.get_available_space_in_frames()? as usize;
        if let Err(e) = fill(frames, &mut renderer) {
            log::warn!("exclusive stream error: {e}");
            shared.failed.store(true, Ordering::Release);
            break;
        }
    }
    let _ = client.stop_stream();
    Ok(())
}

/// Triangular (TPDF) dither, only for 16-bit output.
#[derive(Default)]
struct Dither {
    state: u32,
}

impl Dither {
    /// A uniform random number in [0, 1) (xorshift: cheap and good enough for dither noise).
    fn uniform(&mut self) -> f32 {
        if self.state == 0 {
            self.state = 0x9E37_79B9;
        }
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state as f32 / u32::MAX as f32
    }

    /// Triangular noise in (-1, 1) LSB: the difference of two uniform values.
    fn next(&mut self) -> f32 {
        self.uniform() - self.uniform()
    }
}

/// Converts float samples to the device's little-endian integer format.
///
/// Symphonia decodes N-bit audio as `value / 2^(N-1)`, so multiplying back gives the exact original
/// integers: a 24-bit file at full volume comes out bit-for-bit identical. Dither is added only to
/// 16-bit output and only when a value isn't already a whole 16-bit number (so 16-bit files stay exact).
fn encode(input: &[f32], out: &mut [u8], f: Format, dither: &mut Dither) {
    match (f.store_bits, f.valid_bits) {
        (32, 24) => {
            for (s, o) in input.iter().zip(out.chunks_exact_mut(4)) {
                let v = (*s as f64 * 8_388_608.0)
                    .round()
                    .clamp(-8_388_608.0, 8_388_607.0) as i32;
                o.copy_from_slice(&(v << 8).to_le_bytes());
            }
        }
        (32, _) => {
            for (s, o) in input.iter().zip(out.chunks_exact_mut(4)) {
                let v = (*s as f64 * 2_147_483_648.0)
                    .round()
                    .clamp(-2_147_483_648.0, 2_147_483_647.0) as i32;
                o.copy_from_slice(&v.to_le_bytes());
            }
        }
        (24, _) => {
            for (s, o) in input.iter().zip(out.chunks_exact_mut(3)) {
                let v = (*s as f64 * 8_388_608.0)
                    .round()
                    .clamp(-8_388_608.0, 8_388_607.0) as i32;
                o.copy_from_slice(&v.to_le_bytes()[..3]);
            }
        }
        _ => {
            for (s, o) in input.iter().zip(out.chunks_exact_mut(2)) {
                let x = *s * 32_768.0;
                let x = if x.fract() != 0.0 {
                    x + dither.next()
                } else {
                    x
                };
                let v = x.round().clamp(-32_768.0, 32_767.0) as i16;
                o.copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_four_bit_is_exact() {
        // Values as Symphonia produces them from 24-bit integers.
        let ints = [0i32, 1, -1, 8_388_607, -8_388_608, 123_456, -654_321];
        let floats: Vec<f32> = ints.iter().map(|&i| i as f32 / 8_388_608.0).collect();
        let mut out = vec![0u8; floats.len() * 4];
        encode(
            &floats,
            &mut out,
            Format {
                store_bits: 32,
                valid_bits: 24,
            },
            &mut Dither::default(),
        );
        for (i, chunk) in ints.iter().zip(out.chunks_exact(4)) {
            assert_eq!(i32::from_le_bytes(chunk.try_into().unwrap()) >> 8, *i);
        }
    }

    #[test]
    fn sixteen_bit_source_gets_no_dither() {
        let ints = [0i16, 1, -1, 32_767, -32_768, 12_345];
        let floats: Vec<f32> = ints.iter().map(|&i| i as f32 / 32_768.0).collect();
        let mut out = vec![0u8; floats.len() * 2];
        encode(
            &floats,
            &mut out,
            Format {
                store_bits: 16,
                valid_bits: 16,
            },
            &mut Dither::default(),
        );
        for (i, chunk) in ints.iter().zip(out.chunks_exact(2)) {
            assert_eq!(i16::from_le_bytes(chunk.try_into().unwrap()), *i);
        }
    }
}
