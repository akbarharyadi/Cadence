//! USB exclusive on Android: Cadence's own USB Audio Class 2 driver, for phones that don't offer
//! Android's bit-perfect USB playback (most of them; many never run a DAC below 48 kHz either).
//!
//! Like USB Audio Player Pro, HiBy Music and FiiO Music's exclusive mode, Cadence takes the DAC over:
//! once the user allows it, Android hands over the USB device (`UsbDac.kt`), Cadence detaches
//! Android's audio driver from it, sets the DAC's clock to the song's own rate and streams the
//! samples itself, exactly as decoded. Android and other apps can't use the DAC meanwhile (their
//! sound moves to the phone's speaker); it's given back when Cadence lets go.
//!
//! It's all done the way libusb does it on Linux: usbfs ioctls on the file Android hands over (the
//! descriptors, the clock's rate, the streaming interface's alternate setting, isochronous
//! transfers). Asynchronous DACs report how fast their own clock takes samples on a feedback
//! endpoint; the packet sizes follow it, so nothing is ever dropped or repeated.

use std::ffi::c_void;
use std::io;
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use ringbuf::HeapRb;
use ringbuf::traits::Split;

use super::android_direct::call;
use super::output::{Backend, Output, Renderer, Shared};
use super::pcm::{Dither, Format, encode};
use super::usb_audio::{Clock, Dac, Pacing, Parsed, parse, pick_rate};

/// Linux usbfs (include/uapi/linux/usbdevice_fs.h).
mod usbfs {
    use std::ffi::c_void;
    use std::io;
    use std::os::fd::RawFd;

    #[repr(C)]
    pub struct CtrlTransfer {
        pub request_type: u8,
        pub request: u8,
        pub value: u16,
        pub index: u16,
        pub length: u16,
        pub timeout: u32,
        pub data: *mut c_void,
    }

    #[repr(C)]
    pub struct SetInterface {
        pub interface: u32,
        pub alt: u32,
    }

    #[repr(C)]
    pub struct IsoPacket {
        pub length: u32,
        pub actual_length: u32,
        pub status: u32,
    }

    /// Followed in memory by `number_of_packets` [`IsoPacket`]s.
    #[repr(C)]
    pub struct Urb {
        pub kind: u8,
        pub endpoint: u8,
        pub status: i32,
        pub flags: u32,
        pub buffer: *mut c_void,
        pub buffer_length: i32,
        pub actual_length: i32,
        pub start_frame: i32,
        pub number_of_packets: i32,
        pub error_count: i32,
        pub signr: u32,
        pub usercontext: *mut c_void,
    }

    #[repr(C)]
    pub struct Ioctl {
        pub ifno: i32,
        pub code: i32,
        pub data: *mut c_void,
    }

    pub const URB_TYPE_ISO: u8 = 0;
    pub const URB_ISO_ASAP: u32 = 0x02;

    const fn ioc(dir: u32, nr: u32, size: usize) -> u32 {
        (dir << 30) | ((size as u32) << 16) | ((b'U' as u32) << 8) | nr
    }
    const NONE: u32 = 0;
    const WRITE: u32 = 1;
    const READ: u32 = 2;
    pub const CONTROL: u32 = ioc(READ | WRITE, 0, size_of::<CtrlTransfer>());
    pub const SETINTERFACE: u32 = ioc(READ, 4, size_of::<SetInterface>());
    pub const SUBMITURB: u32 = ioc(READ, 10, size_of::<Urb>());
    pub const DISCARDURB: u32 = ioc(NONE, 11, 0);
    pub const REAPURB: u32 = ioc(WRITE, 12, size_of::<*mut c_void>());
    pub const REAPURBNDELAY: u32 = ioc(WRITE, 13, size_of::<*mut c_void>());
    pub const CLAIMINTERFACE: u32 = ioc(READ, 15, size_of::<u32>());
    pub const RELEASEINTERFACE: u32 = ioc(READ, 16, size_of::<u32>());
    pub const IOCTL: u32 = ioc(READ | WRITE, 18, size_of::<Ioctl>());
    pub const DISCONNECT: u32 = ioc(NONE, 22, 0);
    pub const CONNECT: u32 = ioc(NONE, 23, 0);
    pub const GET_SPEED: u32 = ioc(NONE, 31, 0);

