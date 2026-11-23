//! Voice chat audio: microphone in, speakers out.
//!
//! ```text
//! mic ─> resample to 48 kHz ─> noise suppression + speech detection (RNNoise) ─> Opus ─> send
//! receive ─> Opus decode (per speaker) ─> jitter buffer ─> mix ─> resample ─> speakers
//! ```
//!
//! Sending is voice-activated: frames go out only while the player is talking (plus a short
//! tail so word endings aren't clipped, and a little audio from just before, so first syllables
//! aren't either). cpal streams can't move between threads, so a [`Session`] runs one thread that
//! owns the streams and one that processes the microphone; dropping the session stops both.
//!
//! For tests without a microphone, `CONDENSATE_VOICE_TEST_INPUT=<file.wav>` plays a WAV file
//! into the microphone path once (in real time), then silence.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use condensate_core::play::VOICE_FRAME;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use serde::{Deserialize, Serialize};

const RATE: u32 = 48_000;
/// RNNoise works on 10 ms frames.
const DENOISE_FRAME: usize = nnnoiseless::DenoiseState::FRAME_SIZE;
/// Keep sending this long after speech stops (in 20 ms frames).
const HOLD_FRAMES: u32 = 20;
/// Frames from just before speech starts that are sent along with it.
const PRE_ROLL: usize = 2;
/// Buffer this much of a speaker before playing, to ride out network jitter.
const JITTER_START: usize = 3 * VOICE_FRAME;
/// More than this buffered means we've fallen behind: skip ahead to `JITTER_START`.
const JITTER_MAX: usize = 15 * VOICE_FRAME;
/// Someone counts as speaking for this long after their last frame.
const SPEAKING_FOR: Duration = Duration::from_millis(250);

/// Saved with the app's settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    /// Device names; `None` = the system default.
    pub input: Option<String>,
    pub output: Option<String>,
    /// Detect speech automatically. Otherwise anything louder than `threshold_db` is sent.
    pub auto_sensitivity: bool,
    pub threshold_db: f32,
    pub noise_suppression: bool,
    /// Playback volume, 0.0 to 2.0.
    pub volume: f32,
    pub muted: bool,
    pub deafened: bool,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self { input: None, output: None, auto_sensitivity: true, threshold_db: -45.0, noise_suppression: true, volume: 1.0, muted: false, deafened: false }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Devices {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

pub fn devices() -> Devices {
    let host = cpal::default_host();
    let names = |it: Option<Vec<cpal::Device>>| it.unwrap_or_default().iter().map(|d| d.to_string()).collect();
    Devices {
        inputs: names(host.input_devices().ok().map(|d| d.collect())),
        outputs: names(host.output_devices().ok().map(|d| d.collect())),
    }
}

// ---- Opus ----------------------------------------------------------------------------------

struct Encoder(*mut unsafe_libopus::OpusEncoder);
// The encoder is only ever used from one thread at a time.
unsafe impl Send for Encoder {}

impl Encoder {
    fn new() -> Option<Self> {
        let mut err = 0;
        let st = unsafe { unsafe_libopus::opus_encoder_create(RATE as i32, 1, unsafe_libopus::OPUS_APPLICATION_VOIP, &mut err) };
        if st.is_null() || err != unsafe_libopus::OPUS_OK {
            return None;
        }
        unsafe {
            unsafe_libopus::opus_encoder_ctl!(st, unsafe_libopus::OPUS_SET_BITRATE_REQUEST, 32_000);
            unsafe_libopus::opus_encoder_ctl!(st, unsafe_libopus::OPUS_SET_COMPLEXITY_REQUEST, 5);
            unsafe_libopus::opus_encoder_ctl!(st, unsafe_libopus::OPUS_SET_SIGNAL_REQUEST, unsafe_libopus::OPUS_SIGNAL_VOICE);
        }
        Some(Self(st))
    }

    fn encode(&mut self, pcm: &[i16; VOICE_FRAME]) -> Option<Vec<u8>> {
        let mut out = [0u8; 1500];
        let n = unsafe { unsafe_libopus::opus_encode(self.0, pcm.as_ptr(), VOICE_FRAME as i32, out.as_mut_ptr(), out.len() as i32) };
        (n > 0).then(|| out[..n as usize].to_vec())
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { unsafe_libopus::opus_encoder_destroy(self.0) }
    }
}

struct Decoder(*mut unsafe_libopus::OpusDecoder);
unsafe impl Send for Decoder {}

impl Decoder {
    fn new() -> Option<Self> {
        let mut err = 0;
        let st = unsafe { unsafe_libopus::opus_decoder_create(RATE as i32, 1, &mut err) };
        (!st.is_null() && err == unsafe_libopus::OPUS_OK).then_some(Self(st))
    }

    fn decode(&mut self, packet: &[u8], out: &mut [i16; 5760]) -> usize {
        let n = unsafe { unsafe_libopus::opus_decode(self.0, packet.as_ptr(), packet.len() as i32, out.as_mut_ptr(), out.len() as i32, 0) };
        n.max(0) as usize
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { unsafe_libopus::opus_decoder_destroy(self.0) }
    }
}

// ---- resampling ----------------------------------------------------------------------------

/// Linear interpolation between sample rates; plenty for speech.
struct Resampler {
    /// Source samples per output sample.
    step: f64,
    t: f64,
    prev: f32,
    cur: f32,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Self { step: from as f64 / to as f64, t: 0.0, prev: 0.0, cur: 0.0 }
    }

    /// Push source samples, appending output samples.
    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &x in input {
            while self.t < 1.0 {
                out.push(self.prev + (x - self.prev) * self.t as f32);
                self.t += self.step;
            }
            self.t -= 1.0;
            self.prev = x;
        }
    }

    /// Pull one output sample, taking source samples from `next` as needed.
    fn pull(&mut self, mut next: impl FnMut() -> f32) -> f32 {
        while self.t >= 1.0 {
            self.prev = self.cur;
            self.cur = next();
            self.t -= 1.0;
        }
        let v = self.prev + (self.cur - self.prev) * self.t as f32;
        self.t += self.step;
        v
    }
}

