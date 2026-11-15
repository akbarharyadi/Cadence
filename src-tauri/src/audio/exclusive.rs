//! WASAPI exclusive mode: Cadence takes the device directly, bypassing the Windows mixer, and runs it
//! at the song's own sample rate. With no processing on, every sample reaches the DAC unchanged.
//!
//! A dedicated thread pulls from the same [`Renderer`] as shared mode and converts to the device's
//! integer format. It registers itself with MMCSS as "Pro Audio" (Windows doesn't do that for an
//! app's own thread, event-driven or not), so a busy system doesn't starve it of time.
//!
//! The device is the one the engine names (the chosen one, or the Windows default looked up by id);
//! following the default when it changes is the engine's (see `devices::watch`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use anyhow::{Result, anyhow};
use ringbuf::traits::Split;
use ringbuf::{HeapProd, HeapRb};
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

use super::output::{Output, Renderer, Shared};
use super::pcm::{Dither, Format, encode};

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

// avrt.dll (MMCSS), part of every Windows since Vista; avrt.lib comes with the Windows SDK. Declared
// here rather than through the `windows` crate, which Cadence doesn't depend on directly.
#[link(name = "avrt")]
unsafe extern "system" {
    fn AvSetMmThreadCharacteristicsW(task_name: *const u16, task_index: *mut u32) -> isize;
    fn AvRevertMmThreadCharacteristics(handle: isize) -> i32;
}

/// The calling thread's MMCSS registration ("Pro Audio"); undone when dropped.
struct Mmcss(isize);

impl Mmcss {
    fn pro_audio() -> Option<Self> {
        let name: Vec<u16> = "Pro Audio".encode_utf16().chain(std::iter::once(0)).collect();
        let mut index = 0u32;
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call; `index` is a
        // valid out pointer.
        let handle = unsafe { AvSetMmThreadCharacteristicsW(name.as_ptr(), &mut index) };
        if handle == 0 {
            log::warn!("couldn't register the exclusive stream with MMCSS");
            None
        } else {
            Some(Self(handle))
        }
    }
}

impl Drop for Mmcss {
    fn drop(&mut self) {
        // SAFETY: the handle came from AvSetMmThreadCharacteristicsW on this same thread (the
        // value lives and is dropped inside `run`).
        unsafe {
            AvRevertMmThreadCharacteristics(self.0);
        }
    }
}

/// What the stream thread reports once the device is running.
struct Ready {
    device: String,
    channels: usize,
    format: Format,
    prod: HeapProd<f32>,
}

/// Opens a device exclusively at `rate`: the one with this id (see `devices`), or the Windows
/// default. Fails (so the caller can fall back to shared mode) when the device is busy, exclusive
/// mode is disallowed, or the rate isn't supported.
pub fn open(shared: Arc<Shared>, rate: u32, device: Option<&str>) -> Result<Output> {
    let device = device.map(|id| super::devices::endpoint_id(id).to_string());
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<Ready>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread = std::thread::Builder::new()
        .name("cadence-exclusive".into())
        .spawn(move || {
            if let Err(e) = run(shared, rate, device, thread_stop, &ready_tx) {
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
        Err(_) => {
            // The thread carries on by itself: it must not take the device later (and render
            // beside the stream Cadence falls back to). It stops at its next check.
            stop.store(true, Ordering::Release);
            return Err(anyhow!("the device didn't respond"));
        }
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
    device: Option<String>,
    stop: Arc<AtomicBool>,
    ready: &crossbeam_channel::Sender<Result<Ready>>,
) -> Result<()> {
    let _ = wasapi::initialize_mta();
    let enumerator = DeviceEnumerator::new()?;
    let device = match &device {
        Some(id) => enumerator.get_device(id)?,
        None => enumerator.get_default_device(&Direction::Render)?,
    };
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
    // Given up on (`open` timed out): the device isn't taken.
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
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
    let mut renderer = Renderer::new(cons, shared.clone(), channels, rate, true).with_bits(format.valid_bits as u32);
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
    let handed = ready.send(Ok(Ready {
        device: name,
        channels,
        format,
        prod,
    }));
    // Given up on meanwhile: the device is let go without a sample.
    if handed.is_err() || stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let want = rate as usize * 15 / 100; // 150 ms
    let waited = std::time::Instant::now();
    while renderer.buffered_frames() < want
        && waited.elapsed() < std::time::Duration::from_millis(600)
        && !stop.load(Ordering::Acquire)
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let _mmcss = Mmcss::pro_audio();
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