    /// # Safety
    /// `arg` must be what `request` expects.
    pub unsafe fn ioctl(fd: RawFd, request: u32, arg: *mut c_void) -> io::Result<i32> {
        loop {
            // The request's type differs between C libraries (int on Android, unsigned long on glibc).
            let r = unsafe { libc::ioctl(fd, request as _, arg) };
            if r >= 0 {
                return Ok(r);
            }
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::EINTR) {
                return Err(e);
            }
        }
    }

    /// An ioctl for one interface's kernel driver (detach or re-attach it).
    pub fn driver(fd: RawFd, interface: u8, code: u32) -> io::Result<i32> {
        let mut arg = Ioctl { ifno: interface as i32, code: code as i32, data: std::ptr::null_mut() };
        unsafe { ioctl(fd, IOCTL, (&raw mut arg).cast()) }
    }

    pub fn claim(fd: RawFd, interface: u8, request: u32) -> io::Result<i32> {
        let mut n = interface as u32;
        unsafe { ioctl(fd, request, (&raw mut n).cast()) }
    }

    pub fn set_interface(fd: RawFd, interface: u8, alt: u8) -> io::Result<i32> {
        let mut arg = SetInterface { interface: interface as u32, alt: alt as u32 };
        unsafe { ioctl(fd, SETINTERFACE, (&raw mut arg).cast()) }
    }

    /// A control request; returns the bytes transferred.
    pub fn control(fd: RawFd, request_type: u8, request: u8, value: u16, index: u16, data: &mut [u8]) -> io::Result<usize> {
        let mut arg = CtrlTransfer {
            request_type,
            request,
            value,
            index,
            length: data.len() as u16,
            timeout: 1000,
            data: data.as_mut_ptr().cast(),
        };
        unsafe { ioctl(fd, CONTROL, (&raw mut arg).cast()) }.map(|n| n as usize)
    }
}

// USB Audio Class 2 control requests (the clock's rate).
const CUR: u8 = 0x01;
const RANGE: u8 = 0x02;
const SAM_FREQ: u16 = 0x0100;
const SELECTOR: u16 = 0x0100;
/// Class request to an interface, device to host / host to device.
const GET: u8 = 0xA1;
const SET: u8 = 0x21;

/// The DAC while Cadence holds it, also between songs: a song at another rate reopens the stream
/// right away, and handing the DAC back to Android in between would be heard.
struct Held {
    fd: RawFd,
    name: String,
    dac: Dac,
}

static HELD: Mutex<Option<Held>> = Mutex::new(None);
/// Counts opens, so a delayed release knows whether the DAC was taken again meanwhile.
static OPENS: AtomicU64 = AtomicU64::new(0);
/// Cadence asked the user to allow it to use the DAC and is waiting for the answer.
static WAITING: AtomicBool = AtomicBool::new(false);
static LAST_ASKED: Mutex<Option<Instant>> = Mutex::new(None);
/// The DAC's volume Cadence applies while it drives it (Android's volume doesn't reach the DAC
/// then), as a factor (f32 bits): it starts where Android's was, and the volume keys move it.
static GAIN: AtomicU32 = AtomicU32::new(0x3F80_0000);
static LAST_SYNC: Mutex<Option<Instant>> = Mutex::new(None);