// ---- playback ------------------------------------------------------------------------------

struct Speaker {
    decoder: Decoder,
    queue: VecDeque<f32>,
    playing: bool,
    last: Instant,
}

#[derive(Default)]
struct Mixer {
    speakers: HashMap<u32, Speaker>,
}

impl Mixer {
    fn push(&mut self, member: u32, packet: &[u8]) {
        let Some(s) = (match self.speakers.entry(member) {
            std::collections::hash_map::Entry::Occupied(e) => Some(e.into_mut()),
            std::collections::hash_map::Entry::Vacant(e) => {
                Decoder::new().map(|decoder| e.insert(Speaker { decoder, queue: VecDeque::new(), playing: false, last: Instant::now() }))
            }
        }) else {
            return;
        };
        let mut pcm = [0i16; 5760];
        let n = s.decoder.decode(packet, &mut pcm);
        s.queue.extend(pcm[..n].iter().map(|&v| v as f32 / 32768.0));
        s.last = Instant::now();
        if s.queue.len() > JITTER_MAX {
            let extra = s.queue.len() - JITTER_START;
            s.queue.drain(..extra);
        }
        if !s.playing && s.queue.len() >= JITTER_START {
            s.playing = true;
        }
    }

    /// The next 48 kHz sample of everyone mixed together.
    fn next(&mut self) -> f32 {
        let mut sum = 0.0;
        for s in self.speakers.values_mut().filter(|s| s.playing) {
            match s.queue.pop_front() {
                Some(v) => sum += v,
                None => s.playing = false, // wait for a buffer's worth again
            }
        }
        sum
    }
}

// ---- session -------------------------------------------------------------------------------

/// Live state, readable from any thread.
#[derive(Default)]
pub struct Meters {
    /// The player is talking (and being sent).
    pub speaking: AtomicBool,
    /// Microphone level in dBFS (f32 bits).
    level: AtomicU32,
}

