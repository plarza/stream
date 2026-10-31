//! The audio path from decoders to the device.
//!
//! Decoders write interleaved stereo `f32` at the output rate into a lock-free
//! ring buffer. Swift's AVAudioSourceNode pulls it through
//! [`stream_core_render`] on the audio thread: no locks it can wait on, no
//! allocation. Frame counters on both sides give the audible position, so the
//! UI shows exactly what is heard even while the next song is already being
//! decoded.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use rtrb::{Consumer, Producer, RingBuffer};
use rubato::{FftFixedInOut, Resampler as _};

/// Two seconds of stereo at up to 192 kHz.
const RING_SECONDS: usize = 2;
/// Fade applied when pausing, resuming and jumping, so nothing clicks.
const RAMP_FRAMES: f32 = 480.0;

pub struct Output {
    producer: Mutex<Producer<f32>>,
    /// Frames ever written by decoders.
    pub frames_written: AtomicU64,
    /// Frames ever handed to the device (silence on underrun is not counted).
    pub frames_played: AtomicU64,
    /// Frames before this are stale (a jump happened) and are dropped unheard.
    skip_until: AtomicU64,
    /// Whether the device should hear audio (gain ramps toward 0 or 1).
    pub playing: AtomicBool,
    pub rate: AtomicU32,
    /// Bumped on every jump; writers from an older generation stop.
    generation: AtomicU64,
    /// Volume scale applied in the render callback (1.0 in bit-perfect mode).
    volume: AtomicU32,
}

struct RenderState {
    consumer: Consumer<f32>,
    gain: f32,
}

static OUTPUT: OnceLock<Arc<Output>> = OnceLock::new();
static RENDER: Mutex<Option<RenderState>> = Mutex::new(None);