fn fetch_gain() {
    if let Ok(g) = call("UsbDac", "gain", 0)
        && let Ok(g) = g.parse::<f32>()
    {
        GAIN.store(g.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

/// Keeps the DAC's volume in step with the volume keys. Called often by the engine while Cadence
/// drives the DAC; asks the app at most every 100 ms.
pub fn sync_volume() {
    let mut last = LAST_SYNC.lock().unwrap();
    if last.is_some_and(|t| t.elapsed() < Duration::from_millis(100)) {
        return;
    }
    *last = Some(Instant::now());
    drop(last);
    fetch_gain();
}

/// The DAC's volume in dB while it's below full (None at full: nothing changes the samples).
pub fn volume_db() -> Option<f32> {
    let g = f32::from_bits(GAIN.load(Ordering::Relaxed));
    (g < 1.0).then(|| if g > 0.0 { 20.0 * g.log10() } else { -120.0 })
}

/// Whether the user just allowed Cadence to use the DAC (it asked for that while opening). Checked
/// often by the engine; asks the app at most once a second, and only while waiting.
pub fn allowed() -> bool {
    if !WAITING.load(Ordering::Relaxed) {
        return false;
    }
    {
        let mut last = LAST_ASKED.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return false;
        }
        *last = Some(Instant::now());
    }
    match call("UsbDac", "takeAnswer", 0).as_deref() {
        Ok("1") => {
            WAITING.store(false, Ordering::Relaxed);
            true
        }
        Ok("0") => {
            WAITING.store(false, Ordering::Relaxed);
            false
        }
        _ => false,
    }
}

/// Plays to the USB DAC through Cadence's own driver at `rate` (or the nearest rate the DAC has).
pub fn open(shared: Arc<Shared>, rate: u32) -> Result<Output> {
    let mut held = HELD.lock().unwrap();
    if held.is_none() {
        // "17 FiiO K3": the device's usbfs file and name; "?…": waiting for the user to allow it;
        // "!…": not possible.
        let answer = call("UsbDac", "open", rate as i32)?;
        if let Some(why) = answer.strip_prefix('?') {
            WAITING.store(true, Ordering::Relaxed);
            bail!("{why}");
        }
        if let Some(why) = answer.strip_prefix('!') {
            bail!("{why}");
        }
        let (fd, name) = answer.split_once(' ').ok_or_else(|| anyhow!("unexpected answer {answer:?}"))?;
        let fd: RawFd = fd.parse().map_err(|_| anyhow!("unexpected answer {answer:?}"))?;
        match take_over(fd) {
            Ok(dac) => {
                log::info!("USB exclusive: took over {name}: {dac:?}");
                *held = Some(Held { fd, name: name.to_string(), dac });
            }
            Err(e) => {
                let _ = call("UsbDac", "close", 2000);
                return Err(e.context(format!("Cadence can't drive {name}")));
            }
        }
    }
    OPENS.fetch_add(1, Ordering::Relaxed);
    fetch_gain();
    let h = held.as_ref().unwrap();
    let (fd, dac, name) = (h.fd, h.dac.clone(), h.name.clone());
    drop(held);
    match start(fd, &dac, rate, shared) {
        Ok((stream, prod, rate)) => {
            log::info!("USB exclusive: {name}, {}-bit at {rate} Hz", dac.bits);
            Ok(Output {
                backend: Backend::Usb(stream),
                prod,
                rate,
                channels: 2,
                device: name,
                bits: Some(dac.bits as u32),
            })
        }
        Err(e) => {
            // Let Android have it again right away, so the fallback can play through it.
            give_back(true);
            Err(e)
        }
    }
}

/// Reads the descriptors, detaches Android's audio driver and claims the interfaces Cadence uses.
fn take_over(fd: RawFd) -> Result<Dac> {
    let mut desc = vec![0u8; 16 * 1024];
    let n = unsafe {
        libc::lseek(fd, 0, libc::SEEK_SET);
        libc::read(fd, desc.as_mut_ptr().cast(), desc.len())
    };
    if n <= 0 {
        bail!("couldn't read its descriptors: {}", io::Error::last_os_error());
    }
    desc.truncate(n as usize);
    // Super speed and faster count microframes too.
    let speed = unsafe { usbfs::ioctl(fd, usbfs::GET_SPEED, std::ptr::null_mut()) }.unwrap_or(3);
    let mut parsed = parse(&desc, speed >= 3)?;
    for interface in [parsed.dac.control, parsed.dac.stream] {
        // No driver to detach is fine (ENODATA).
        let _ = usbfs::driver(fd, interface, usbfs::DISCONNECT);
        if let Err(e) = usbfs::claim(fd, interface, usbfs::CLAIMINTERFACE) {
            reattach(fd, &parsed.dac);
            bail!("couldn't take interface {interface} over: {e}");
        }
    }
    parsed.dac.clock = resolve_clock(fd, &parsed);
    Ok(parsed.dac)
}

/// Releases the interfaces and lets Android's audio driver take the DAC again.
fn reattach(fd: RawFd, dac: &Dac) {
    for interface in [dac.stream, dac.control] {
        let _ = usbfs::claim(fd, interface, usbfs::RELEASEINTERFACE);
    }
    // The audio driver binds to the control interface and takes the streaming one itself.
    for interface in [dac.control, dac.stream] {
        let _ = usbfs::driver(fd, interface, usbfs::CONNECT);
    }
}

/// The clock source behind the playback terminal's clock (through selectors and multipliers).
fn resolve_clock(fd: RawFd, parsed: &Parsed) -> u8 {
    let mut id = parsed.dac.clock;
    for _ in 0..8 {
        match parsed.clocks.iter().find(|(c, _)| *c == id).map(|(_, k)| k) {
            Some(Clock::Selector(inputs)) if !inputs.is_empty() => {
                let mut pin = [0u8];
                let index = ((id as u16) << 8) | parsed.dac.control as u16;
                let chosen = usbfs::control(fd, GET, CUR, SELECTOR, index, &mut pin)
                    .ok()
                    .and_then(|_| inputs.get((pin[0] as usize).wrapping_sub(1)))
                    .unwrap_or(&inputs[0]);
                id = *chosen;
            }
            Some(Clock::Multiplier(source)) => id = *source,
            _ => break,
        }
    }
    id
}

/// The sample rates the DAC's clock offers: (min, max, step) ranges, if it says.
fn rates(fd: RawFd, dac: &Dac) -> Option<Vec<(u32, u32, u32)>> {
    let index = ((dac.clock as u16) << 8) | dac.control as u16;
    let mut head = [0u8; 2];
    usbfs::control(fd, GET, RANGE, SAM_FREQ, index, &mut head).ok()?;
    let count = u16::from_le_bytes(head) as usize;
    let mut all = vec![0u8; 2 + 12 * count];
    let n = usbfs::control(fd, GET, RANGE, SAM_FREQ, index, &mut all).ok()?;
    let u32_at = |i: usize| u32::from_le_bytes([all[i], all[i + 1], all[i + 2], all[i + 3]]);
    Some(
        (0..count)
            .filter(|k| 2 + 12 * k + 12 <= n)
            .map(|k| (u32_at(2 + 12 * k), u32_at(6 + 12 * k), u32_at(10 + 12 * k)))
            .collect(),
    )
}

/// Sets the DAC to `rate` and starts streaming to it.
fn start(fd: RawFd, dac: &Dac, rate: u32, shared: Arc<Shared>) -> Result<(UsbStream, ringbuf::HeapProd<f32>, u32)> {
    let rate = rates(fd, dac).map_or(rate, |r| pick_rate(&r, rate));
    // The clock changes while the interface is idle (alternate setting 0), as Linux does it.
    usbfs::set_interface(fd, dac.stream, 0).map_err(|e| anyhow!("couldn't reset its playback: {e}"))?;
    let index = ((dac.clock as u16) << 8) | dac.control as u16;
    let mut want = rate.to_le_bytes();
    usbfs::control(fd, SET, CUR, SAM_FREQ, index, &mut want).map_err(|e| anyhow!("couldn't set it to {rate} Hz: {e}"))?;
    let mut got = [0u8; 4];
    if usbfs::control(fd, GET, CUR, SAM_FREQ, index, &mut got).is_ok_and(|n| n == 4) {
        let got = u32::from_le_bytes(got);
        if got != rate {
            bail!("it stayed at {got} Hz instead of {rate} Hz");
        }
    }
    usbfs::set_interface(fd, dac.stream, dac.alt).map_err(|e| anyhow!("couldn't start its playback: {e}"))?;
    // Some DACs need a moment after a rate change before they take samples.
    std::thread::sleep(Duration::from_millis(30));

    let channels = 2;
    // Half a second of buffer, as in the other modes.
    let (prod, cons) = HeapRb::<f32>::new((rate as usize / 2) * channels).split();
    let renderer = Renderer::new(cons, shared.clone(), channels, rate, true);
    let stop = Arc::new(AtomicBool::new(false));
    let mut pump = Pump::new(fd, dac, rate, renderer, shared);
    let flag = stop.clone();
    let thread = std::thread::Builder::new()
        .name("usb-dac".into())
        .spawn(move || pump.run(&flag))?;
    Ok((
        UsbStream {
            fd,
            interface: dac.stream,
            stop,
            thread: Some(thread),
        },
        prod,
        rate,
    ))
}

/// Keeps the DAC streaming; dropping it stops (and gives the DAC back to Android a moment later,
/// unless Cadence opens it again first).
pub struct UsbStream {
    fd: RawFd,
    interface: u8,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for UsbStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = usbfs::set_interface(self.fd, self.interface, 0);
        let opens = OPENS.load(Ordering::Relaxed);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            if OPENS.load(Ordering::Relaxed) == opens {
                give_back(false);
            }
        });
    }
}