impl Meters {
    pub fn level_db(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

/// Microphone and speakers running. Stops when dropped.
pub struct Session {
    stop: Arc<AtomicBool>,
    mixer: Arc<Mutex<Mixer>>,
    pub meters: Arc<Meters>,
    /// Problems opening devices, for showing to the player.
    pub problems: Vec<String>,
}

impl Session {
    /// Start voice. Encoded frames (while the player talks) go to `send`; with `None`, only the
    /// meters run (a microphone test).
    pub fn start(settings: Arc<Mutex<VoiceSettings>>, send: Option<Box<dyn Fn(Vec<u8>) + Send>>) -> Session {
        let stop = Arc::new(AtomicBool::new(false));
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        let meters = Arc::new(Meters::default());
        meters.level.store((-100f32).to_bits(), Ordering::Relaxed);
        let (mic_tx, mic_rx) = mpsc::sync_channel::<Vec<f32>>(64);
        let (ready_tx, ready_rx) = mpsc::channel::<(u32, Vec<String>)>();

        let s = settings.lock().unwrap().clone();
        let test_input = std::env::var("CONDENSATE_VOICE_TEST_INPUT").ok().filter(|v| !v.is_empty());
        let play_audio = send.is_some();
        {
            let (stop, mixer, settings) = (stop.clone(), mixer.clone(), settings.clone());
            let test = test_input.is_some();
            std::thread::Builder::new()
                .name("voice-devices".into())
                .spawn(move || run_devices(s, test, play_audio, mic_tx, mixer, settings, stop, ready_tx))
                .ok();
        }
        let (mut mic_rate, mut problems) = ready_rx.recv_timeout(Duration::from_secs(5)).unwrap_or((RATE, vec!["the audio devices didn't respond".into()]));
        if let Some(path) = test_input {
            match test_feed(&path, stop.clone()) {
                Ok((rate, rx)) => {
                    mic_rate = rate;
                    forward(rx, stop.clone(), settings.clone(), meters.clone(), send, mic_rate);
                    return Session { stop, mixer, meters, problems };
                }
                Err(e) => problems.push(format!("test input {path}: {e}")),
            }
        }
        forward(mic_rx, stop.clone(), settings, meters.clone(), send, mic_rate);
        Session { stop, mixer, meters, problems }
    }

    /// A voice frame from lobby member `member`.
    pub fn incoming(&self, member: u32, packet: &[u8]) {
        self.mixer.lock().unwrap().push(member, packet);
    }

    /// Members heard in the last moment.
    pub fn speaking(&self) -> Vec<u32> {
        let m = self.mixer.lock().unwrap();
        m.speakers.iter().filter(|(_, s)| s.last.elapsed() < SPEAKING_FOR).map(|(id, _)| *id).collect()
    }

    /// Forget a member who left (frees their decoder).
    pub fn forget(&self, member: u32) {
        self.mixer.lock().unwrap().speakers.remove(&member);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn forward(
    rx: Receiver<Vec<f32>>,
    stop: Arc<AtomicBool>,
    settings: Arc<Mutex<VoiceSettings>>,
    meters: Arc<Meters>,
    send: Option<Box<dyn Fn(Vec<u8>) + Send>>,
    rate: u32,
) {
    std::thread::Builder::new()
        .name("voice-mic".into())
        .spawn(move || {
            let mut p = Processor::new(rate, settings, meters, send);
            loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(chunk) => p.push(&chunk),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
            }
            p.meters.speaking.store(false, Ordering::Relaxed);
        })
        .ok();
}

/// Open the devices and keep them running until `stop`. Reports the microphone's sample rate
/// and any problems on `ready`.
#[allow(clippy::too_many_arguments)]
fn run_devices(
    s: VoiceSettings,
    skip_input: bool,
    play_audio: bool,
    mic_tx: SyncSender<Vec<f32>>,
    mixer: Arc<Mutex<Mixer>>,
    settings: Arc<Mutex<VoiceSettings>>,
    stop: Arc<AtomicBool>,
    ready: mpsc::Sender<(u32, Vec<String>)>,
) {
    let host = cpal::default_host();
    let mut problems = Vec::new();
    let mut rate = RATE;
    let mut streams = Vec::new();

    if !skip_input {
        let device = match &s.input {
            Some(name) => host.input_devices().ok().and_then(|mut d| d.find(|d| d.to_string() == *name)).or_else(|| host.default_input_device()),
            None => host.default_input_device(),
        };
        match device.map(|d| open_input(&d, mic_tx)) {
            Some(Ok((stream, r))) => {
                rate = r;
                streams.push(stream);
            }
            Some(Err(e)) => problems.push(format!("microphone: {e}")),
            None => problems.push("no microphone found".into()),
        }
    }
    if play_audio {
        let device = match &s.output {
            Some(name) => host.output_devices().ok().and_then(|mut d| d.find(|d| d.to_string() == *name)).or_else(|| host.default_output_device()),
            None => host.default_output_device(),
        };
        match device.map(|d| open_output(&d, mixer, settings)) {
            Some(Ok(stream)) => streams.push(stream),
            Some(Err(e)) => problems.push(format!("speakers: {e}")),
            None => problems.push("no speakers found".into()),
        }
    }
    let _ = ready.send((rate, problems));
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(streams);
}

fn open_input(device: &cpal::Device, tx: SyncSender<Vec<f32>>) -> Result<(cpal::Stream, u32), String> {
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let rate = config.sample_rate();
    let channels = config.channels() as usize;
    let stream = match config.sample_format() {
        SampleFormat::F32 => input_stream::<f32>(device, &config.config(), channels, tx),
        SampleFormat::I16 => input_stream::<i16>(device, &config.config(), channels, tx),
        SampleFormat::U16 => input_stream::<u16>(device, &config.config(), channels, tx),
        SampleFormat::I32 => input_stream::<i32>(device, &config.config(), channels, tx),
        f => return Err(format!("unsupported sample format {f}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, rate))
}

fn input_stream<T>(device: &cpal::Device, config: &cpal::StreamConfig, channels: usize, tx: SyncSender<Vec<f32>>) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device
        .build_input_stream::<T, _, _>(
            config.clone(),
            move |data: &[T], _| {
                let mono: Vec<f32> = data.chunks(channels).map(|f| f.iter().map(|&s| f32::from_sample_(s)).sum::<f32>() / channels as f32).collect();
                let _ = tx.try_send(mono); // drop audio rather than block the device
            },
            |e| eprintln!("voice: microphone error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

fn open_output(device: &cpal::Device, mixer: Arc<Mutex<Mixer>>, settings: Arc<Mutex<VoiceSettings>>) -> Result<cpal::Stream, String> {
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let rate = config.sample_rate();
    let channels = config.channels() as usize;
    let stream = match config.sample_format() {
        SampleFormat::F32 => output_stream::<f32>(device, &config.config(), channels, rate, mixer, settings),
        SampleFormat::I16 => output_stream::<i16>(device, &config.config(), channels, rate, mixer, settings),
        SampleFormat::U16 => output_stream::<u16>(device, &config.config(), channels, rate, mixer, settings),
        SampleFormat::I32 => output_stream::<i32>(device, &config.config(), channels, rate, mixer, settings),
        f => return Err(format!("unsupported sample format {f}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

fn output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    rate: u32,
    mixer: Arc<Mutex<Mixer>>,
    settings: Arc<Mutex<VoiceSettings>>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let mut resampler = Resampler::new(RATE, rate);
    device
        .build_output_stream::<T, _, _>(
            config.clone(),
            move |data: &mut [T], _| {
                let (volume, deafened) = settings.lock().map(|s| (s.volume, s.deafened)).unwrap_or((1.0, false));
                let mut m = mixer.lock().unwrap();
                for frame in data.chunks_mut(channels) {
                    let v = resampler.pull(|| m.next());
                    let v = if deafened { 0.0 } else { (v * volume).clamp(-1.0, 1.0) };
                    frame.fill(T::from_sample_(v));
                }
            },
            |e| eprintln!("voice: speaker error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

/// Feed a WAV file in real time (tests).
fn test_feed(path: &str, stop: Arc<AtomicBool>) -> Result<(u32, Receiver<Vec<f32>>), String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader.samples::<i32>().filter_map(Result::ok).map(|s| s as f32 / (1i64 << (spec.bits_per_sample - 1)) as f32).collect(),
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
    };
    let mono: Vec<f32> = samples.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect();
    let (tx, rx) = mpsc::sync_channel(64);
    let chunk = (spec.sample_rate / 100) as usize; // 10 ms
    std::thread::spawn(move || {
        let start = Instant::now();
        let silence = vec![0.0; chunk];
        let mut sent = 0usize;
        let mut pos = 0usize;
        while !stop.load(Ordering::Relaxed) {
            let piece = if pos < mono.len() { &mono[pos..(pos + chunk).min(mono.len())] } else { &silence[..] };
            pos += chunk;
            if tx.send(piece.to_vec()).is_err() {
                return;
            }
            sent += 1;
            let due = start + Duration::from_millis(sent as u64 * 10);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    });
    Ok((spec.sample_rate, rx))
}

/// Microphone processing: 48 kHz, denoise, detect speech, encode, gate.
struct Processor {
    resampler: Resampler,
    buf: Vec<f32>,
    denoise: Box<nnnoiseless::DenoiseState<'static>>,
    frame: Vec<f32>,
    vad: f32,
    sum_sq: f32,
    encoder: Option<Encoder>,
    settings: Arc<Mutex<VoiceSettings>>,
    meters: Arc<Meters>,
    send: Option<Box<dyn Fn(Vec<u8>) + Send>>,
    hold: u32,
    recent: VecDeque<Vec<u8>>,
}

impl Processor {
    fn new(rate: u32, settings: Arc<Mutex<VoiceSettings>>, meters: Arc<Meters>, send: Option<Box<dyn Fn(Vec<u8>) + Send>>) -> Self {
        Self {
            resampler: Resampler::new(rate, RATE),
            buf: Vec::new(),
            denoise: nnnoiseless::DenoiseState::new(),
            frame: Vec::with_capacity(VOICE_FRAME),
            vad: 0.0,
            sum_sq: 0.0,
            encoder: if send.is_some() { Encoder::new() } else { None },
            settings,
            meters,
            send,
            hold: 0,
            recent: VecDeque::new(),
        }
    }

    fn push(&mut self, chunk: &[f32]) {
        self.resampler.push(chunk, &mut self.buf);
        let s = self.settings.lock().unwrap().clone();
        while self.buf.len() >= DENOISE_FRAME {
            let input: Vec<f32> = self.buf.drain(..DENOISE_FRAME).map(|v| v * 32767.0).collect();
            self.sum_sq += input.iter().map(|v| (v / 32767.0).powi(2)).sum::<f32>();
            let mut out = [0f32; DENOISE_FRAME];
            let vad = self.denoise.process_frame(&mut out, &input);
            self.vad = self.vad.max(vad);
            self.frame.extend_from_slice(if s.noise_suppression { &out } else { &input });
            if self.frame.len() >= VOICE_FRAME {
                self.finish_frame(&s);
            }
        }
    }

    /// A full 20 ms frame: decide whether the player is talking, encode, send.
    fn finish_frame(&mut self, s: &VoiceSettings) {
        let rms = (self.sum_sq / VOICE_FRAME as f32).sqrt();
        let level = if rms > 0.0 { 20.0 * rms.log10() } else { -100.0 };
        self.meters.level.store(level.max(-100.0).to_bits(), Ordering::Relaxed);
        let loud = if s.auto_sensitivity { self.vad > 0.6 && level > -60.0 } else { level > s.threshold_db };
        self.vad = 0.0;
        self.sum_sq = 0.0;
        if loud {
            self.hold = HOLD_FRAMES;
        } else {
            self.hold = self.hold.saturating_sub(1);
        }
        let talking = self.hold > 0 && !s.muted && !s.deafened;

        let mut pcm = [0i16; VOICE_FRAME];
        for (o, v) in pcm.iter_mut().zip(self.frame.drain(..)) {
            *o = v.clamp(-32768.0, 32767.0) as i16;
        }
        let was = self.meters.speaking.swap(talking, Ordering::Relaxed);
        let (Some(enc), Some(send)) = (&mut self.encoder, &self.send) else { return };
        let Some(packet) = enc.encode(&pcm) else { return };
        if talking {
            if !was {
                for p in self.recent.drain(..) {
                    send(p);
                }
            }
            send(packet);
        } else {
            self.recent.push_back(packet);
            if self.recent.len() > PRE_ROLL {
                self.recent.pop_front();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_rate() {
        let mut r = Resampler::new(44_100, 48_000);
        let mut out = Vec::new();
        r.push(&vec![0.5; 44_100], &mut out);
        assert!((out.len() as i64 - 48_000).abs() < 3, "{}", out.len());
        let mut r = Resampler::new(48_000, 44_100);
        let n = (0..44_100).map(|_| r.pull(|| 1.0)).count();
        assert_eq!(n, 44_100);
    }

    #[test]
    fn opus_round_trip() {
        let mut enc = Encoder::new().unwrap();
        let mut mixer = Mixer::default();
        let mut pcm = [0i16; VOICE_FRAME];
        for (i, s) in pcm.iter_mut().enumerate() {
            *s = ((i as f32 / 20.0).sin() * 8000.0) as i16;
        }
        for _ in 0..5 {
            let packet = enc.encode(&pcm).unwrap();
            assert!(packet.len() < 200);
            mixer.push(7, &packet);
        }
        assert!(mixer.speakers[&7].playing);
        let energy: f32 = (0..VOICE_FRAME * 4).map(|_| mixer.next().powi(2)).sum();
        assert!(energy > 1.0, "decoded audio is silent");
    }
}