impl Output {
    pub fn shared() -> Arc<Output> {
        OUTPUT
            .get_or_init(|| {
                let (producer, consumer) = RingBuffer::new(192_000 * 2 * RING_SECONDS);
                *RENDER.lock() = Some(RenderState { consumer, gain: 0.0 });
                Arc::new(Output {
                    producer: Mutex::new(producer),
                    frames_written: AtomicU64::new(0),
                    frames_played: AtomicU64::new(0),
                    skip_until: AtomicU64::new(0),
                    playing: AtomicBool::new(false),
                    rate: AtomicU32::new(44_100),
                    generation: AtomicU64::new(0),
                    volume: AtomicU32::new(1.0f32.to_bits()),
                })
            })
            .clone()
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn rate(&self) -> u32 {
        self.rate.load(Ordering::Relaxed)
    }

    pub fn set_volume(&self, v: f32) {
        self.volume.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Drops everything not yet heard and invalidates current writers.
    /// Returns the new generation and the frame index the next audio starts at.
    pub fn flush(&self) -> (u64, u64) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        // Holding the producer guarantees no writer is mid-chunk.
        let _producer = self.producer.lock();
        let at = self.frames_written.load(Ordering::Acquire);
        self.skip_until.store(at, Ordering::Release);
        (generation, at)
    }

    /// Buffered but not yet heard, in frames.
    pub fn pending_frames(&self) -> u64 {
        self.frames_written
            .load(Ordering::Acquire)
            .saturating_sub(self.frames_played.load(Ordering::Acquire).max(self.skip_until.load(Ordering::Acquire)))
    }

    /// Writes interleaved stereo frames, waiting for room. Returns false when a
    /// jump made this writer stale.
    pub fn write(&self, mut samples: &[f32], generation: u64) -> bool {
        while !samples.is_empty() {
            if self.generation() != generation {
                return false;
            }
            let wrote = {
                let mut p = self.producer.lock();
                if self.generation() != generation {
                    return false;
                }
                let n = p.slots().min(samples.len()) & !1;
                if n > 0 {
                    let (head, rest) = samples.split_at(n);
                    if let Ok(chunk) = p.write_chunk_uninit(n) {
                        chunk.fill_from_iter(head.iter().copied());
                    }
                    samples = rest;
                    self.frames_written.fetch_add((n / 2) as u64, Ordering::AcqRel);
                }
                n
            };
            if wrote == 0 {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        true
    }
}

/// Called by Swift's AVAudioSourceNode on the audio thread. Fills `frames`
/// samples into each channel buffer and returns how many were real audio.
///
/// # Safety
/// `left` and `right` must point to at least `frames` writable floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stream_core_render(left: *mut f32, right: *mut f32, frames: u32) -> u32 {
    let frames = frames as usize;
    // SAFETY: the caller guarantees both buffers hold `frames` floats.
    let (l, r) = unsafe { (std::slice::from_raw_parts_mut(left, frames), std::slice::from_raw_parts_mut(right, frames)) };
    l.fill(0.0);
    r.fill(0.0);
    let Some(out) = OUTPUT.get() else { return 0 };
    let Some(mut guard) = RENDER.try_lock() else { return 0 };
    let Some(state) = guard.as_mut() else { return 0 };

    // Drop audio made stale by a jump.
    let skip = out.skip_until.load(Ordering::Acquire);
    let played = out.frames_played.load(Ordering::Acquire);
    if played < skip {
        let stale = ((skip - played) as usize).min(state.consumer.slots() / 2);
        if stale > 0 {
            if let Ok(chunk) = state.consumer.read_chunk(stale * 2) {
                chunk.commit_all();
            }
            out.frames_played.fetch_add(stale as u64, Ordering::AcqRel);
            state.gain = 0.0;
        }
        if out.frames_played.load(Ordering::Acquire) < skip {
            return 0;
        }
    }

    let target = if out.playing.load(Ordering::Relaxed) { 1.0 } else { 0.0 };
    if target == 0.0 && state.gain == 0.0 {
        return 0;
    }
    let available = state.consumer.slots() / 2;
    let n = available.min(frames);
    let volume = f32::from_bits(out.volume.load(Ordering::Relaxed));
    let step = 1.0 / RAMP_FRAMES;
    if n > 0
        && let Ok(chunk) = state.consumer.read_chunk(n * 2)
    {
        let (a, b) = chunk.as_slices();
        let mut i = 0usize;
        for pair in a.chunks_exact(2).chain(b.chunks_exact(2)) {
            if state.gain != target {
                state.gain = if target > state.gain { (state.gain + step).min(1.0) } else { (state.gain - step).max(0.0) };
            }
            let g = state.gain * volume;
            l[i] = pair[0] * g;
            r[i] = pair[1] * g;
            i += 1;
        }
        // While fading out to pause, the audio is heard and consumed; once
        // silent, stop consuming so resume continues where it paused.
        chunk.commit_all();
        out.frames_played.fetch_add(n as u64, Ordering::AcqRel);
    }
    n as u32
}

/// Converts decoded audio to stereo at the output rate and writes it.
pub struct TrackWriter {
    out: Arc<Output>,
    generation: u64,
    resampler: Option<StereoResampler>,
    gain: f32,
    scratch: Vec<f32>,
}

impl TrackWriter {
    pub fn new(out: Arc<Output>, generation: u64, src_rate: u32, gain: f32) -> Self {
        let dst = out.rate();
        let resampler = (src_rate != dst).then(|| StereoResampler::new(src_rate, dst));
        Self { out, generation, resampler, gain, scratch: Vec::new() }
    }

    /// `samples` is interleaved with `channels` channels at the source rate.
    pub fn write(&mut self, samples: &[f32], channels: usize) -> bool {
        self.scratch.clear();
        let g = self.gain;
        match channels {
            0 => return true,
            1 => self.scratch.extend(samples.iter().flat_map(|s| [s * g, s * g])),
            2 => self.scratch.extend(samples.iter().map(|s| s * g)),
            c => {
                for f in samples.chunks_exact(c) {
                    self.scratch.push(f[0] * g);
                    self.scratch.push(f[1] * g);
                }
            }
        }
        match &mut self.resampler {
            None => self.out.write(&self.scratch, self.generation),
            Some(rs) => {
                let resampled = rs.push(&self.scratch);
                self.out.write(&resampled, self.generation)
            }
        }
    }

    /// Flushes the resampler's tail at the end of a song.
    pub fn finish(&mut self) -> bool {
        if let Some(rs) = &mut self.resampler {
            let tail = rs.finish();
            return self.out.write(&tail, self.generation);
        }
        true
    }

    pub fn is_current(&self) -> bool {
        self.out.generation() == self.generation
    }
}

struct StereoResampler {
    inner: FftFixedInOut<f32>,
    pending: [Vec<f32>; 2],
    /// Output frames still to drop to undo the resampler's delay.
    delay: usize,
    ratio: f64,
    frames_in: usize,
    frames_out: usize,
}

impl StereoResampler {
    fn new(src: u32, dst: u32) -> Self {
        let inner = FftFixedInOut::<f32>::new(src as usize, dst as usize, 1024, 2).expect("valid sample rates");
        let delay = inner.output_delay();
        Self {
            inner,
            pending: [Vec::new(), Vec::new()],
            delay,
            ratio: dst as f64 / src as f64,
            frames_in: 0,
            frames_out: 0,
        }
    }

    fn push(&mut self, interleaved: &[f32]) -> Vec<f32> {
        for f in interleaved.chunks_exact(2) {
            self.pending[0].push(f[0]);
            self.pending[1].push(f[1]);
        }
        self.frames_in += interleaved.len() / 2;
        let mut out = Vec::new();
        loop {
            let need = self.inner.input_frames_next();
            if self.pending[0].len() < need {
                break;
            }
            let chunk = [self.pending[0][..need].to_vec(), self.pending[1][..need].to_vec()];
            self.pending[0].drain(..need);
            self.pending[1].drain(..need);
            if let Ok(res) = self.inner.process(&chunk, None) {
                self.interleave_into(&res, &mut out);
            }
        }
        out
    }

    fn finish(&mut self) -> Vec<f32> {
        let mut out = Vec::new();
        let rest = [std::mem::take(&mut self.pending[0]), std::mem::take(&mut self.pending[1])];
        if !rest[0].is_empty()
            && let Ok(res) = self.inner.process_partial(Some(&rest), None)
        {
            self.interleave_into(&res, &mut out);
        }
        // Push zeros through until every input frame has come out.
        let expected = (self.frames_in as f64 * self.ratio).round() as usize;
        let mut guard = 0;
        while self.frames_out < expected && guard < 8 {
            if let Ok(res) = self.inner.process_partial::<Vec<f32>>(None, None) {
                self.interleave_into(&res, &mut out);
            }
            guard += 1;
        }
        let extra = self.frames_out.saturating_sub(expected);
        out.truncate(out.len().saturating_sub(extra * 2));
        out
    }

    fn interleave_into(&mut self, planar: &[Vec<f32>], out: &mut Vec<f32>) {
        let n = planar[0].len();
        let skip = self.delay.min(n);
        self.delay -= skip;
        for i in skip..n {
            out.push(planar[0][i]);
            out.push(planar[1][i]);
        }
        self.frames_out += n - skip;
    }
}