/// Lets go of the DAC now: Android's audio driver takes it again (`wait`: until Android's audio
/// system lists it again, at most a couple of seconds, so playing through Android finds it).
pub fn give_back(wait: bool) {
    let mut held = HELD.lock().unwrap();
    let Some(h) = held.take() else {
        return;
    };
    reattach(h.fd, &h.dac);
    let _ = call("UsbDac", "close", if wait { 2000 } else { 0 });
    log::info!("USB exclusive: gave {} back to Android", h.name);
}

/// One isochronous transfer: the URB (with its packets after it) and its data.
struct Transfer {
    /// u64s keep the URB's pointers aligned.
    mem: Vec<u64>,
    buf: Vec<u8>,
    packets: usize,
}

impl Transfer {
    fn new(packets: usize, packet_bytes: usize) -> Self {
        let bytes = size_of::<usbfs::Urb>() + packets * size_of::<usbfs::IsoPacket>();
        Self {
            mem: vec![0; bytes.div_ceil(8)],
            buf: vec![0; packets * packet_bytes],
            packets,
        }
    }

    fn urb(&mut self) -> *mut usbfs::Urb {
        self.mem.as_mut_ptr().cast()
    }

    fn packet(&mut self, i: usize) -> &mut usbfs::IsoPacket {
        assert!(i < self.packets);
        // SAFETY: `mem` has room for `packets` of them right after the URB.
        unsafe { &mut *self.mem.as_mut_ptr().cast::<u8>().add(size_of::<usbfs::Urb>()).cast::<usbfs::IsoPacket>().add(i) }
    }

    /// Hands the transfer to the kernel; `len` bytes of `buf`, split as its packets say.
    fn submit(&mut self, fd: RawFd, endpoint: u8, len: usize, tag: usize) -> io::Result<()> {
        let buffer = self.buf.as_mut_ptr().cast();
        let packets = self.packets as i32;
        let urb = self.urb();
        // SAFETY: `urb` points at our own URB memory, which stays put until the kernel hands it back
        // (the pump reaps every transfer before it ends).
        unsafe {
            urb.write(usbfs::Urb {
                kind: usbfs::URB_TYPE_ISO,
                endpoint,
                status: 0,
                flags: usbfs::URB_ISO_ASAP,
                buffer,
                buffer_length: len as i32,
                actual_length: 0,
                start_frame: 0,
                number_of_packets: packets,
                error_count: 0,
                signr: 0,
                usercontext: tag as *mut c_void,
            });
            usbfs::ioctl(fd, usbfs::SUBMITURB, urb.cast()).map(|_| ())
        }
    }
}

/// Transfers queued at once, and how long each one plays.
const QUEUED: usize = 8;
const TRANSFER_MS: usize = 4;
const FEEDBACK_TAG: usize = 1000;

/// Streams to the DAC: fills each transfer from the renderer as the DAC finishes one.
struct Pump {
    fd: RawFd,
    endpoint: u8,
    feedback: Option<u8>,
    renderer: Renderer,
    shared: Arc<Shared>,
    format: Format,
    dither: Dither,
    floats: Vec<f32>,
    frame_bytes: usize,
    max_frames: usize,
    data: Vec<Transfer>,
    sync: Vec<Transfer>,
    pacing: Pacing,
}

impl Pump {
    fn new(fd: RawFd, dac: &Dac, rate: u32, renderer: Renderer, shared: Arc<Shared>) -> Self {
        let per_second = if dac.high_speed { 8000 } else { 1000 } >> dac.interval_log;
        let packets = (per_second as usize * TRANSFER_MS / 1000).clamp(1, 128);
        let frame_bytes = 2 * dac.subslot;
        let max_frames = dac.max_packet / frame_bytes;
        Self {
            fd,
            endpoint: dac.endpoint,
            feedback: dac.feedback.map(|(ep, _)| ep),
            renderer,
            shared,
            format: Format { store_bits: dac.subslot * 8, valid_bits: dac.bits },
            dither: Dither::default(),
            floats: vec![0.0; max_frames * 2],
            frame_bytes,
            max_frames,
            data: (0..QUEUED).map(|_| Transfer::new(packets, max_frames * frame_bytes)).collect(),
            sync: dac.feedback.map_or(Vec::new(), |(_, size)| (0..2).map(|_| Transfer::new(1, size.max(4))).collect()),
            pacing: Pacing::new(rate, dac.high_speed, dac.interval_log),
        }
    }

    /// Renders the next packets of music into data transfer `i`; returns its length in bytes.
    fn fill(&mut self, i: usize) -> usize {
        let gain = f32::from_bits(GAIN.load(Ordering::Relaxed));
        let mut offset = 0;
        for p in 0..self.data[i].packets {
            let frames = self.pacing.next_frames(self.max_frames);
            let samples = frames * 2;
            let bytes = frames * self.frame_bytes;
            self.renderer.render(&mut self.floats[..samples]);
            if gain < 1.0 {
                self.floats[..samples].iter_mut().for_each(|s| *s *= gain);
            }
            let t = &mut self.data[i];
            encode(&self.floats[..samples], &mut t.buf[offset..offset + bytes], self.format, &mut self.dither);
            let packet = t.packet(p);
            packet.length = bytes as u32;
            packet.actual_length = 0;
            packet.status = 0;
            offset += bytes;
        }
        offset
    }

    fn reap(&self) -> io::Result<*mut usbfs::Urb> {
        let mut urb: *mut c_void = std::ptr::null_mut();
        unsafe { usbfs::ioctl(self.fd, usbfs::REAPURB, (&raw mut urb).cast())? };
        Ok(urb.cast())
    }

    fn run(&mut self, stop: &AtomicBool) {
        // The same priority Android gives its own audio threads.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS as _, libc::gettid() as _, -19);
        }
        let mut in_flight = 0usize;
        let mut lost = false;
        for i in 0..self.data.len() {
            let len = self.fill(i);
            match self.data[i].submit(self.fd, self.endpoint, len, i) {
                Ok(()) => in_flight += 1,
                Err(e) => {
                    log::warn!("USB exclusive: couldn't send to the DAC: {e}");
                    lost = true;
                    break;
                }
            }
        }
        if let Some(ep) = self.feedback.filter(|_| !lost) {
            for j in 0..self.sync.len() {
                let size = self.sync[j].buf.len();
                self.sync[j].packet(0).length = size as u32;
                if self.sync[j].submit(self.fd, ep, size, FEEDBACK_TAG + j).is_ok() {
                    in_flight += 1;
                }
            }
        }
        while in_flight > 0 {
            let urb = match self.reap() {
                Ok(urb) => urb,
                Err(e) => {
                    // The DAC was unplugged (or the file closed): nothing more will come back.
                    log::warn!("USB exclusive: the DAC went away: {e}");
                    lost = true;
                    break;
                }
            };
            in_flight -= 1;
            // SAFETY: the kernel hands back one of our own URBs, finished.
            let (tag, status) = unsafe { ((*urb).usercontext as usize, (*urb).status) };
            if status == -libc::ENODEV || status == -libc::ESHUTDOWN {
                lost = true;
            }
            if lost || stop.load(Ordering::Acquire) {
                continue;
            }
            if tag >= FEEDBACK_TAG {
                let j = tag - FEEDBACK_TAG;
                let got = self.sync[j].packet(0).actual_length as usize;
                if status == 0 && got > 0 {
                    let n = got.min(4);
                    let mut raw = [0u8; 4];
                    raw[..n].copy_from_slice(&self.sync[j].buf[..n]);
                    self.pacing.feedback(&raw[..n]);
                }
                let size = self.sync[j].buf.len();
                self.sync[j].packet(0).length = size as u32;
                if self.sync[j].submit(self.fd, self.feedback.unwrap(), size, tag).is_ok() {
                    in_flight += 1;
                }
            } else {
                let len = self.fill(tag);
                match self.data[tag].submit(self.fd, self.endpoint, len, tag) {
                    Ok(()) => in_flight += 1,
                    Err(e) => {
                        log::warn!("USB exclusive: couldn't send to the DAC: {e}");
                        lost = true;
                    }
                }
            }
        }
        if lost {
            self.drain(in_flight);
            // Like headphones being pulled: pause, then the engine reopens (Android's output).
            self.shared.playing.store(false, Ordering::Relaxed);
            self.shared.failed.store(true, Ordering::Release);
        }
    }

    /// Gets back transfers still queued after a failure, so none outlives its memory.
    fn drain(&mut self, mut in_flight: usize) {
        if in_flight == 0 {
            return;
        }
        for t in self.data.iter_mut().chain(self.sync.iter_mut()) {
            let urb = t.urb();
            let _ = unsafe { usbfs::ioctl(self.fd, usbfs::DISCARDURB, urb.cast()) };
        }
        let until = Instant::now() + Duration::from_millis(500);
        while in_flight > 0 && Instant::now() < until {
            let mut urb: *mut c_void = std::ptr::null_mut();
            match unsafe { usbfs::ioctl(self.fd, usbfs::REAPURBNDELAY, (&raw mut urb).cast()) } {
                Ok(_) => in_flight -= 1,
                Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
    }
}
