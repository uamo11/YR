use futures_util::StreamExt;
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;
use yr_core::PlaybackStatus;

fn shared_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .tcp_nodelay(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

#[derive(Debug, Clone)]
pub struct AudioSnapshot {
    pub status: PlaybackStatus,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub volume: f32,
    pub speed: f32,
    pub codec_label: Option<String>,
    pub finished_counter: u64,
}

impl Default for AudioSnapshot {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Idle,
            position_secs: 0.0,
            duration_secs: None,
            volume: 0.85,
            speed: 1.0,
            codec_label: None,
            finished_counter: 0,
        }
    }
}

enum AudioCommand {
    PlayFile {
        path: PathBuf,
        duration_hint: Option<u64>,
    },
    PlayUrl {
        url: String,
        track_id: String,
        duration_hint: Option<u64>,
    },
    Pause,
    Resume,
    Stop,
    Seek(Duration),
    SetVolume(f32),
    SetSpeed(f32),
    PrefetchUrl {
        url: String,
        track_id: String,
    },
    SetEqualizer(yr_core::EqualizerConfig),
}

/// Pure-Rust audio playback engine running on a dedicated OS thread.
#[derive(Clone)]
pub struct AudioEngine {
    tx: mpsc::Sender<AudioCommand>,
    snapshot: Arc<Mutex<AudioSnapshot>>,
}

impl AudioEngine {
    pub fn new(
        cache_dir: PathBuf,
        runtime: tokio::runtime::Handle,
        initial_volume: f32,
        initial_eq: yr_core::EqualizerConfig,
    ) -> Self {
        let _ = std::fs::create_dir_all(&cache_dir);
        let (tx, rx) = mpsc::channel::<AudioCommand>();
        let snapshot = Arc::new(Mutex::new(AudioSnapshot {
            volume: initial_volume.clamp(0.0, 1.0),
            ..Default::default()
        }));
        let snap_clone = Arc::clone(&snapshot);

        std::thread::Builder::new()
            .name("yr-audio-thread".to_string())
            .spawn(move || audio_thread_main(rx, snap_clone, cache_dir, runtime, initial_eq))
            .expect("failed to spawn yr-audio-thread");

        Self { tx, snapshot }
    }

    pub fn snapshot(&self) -> AudioSnapshot {
        self.snapshot
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    pub fn play_file(&self, path: PathBuf, duration_hint: Option<u64>) {
        let _ = self.tx.send(AudioCommand::PlayFile {
            path,
            duration_hint,
        });
    }

    pub fn play_url(&self, url: String, track_id: String, duration_hint: Option<u64>) {
        let _ = self.tx.send(AudioCommand::PlayUrl {
            url,
            track_id,
            duration_hint,
        });
    }

    pub fn pause(&self) {
        let _ = self.tx.send(AudioCommand::Pause);
    }

    pub fn resume(&self) {
        let _ = self.tx.send(AudioCommand::Resume);
    }

    pub fn stop(&self) {
        let _ = self.tx.send(AudioCommand::Stop);
    }

    pub fn seek(&self, position: Duration) {
        let _ = self.tx.send(AudioCommand::Seek(position));
    }

    pub fn set_volume(&self, volume: f32) {
        let _ = self.tx.send(AudioCommand::SetVolume(volume.clamp(0.0, 1.0)));
    }

    pub fn set_speed(&self, speed: f32) {
        let _ = self.tx.send(AudioCommand::SetSpeed(speed.clamp(0.5, 2.0)));
    }

    pub fn prefetch_url(&self, url: String, track_id: String) {
        let _ = self.tx.send(AudioCommand::PrefetchUrl { url, track_id });
    }

    pub fn set_equalizer(&self, eq: yr_core::EqualizerConfig) {
        let _ = self.tx.send(AudioCommand::SetEqualizer(eq));
    }
}

fn prune_stream_cache(cache_dir: &std::path::Path, max_bytes: u64, target_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(cache_dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total = 0_u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            let len = meta.len();
            let modified = meta
                .modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            total = total.saturating_add(len);
            files.push((path, len, modified));
        }
    }
    if total <= max_bytes {
        return;
    }
    files.sort_by_key(|(_, _, m)| *m);
    for (path, len, _) in files {
        if total <= target_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

fn audio_thread_main(
    rx: mpsc::Receiver<AudioCommand>,
    snapshot: Arc<Mutex<AudioSnapshot>>,
    cache_dir: PathBuf,
    runtime: tokio::runtime::Handle,
    initial_eq: yr_core::EqualizerConfig,
) {
    prune_stream_cache(&cache_dir, 128 * 1024 * 1024, 64 * 1024 * 1024);
    let eq_processor = Arc::new(Mutex::new(EqualizerProcessor::new(initial_eq)));
    let mut output: Option<(OutputStream, OutputStreamHandle)> = None;
    let mut sink: Option<Sink> = None;
    let mut active_cancel: Option<Arc<AtomicBool>> = None;
    let mut active_prefetch: Option<(String, Arc<AtomicBool>)> = None;
    let mut volume: f32 = snapshot.lock().map(|s| s.volume).unwrap_or(0.85);
    let mut speed: f32 = 1.0;
    let mut current_duration: Option<f64> = None;
    let mut active_playback = false;
    let mut base_pos_secs = 0.0_f64;
    let mut play_started_at: Option<Instant> = None;

    fn ensure_output(
        out: &mut Option<(OutputStream, OutputStreamHandle)>,
    ) -> Option<&OutputStreamHandle> {
        if out.is_none() {
            match OutputStream::try_default() {
                Ok(pair) => *out = Some(pair),
                Err(e) => {
                    tracing::error!("Audio output init failed: {e}");
                    return None;
                }
            }
        }
        out.as_ref().map(|(_, h)| h)
    }

    loop {
        match rx.recv_timeout(Duration::from_millis(120)) {
            Ok(cmd) => match cmd {
                AudioCommand::PlayFile {
                    path,
                    duration_hint,
                } => {
                    if let Some(c) = active_cancel.take() {
                        c.store(true, Ordering::Relaxed);
                    }
                    if let Some(s) = sink.take() {
                        s.stop();
                    }
                    active_playback = false;
                    play_started_at = None;
                    base_pos_secs = 0.0;
                    current_duration = duration_hint.filter(|&d| d > 0).map(|d| d as f64);
                    if let Ok(mut g) = snapshot.lock() {
                        g.status = PlaybackStatus::Loading;
                        g.position_secs = 0.0;
                        g.duration_secs = current_duration;
                    }

                    let Some(handle) = ensure_output(&mut output) else {
                        set_status(
                            &snapshot,
                            PlaybackStatus::Error("Sin dispositivo de audio".into()),
                        );
                        continue;
                    };

                    let ext_str = path.extension().and_then(|e| e.to_str()).map(str::to_owned);
                    let is_wav = ext_str
                        .as_deref()
                        .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
                    match File::open(&path).map_err(|e| e.to_string()).and_then(|f| {
                        SymphoniaStreamDecoder::new(
                            Box::new(f),
                            ext_str.as_deref(),
                            None,
                            Arc::clone(&eq_processor),
                        )
                    }) {
                        Ok(decoder) => {
                            let codec_badge = decoder.codec_label.clone();
                            let dur = if is_wav {
                                duration_hint
                                    .filter(|&d| d > 0)
                                    .map(|d| d as f64)
                                    .or_else(|| decoder.total_duration().map(|d| d.as_secs_f64()))
                            } else {
                                decoder
                                    .total_duration()
                                    .map(|d| d.as_secs_f64())
                                    .or_else(|| duration_hint.filter(|&d| d > 0).map(|d| d as f64))
                            };
                            match Sink::try_new(handle) {
                                Ok(new_sink) => {
                                    new_sink.set_volume(volume);
                                    new_sink.set_speed(speed);
                                    new_sink.append(decoder);
                                    sink = Some(new_sink);
                                    current_duration = dur;
                                    base_pos_secs = 0.0;
                                    play_started_at = Some(Instant::now());
                                    active_playback = true;
                                    if let Ok(mut g) = snapshot.lock() {
                                        g.status = PlaybackStatus::Playing;
                                        g.position_secs = 0.0;
                                        g.duration_secs = dur;
                                        g.codec_label = Some(codec_badge);
                                    }
                                }
                                Err(e) => {
                                    set_status(&snapshot, PlaybackStatus::Error(e.to_string()));
                                }
                            }
                        }
                        Err(e) => {
                            set_status(&snapshot, PlaybackStatus::Error(e));
                        }
                    }
                }
                AudioCommand::PlayUrl {
                    url,
                    track_id,
                    duration_hint,
                } => {
                    if let Some(c) = active_cancel.take() {
                        c.store(true, Ordering::Relaxed);
                    }
                    if let Some((pid, pc)) = active_prefetch.take() {
                        if pid == track_id {
                            pc.store(true, Ordering::Relaxed);
                        } else {
                            active_prefetch = Some((pid, pc));
                        }
                    }
                    if let Some(s) = sink.take() {
                        s.stop();
                    }
                    active_playback = false;
                    play_started_at = None;
                    base_pos_secs = 0.0;
                    current_duration = duration_hint.filter(|&d| d > 0).map(|d| d as f64);
                    if let Ok(mut g) = snapshot.lock() {
                        g.status = PlaybackStatus::Buffering;
                        g.position_secs = 0.0;
                        g.duration_secs = current_duration;
                    }

                    let Some(handle) = ensure_output(&mut output) else {
                        set_status(
                            &snapshot,
                            PlaybackStatus::Error("Sin dispositivo de audio".into()),
                        );
                        continue;
                    };

                    let final_cache = cache_dir.join(format!("stream_{track_id}.audio"));
                    let cached_ready = std::fs::metadata(&final_cache)
                        .map(|m| m.len() > 4096)
                        .unwrap_or(false);
                    let decoder_res = if cached_ready {
                        File::open(&final_cache)
                            .map_err(|e| e.to_string())
                            .and_then(|f| {
                                SymphoniaStreamDecoder::new(
                                    Box::new(f),
                                    Some("m4a"),
                                    None,
                                    Arc::clone(&eq_processor),
                                )
                            })
                    } else {
                        let cancel = Arc::new(AtomicBool::new(false));
                        active_cancel = Some(Arc::clone(&cancel));
                        let tmp_file = cache_dir.join(format!("stream_{track_id}.tmp"));
                        open_progressive_stream(
                            &runtime,
                            &url,
                            tmp_file,
                            Some(final_cache),
                            cancel,
                        )
                        .and_then(|reader| {
                            let probed_flag = Arc::clone(&reader.probed);
                            SymphoniaStreamDecoder::new(
                                Box::new(reader),
                                Some("m4a"),
                                Some(probed_flag),
                                Arc::clone(&eq_processor),
                            )
                        })
                    };

                    match decoder_res {
                        Ok(decoder) => {
                            let dur = decoder
                                .total_duration()
                                .map(|d| d.as_secs_f64())
                                .or(duration_hint.map(|d| d as f64));
                            let codec_badge = decoder.codec_label.clone();
                            match Sink::try_new(handle) {
                                Ok(new_sink) => {
                                    new_sink.set_volume(volume);
                                    new_sink.set_speed(speed);
                                    new_sink.append(decoder);
                                    sink = Some(new_sink);
                                    current_duration = dur;
                                    base_pos_secs = 0.0;
                                    play_started_at = Some(Instant::now());
                                    active_playback = true;
                                    if let Ok(mut g) = snapshot.lock() {
                                        g.status = PlaybackStatus::Playing;
                                        g.position_secs = 0.0;
                                        g.duration_secs = dur;
                                        g.codec_label = Some(codec_badge);
                                    }
                                }
                                Err(e) => {
                                    set_status(&snapshot, PlaybackStatus::Error(e.to_string()));
                                }
                            }
                        }
                        Err(e) => {
                            set_status(&snapshot, PlaybackStatus::Error(e));
                        }
                    }
                }
                AudioCommand::PrefetchUrl { url, track_id } => {
                    let final_cache = cache_dir.join(format!("stream_{track_id}.audio"));
                    if std::fs::metadata(&final_cache)
                        .map(|m| m.len() > 4096)
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    if let Some((pid, pc)) = active_prefetch.take() {
                        if pid == track_id {
                            active_prefetch = Some((pid, pc));
                            continue;
                        }
                        pc.store(true, Ordering::Relaxed);
                    }
                    prune_stream_cache(&cache_dir, 128 * 1024 * 1024, 64 * 1024 * 1024);
                    let cancel = Arc::new(AtomicBool::new(false));
                    active_prefetch = Some((track_id.clone(), Arc::clone(&cancel)));
                    let part_path = cache_dir.join(format!("stream_{track_id}.pre.part"));
                    prefetch_stream_to_cache(&runtime, url, part_path, final_cache, cancel);
                }
                AudioCommand::Pause => {
                    if let Some(s) = &sink {
                        s.pause();
                        if let Some(started) = play_started_at.take() {
                            base_pos_secs += started.elapsed().as_secs_f64() * speed as f64;
                        }
                        set_status(&snapshot, PlaybackStatus::Paused);
                    }
                }
                AudioCommand::Resume => {
                    if let Some(s) = &sink {
                        s.play();
                        play_started_at = Some(Instant::now());
                        set_status(&snapshot, PlaybackStatus::Playing);
                    }
                }
                AudioCommand::Stop => {
                    if let Some(c) = active_cancel.take() {
                        c.store(true, Ordering::Relaxed);
                    }
                    if let Some(s) = sink.take() {
                        s.stop();
                    }
                    active_playback = false;
                    play_started_at = None;
                    base_pos_secs = 0.0;
                    current_duration = None;
                    if let Ok(mut g) = snapshot.lock() {
                        g.status = PlaybackStatus::Idle;
                        g.position_secs = 0.0;
                        g.duration_secs = None;
                        g.codec_label = None;
                    }
                }
                AudioCommand::Seek(pos) => {
                    if let Some(s) = &sink {
                        match s.try_seek(pos) {
                            Ok(()) => {
                                base_pos_secs = pos.as_secs_f64();
                                if !s.is_paused() {
                                    play_started_at = Some(Instant::now());
                                }
                                if let Ok(mut g) = snapshot.lock() {
                                    g.position_secs = base_pos_secs;
                                }
                            }
                            Err(e) => {
                                eprintln!(
                                    "[yr-audio] try_seek({:.2}s) failed: {e}",
                                    pos.as_secs_f64()
                                );
                            }
                        }
                    }
                }
                AudioCommand::SetVolume(v) => {
                    volume = v;
                    if let Some(s) = &sink {
                        s.set_volume(volume);
                    }
                    if let Ok(mut g) = snapshot.lock() {
                        g.volume = volume;
                    }
                }
                AudioCommand::SetSpeed(new_speed) => {
                    let clamped = new_speed.clamp(0.5, 2.0);
                    if let Some(s) = &sink {
                        if !s.is_paused() {
                            if let Some(started) = play_started_at.take() {
                                base_pos_secs += started.elapsed().as_secs_f64() * speed as f64;
                            }
                            play_started_at = Some(Instant::now());
                        }
                        s.set_speed(clamped);
                    }
                    speed = clamped;
                    if let Ok(mut g) = snapshot.lock() {
                        g.speed = speed;
                    }
                }
                AudioCommand::SetEqualizer(eq_cfg) => {
                    if let Ok(mut ep) = eq_processor.lock() {
                        ep.set_config(eq_cfg);
                    }
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        // Update position and detect natural track end.
        if active_playback {
            if let Some(s) = &sink {
                if s.empty() {
                    active_playback = false;
                    play_started_at = None;
                    if let Ok(mut g) = snapshot.lock() {
                        g.status = PlaybackStatus::Idle;
                        g.position_secs = current_duration.unwrap_or(0.0);
                        g.finished_counter += 1;
                    }
                } else if !s.is_paused() {
                    let pos = base_pos_secs
                        + play_started_at
                            .map(|t| t.elapsed().as_secs_f64() * speed as f64)
                            .unwrap_or(0.0);
                    if let Ok(mut g) = snapshot.lock() {
                        g.position_secs = pos;
                    }
                }
            }
        }
    }
}

fn set_status(snapshot: &Arc<Mutex<AudioSnapshot>>, status: PlaybackStatus) {
    if let Ok(mut g) = snapshot.lock() {
        g.status = status;
    }
}

struct StreamState {
    written: u64,
    total: Option<u64>,
    finished: bool,
    error: Option<String>,
}

/// Progressive HTTP-to-temp-file reader implementing `Read + Seek` for `rodio::Decoder`.
struct ProgressiveFileReader {
    file: File,
    pos: u64,
    state: Arc<(Mutex<StreamState>, Condvar)>,
    path: PathBuf,
    cancel: Arc<AtomicBool>,
    probed: Arc<AtomicBool>,
}

impl Drop for ProgressiveFileReader {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Read for ProgressiveFileReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let (lock, cvar) = &*self.state;
        let mut guard = lock.lock().map_err(|_| io::Error::other("poisoned"))?;

        while self.pos >= guard.written && !guard.finished {
            if self.cancel.load(Ordering::Relaxed) {
                return Ok(0);
            }
            if let Some(err) = &guard.error {
                return Err(io::Error::other(err.clone()));
            }
            guard = cvar
                .wait(guard)
                .map_err(|_| io::Error::other("poisoned"))?;
        }

        if self.pos >= guard.written && guard.finished {
            return Ok(0);
        }
        drop(guard);

        self.file.seek(SeekFrom::Start(self.pos))?;
        let n = self.file.read(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for ProgressiveFileReader {
    fn seek(&mut self, style: SeekFrom) -> io::Result<u64> {
        let (lock, cvar) = &*self.state;
        let target: i64 = match style {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::Current(delta) => self.pos as i64 + delta,
            SeekFrom::End(delta) => {
                let mut guard = lock.lock().map_err(|_| io::Error::other("poisoned"))?;
                while guard.total.is_none() && !guard.finished {
                    if let Some(err) = &guard.error {
                        return Err(io::Error::other(err.clone()));
                    }
                    guard = cvar
                        .wait(guard)
                        .map_err(|_| io::Error::other("poisoned"))?;
                }
                let end = guard.total.unwrap_or(guard.written) as i64;
                end + delta
            }
        };

        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "negative seek position",
            ));
        }

        let target_u64 = target as u64;
        self.pos = target_u64;
        Ok(self.pos)
    }
}

impl MediaSource for ProgressiveFileReader {
    fn is_seekable(&self) -> bool {
        // Keep unseekable during format probing so `isomp4` starts playback immediately
        // after `moov` instead of scanning every `mdat`/`moof` box across the file.
        if !self.probed.load(Ordering::Acquire) {
            return false;
        }
        let (lock, _) = &*self.state;
        lock.lock()
            .map(|g| g.total.is_some() || g.finished)
            .unwrap_or(false)
    }

    fn byte_len(&self) -> Option<u64> {
        if !self.probed.load(Ordering::Acquire) {
            return None;
        }
        let (lock, _) = &*self.state;
        lock.lock().ok().and_then(|g| g.total)
    }
}

#[derive(Debug, Clone, Copy)]
struct BiquadCoeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl BiquadCoeffs {
    fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    fn peaking(sample_rate: f32, f0: f32, gain_db: f32, q: f32) -> Self {
        if gain_db.abs() < 0.05 {
            return Self::identity();
        }
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * (f0 / sample_rate).clamp(0.0001, 0.499);
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let cos_w0 = w0.cos();

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    fn low_shelf(sample_rate: f32, f0: f32, gain_db: f32, q: f32) -> Self {
        if gain_db.abs() < 0.05 {
            return Self::identity();
        }
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * (f0 / sample_rate).clamp(0.0001, 0.499);
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let cos_w0 = w0.cos();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
        let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    fn high_shelf(sample_rate: f32, f0: f32, gain_db: f32, q: f32) -> Self {
        if gain_db.abs() < 0.05 {
            return Self::identity();
        }
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * (f0 / sample_rate).clamp(0.0001, 0.499);
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let cos_w0 = w0.cos();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct FilterChannelState {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl FilterChannelState {
    #[inline(always)]
    fn process(&mut self, x: f32, c: &BiquadCoeffs) -> f32 {
        let y = c.b0 * x + c.b1 * self.x1 + c.b2 * self.x2 - c.a1 * self.y1 - c.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

pub struct EqualizerProcessor {
    pub config: yr_core::EqualizerConfig,
    sample_rate: u32,
    channels: usize,
    coeffs: Vec<BiquadCoeffs>,
    states: Vec<Vec<FilterChannelState>>,
    smooth_gain: f32,
    power_ema: f32,
}

impl EqualizerProcessor {
    pub fn new(config: yr_core::EqualizerConfig) -> Self {
        let mut ep = Self {
            config,
            sample_rate: 44100,
            channels: 2,
            coeffs: Vec::new(),
            states: Vec::new(),
            smooth_gain: 1.0,
            power_ema: 0.18 * 0.18,
        };
        ep.recompute();
        ep
    }

    pub fn set_config(&mut self, config: yr_core::EqualizerConfig) {
        let changed = self.config != config;
        self.config = config;
        if changed {
            self.recompute();
        }
    }

    fn recompute(&mut self) {
        self.coeffs.clear();
        let sr = self.sample_rate as f32;

        if self.config.morphe_dynamic_bass {
            self.coeffs.push(BiquadCoeffs::low_shelf(sr, 80.0, 3.8, 0.707));
        }
        if self.config.morphe_vocal_boost {
            self.coeffs.push(BiquadCoeffs::peaking(sr, 2500.0, 3.5, 1.0));
        }

        if self.config.enabled {
            if self.config.bass_boost_db.abs() >= 0.05 {
                self.coeffs.push(BiquadCoeffs::low_shelf(sr, 80.0, self.config.bass_boost_db, 0.707));
            }
            if self.config.bands_db[0].abs() >= 0.05 {
                self.coeffs.push(BiquadCoeffs::low_shelf(sr, 60.0, self.config.bands_db[0], 0.9));
            }
            if self.config.bands_db[1].abs() >= 0.05 {
                self.coeffs.push(BiquadCoeffs::peaking(sr, 230.0, self.config.bands_db[1], 1.2));
            }
            if self.config.bands_db[2].abs() >= 0.05 {
                self.coeffs.push(BiquadCoeffs::peaking(sr, 910.0, self.config.bands_db[2], 1.2));
            }
            if self.config.bands_db[3].abs() >= 0.05 {
                self.coeffs.push(BiquadCoeffs::peaking(sr, 3600.0, self.config.bands_db[3], 1.2));
            }
            if self.config.bands_db[4].abs() >= 0.05 {
                self.coeffs.push(BiquadCoeffs::high_shelf(sr, 14000.0, self.config.bands_db[4], 0.9));
            }
        }
        self.states = vec![vec![FilterChannelState::default(); self.channels]; self.coeffs.len()];
    }

    pub fn process(&mut self, samples: &mut [i16], rate: u32, channels: usize) {
        let has_morphe = self.config.morphe_spatial_audio
            || self.config.morphe_vocal_boost
            || self.config.morphe_dynamic_bass
            || self.config.morphe_limiter;

        if !self.config.enabled && !self.config.loudness_normalization && !has_morphe {
            return;
        }
        let safe_ch = channels.clamp(1, 8);
        if self.sample_rate != rate || self.channels != safe_ch {
            self.sample_rate = rate.max(8000);
            self.channels = safe_ch;
            self.recompute();
        }

        let do_eq = !self.coeffs.is_empty();
        let do_norm = self.config.loudness_normalization;

        for (i, s) in samples.iter_mut().enumerate() {
            let ch = i % safe_ch;
            let mut x = *s as f32 / 32768.0;

            if do_eq {
                for (stage_idx, c) in self.coeffs.iter().enumerate() {
                    if let Some(ch_states) = self.states.get_mut(stage_idx) {
                        if let Some(st) = ch_states.get_mut(ch) {
                            x = st.process(x, c);
                        }
                    }
                }
            }

            if do_norm {
                self.power_ema = self.power_ema * 0.9999 + (x * x) * 0.0001;
                let rms = self.power_ema.sqrt().max(0.008);
                let target_gain = (0.18 / rms).clamp(0.45, 2.2);
                self.smooth_gain = self.smooth_gain * 0.9996 + target_gain * 0.0004;
                x *= self.smooth_gain;
            }

            let clamped = x.clamp(-1.2, 1.2);
            let soft = if self.config.morphe_limiter {
                if clamped > 1.0 {
                    1.0 - (-clamped + 1.0).exp() * 0.1
                } else if clamped < -1.0 {
                    -1.0 + (clamped + 1.0).exp() * 0.1
                } else {
                    clamped
                }
            } else {
                clamped.clamp(-1.0, 1.0)
            };
            *s = (soft * 32767.0).clamp(-32768.0, 32767.0) as i16;
        }

        // Morphe 3D Binaural Spatial Expander (Mid-Side Stereo Widening)
        if safe_ch == 2 && self.config.morphe_spatial_audio {
            for chunk in samples.chunks_exact_mut(2) {
                let l = chunk[0] as f32;
                let r = chunk[1] as f32;
                let mid = (l + r) * 0.5;
                let side = (l - r) * 0.5;
                let wide_side = side * 1.38;
                let wide_mid = mid * 0.96;
                chunk[0] = (wide_mid + wide_side).clamp(-32768.0, 32767.0) as i16;
                chunk[1] = (wide_mid - wide_side).clamp(-32768.0, 32767.0) as i16;
            }
        }
    }
}

struct SymphoniaStreamDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    spec: SignalSpec,
    codec_label: String,
    samples: Vec<i16>,
    sample_idx: usize,
    total_duration: Option<Duration>,
    eof: bool,
    eq: Arc<Mutex<EqualizerProcessor>>,
}

impl SymphoniaStreamDecoder {
    fn new(
        source: Box<dyn MediaSource>,
        ext_hint: Option<&str>,
        probed_flag: Option<Arc<AtomicBool>>,
        eq: Arc<Mutex<EqualizerProcessor>>,
    ) -> Result<Self, String> {
        let mss = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        hint.with_extension(ext_hint.unwrap_or("m4a"));
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(|e| e.to_string())?;
        if let Some(flag) = probed_flag {
            flag.store(true, Ordering::Release);
        }

        let (track_id, codec_params, decoder) = probed
            .format
            .tracks()
            .iter()
            .filter(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .find_map(|t| {
                symphonia::default::get_codecs()
                    .make(&t.codec_params, &DecoderOptions::default())
                    .ok()
                    .map(|d| (t.id, t.codec_params.clone(), d))
            })
            .ok_or_else(|| "Sin pista de audio compatible".to_string())?;
        let total_duration = codec_params
            .time_base
            .zip(codec_params.n_frames)
            .map(|(tb, frames)| {
                let t = tb.calc_time(frames);
                Duration::from_secs(t.seconds) + Duration::from_secs_f64(t.frac)
            });

        let spec = SignalSpec::new(
            codec_params.sample_rate.unwrap_or(44_100),
            codec_params
                .channels
                .unwrap_or_else(|| symphonia::core::audio::Channels::FRONT_LEFT | symphonia::core::audio::Channels::FRONT_RIGHT),
        );
        let codec_name = match codec_params.codec {
            symphonia::core::codecs::CODEC_TYPE_AAC => "AAC".to_string(),
            symphonia::core::codecs::CODEC_TYPE_OPUS => "OPUS".to_string(),
            symphonia::core::codecs::CODEC_TYPE_VORBIS => "VORBIS".to_string(),
            symphonia::core::codecs::CODEC_TYPE_MP3 => "MP3".to_string(),
            symphonia::core::codecs::CODEC_TYPE_FLAC => "FLAC".to_string(),
            _ => ext_hint
                .filter(|s| !s.is_empty())
                .unwrap_or("AUDIO")
                .to_ascii_uppercase(),
        };
        let khz = spec.rate as f64 / 1000.0;
        let codec_label = format!("{codec_name} • {khz:.1} kHz");

        let mut out = Self {
            format: probed.format,
            decoder,
            track_id,
            spec,
            codec_label,
            samples: Vec::new(),
            sample_idx: 0,
            total_duration,
            eof: false,
            eq,
        };
        out.decode_next_packet();
        Ok(out)
    }

    fn decode_next_packet(&mut self) -> bool {
        self.samples.clear();
        self.sample_idx = 0;
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(_) => {
                    self.eof = true;
                    return false;
                }
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    if spec.channels.count() > 0 && spec.rate > 0 {
                        self.spec = spec;
                    }
                    let mut buf = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
                    buf.copy_interleaved_ref(decoded);
                    self.samples.extend_from_slice(buf.samples());
                    if !self.samples.is_empty() {
                        if let Ok(mut ep) = self.eq.lock() {
                            ep.process(&mut self.samples, self.spec.rate, self.spec.channels.count());
                        }
                        return true;
                    }
                }
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(_) => {
                    self.eof = true;
                    return false;
                }
            }
        }
    }
}

impl Iterator for SymphoniaStreamDecoder {
    type Item = i16;

    fn next(&mut self) -> Option<Self::Item> {
        if self.sample_idx >= self.samples.len() {
            if self.eof || !self.decode_next_packet() {
                return None;
            }
        }
        let s = self.samples[self.sample_idx];
        self.sample_idx += 1;
        Some(s)
    }
}

impl Source for SymphoniaStreamDecoder {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.samples.len().saturating_sub(self.sample_idx))
    }

    fn channels(&self) -> u16 {
        self.spec.channels.count().max(1) as u16
    }

    fn sample_rate(&self) -> u32 {
        self.spec.rate.max(1)
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let target = Time::new(pos.as_secs(), f64::from(pos.subsec_nanos()) / 1_000_000_000.0);
        self.format
            .seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time: target,
                    track_id: None,
                },
            )
            .map_err(|e| {
                rodio::source::SeekError::Other(Box::new(io::Error::other(e.to_string())))
            })?;
        self.decoder.reset();
        self.samples.clear();
        self.sample_idx = 0;
        self.eof = false;
        self.decode_next_packet();
        Ok(())
    }
}

fn stream_client_ua(url: &str) -> Option<&'static str> {
    if url.contains("c=MWEB") || url.contains("c=WEB") {
        Some("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36")
    } else if url.contains("c=VISIONOS") {
        Some("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_6) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15")
    } else if url.contains("c=IOS") {
        Some("com.google.ios.youtube/19.29.1 (iPhone16,2; U; CPU iOS 17_5_1 like Mac OS X;)")
    } else if url.contains("c=ANDROID_VR") {
        Some("com.google.android.apps.youtube.vr.oculus/1.60.19 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip")
    } else {
        Some("Mozilla/5.0 (Linux; Android 14; Mobile) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36")
    }
}

fn stream_request_builder(
    client: &reqwest::Client,
    url: &str,
    range_val: Option<String>,
) -> reqwest::RequestBuilder {
    let mut r = client.get(url);
    if let Some(ua) = stream_client_ua(url) {
        r = r.header(reqwest::header::USER_AGENT, ua);
    }
    if url.contains("c=MWEB") || url.contains("c=WEB") {
        r = r
            .header(reqwest::header::ORIGIN, "https://m.youtube.com")
            .header(reqwest::header::REFERER, "https://m.youtube.com/");
    }
    if let Some(rv) = range_val {
        r = r.header(reqwest::header::RANGE, rv);
    }
    r
}

fn open_progressive_stream(
    runtime: &tokio::runtime::Handle,
    url: &str,
    path: PathBuf,
    final_cache: Option<PathBuf>,
    cancel: Arc<AtomicBool>,
) -> Result<ProgressiveFileReader, String> {
    let mut writer = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let reader_file = File::open(&path).map_err(|e| e.to_string())?;

    let state = Arc::new((
        Mutex::new(StreamState {
            written: 0,
            total: None,
            finished: false,
            error: None,
        }),
        Condvar::new(),
    ));

    let state_bg = Arc::clone(&state);
    let url_owned = url.to_string();
    let cancel_bg = Arc::clone(&cancel);
    let path_bg = path.clone();

    runtime.spawn(async move {
        const CHUNK_SIZE: u64 = 524_288;
        let client = shared_http_client();
        let build_req = |range_val: Option<String>| stream_request_builder(client, &url_owned, range_val);

        let set_err = |msg: String| {
            tracing::warn!("progressive stream error: {msg}");
            let (lock, cvar) = &*state_bg;
            if let Ok(mut g) = lock.lock() {
                g.error = Some(msg);
                g.finished = true;
            }
            cvar.notify_all();
        };

        let first_resp = match build_req(Some(format!("bytes=0-{}", CHUNK_SIZE - 1)))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() || r.status() == reqwest::StatusCode::PARTIAL_CONTENT => r,
            Ok(r) => {
                set_err(format!("HTTP {}", r.status()));
                return;
            }
            Err(e) => {
                set_err(e.to_string());
                return;
            }
        };

        let is_partial = first_resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut total = if is_partial {
            first_resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.rsplit('/').next())
                .and_then(|n| n.parse::<u64>().ok())
        } else {
            first_resp.content_length()
        };

        {
            let (lock, cvar) = &*state_bg;
            if let Ok(mut g) = lock.lock() {
                g.total = total;
            }
            cvar.notify_all();
        }

        let mut written_total: u64 = 0;
        let mut current_resp = Some(first_resp);

        while let Some(resp) = current_resp.take() {
            let mut chunk_written: u64 = 0;
            let mut stream = resp.bytes_stream();
            while let Some(item) = stream.next().await {
                if cancel_bg.load(Ordering::Relaxed) {
                    break;
                }
                match item {
                    Ok(bytes) => {
                        if let Err(e) = writer.write_all(&bytes) {
                            set_err(e.to_string());
                            return;
                        }
                        let len = bytes.len() as u64;
                        written_total += len;
                        chunk_written += len;
                        let (lock, cvar) = &*state_bg;
                        if let Ok(mut g) = lock.lock() {
                            g.written += len;
                        }
                        cvar.notify_all();
                    }
                    Err(e) => {
                        set_err(e.to_string());
                        return;
                    }
                }
            }

            if cancel_bg.load(Ordering::Relaxed) || !is_partial || chunk_written == 0 {
                break;
            }
            if let Some(t) = total {
                if written_total >= t {
                    break;
                }
            }

            let end = match total {
                Some(t) => (written_total + CHUNK_SIZE - 1).min(t.saturating_sub(1)),
                None => written_total + CHUNK_SIZE - 1,
            };
            match build_req(Some(format!("bytes={written_total}-{end}")))
                .send()
                .await
            {
                Ok(r) if r.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE => break,
                Ok(r) if r.status().is_success() || r.status() == reqwest::StatusCode::PARTIAL_CONTENT => {
                    if total.is_none() {
                        total = r
                            .headers()
                            .get(reqwest::header::CONTENT_RANGE)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|s| s.rsplit('/').next())
                            .and_then(|n| n.parse::<u64>().ok());
                        if let Some(t) = total {
                            let (lock, cvar) = &*state_bg;
                            if let Ok(mut g) = lock.lock() {
                                g.total = Some(t);
                            }
                            cvar.notify_all();
                        }
                    }
                    current_resp = Some(r);
                }
                Ok(r) => {
                    set_err(format!("HTTP {}", r.status()));
                    return;
                }
                Err(e) => {
                    set_err(e.to_string());
                    return;
                }
            }
        }

        let _ = writer.flush();
        if written_total > 4096 && total.is_none_or(|t| written_total >= t) {
            if let Some(dest) = final_cache {
                let _ = std::fs::rename(&path_bg, dest);
            }
        }
        let (lock, cvar) = &*state_bg;
        if let Ok(mut g) = lock.lock() {
            g.finished = true;
        }
        cvar.notify_all();
    });

    // Wait briefly for initial bytes (header) so Decoder::new doesn't fail on an empty file.
    {
        let (lock, cvar) = &*state;
        let mut guard = lock.lock().map_err(|_| "poisoned".to_string())?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while guard.written < 16_384 && !guard.finished {
            if let Some(err) = &guard.error {
                return Err(err.clone());
            }
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                break;
            }
            let (g, _) = cvar
                .wait_timeout(guard, rem)
                .map_err(|_| "poisoned".to_string())?;
            guard = g;
        }
        if let Some(err) = &guard.error {
            return Err(err.clone());
        }
        if guard.written == 0 {
            return Err("Stream vacío (0 bytes)".to_string());
        }
    }

    let probed = Arc::new(AtomicBool::new(false));
    Ok(ProgressiveFileReader {
        file: reader_file,
        pos: 0,
        state,
        path,
        cancel,
        probed,
    })
}

fn prefetch_stream_to_cache(
    runtime: &tokio::runtime::Handle,
    url: String,
    part_path: PathBuf,
    final_path: PathBuf,
    cancel: Arc<AtomicBool>,
) {
    runtime.spawn(async move {
        let Ok(mut writer) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&part_path)
        else {
            return;
        };
        let client = shared_http_client();
        let Ok(resp) = stream_request_builder(client, &url, None).send().await else {
            let _ = std::fs::remove_file(&part_path);
            return;
        };
        if !resp.status().is_success() {
            let _ = std::fs::remove_file(&part_path);
            return;
        }
        let expected_len = resp.content_length();
        let mut written = 0_u64;
        let mut stream = resp.bytes_stream();
        while let Some(item) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                let _ = std::fs::remove_file(&part_path);
                return;
            }
            match item {
                Ok(bytes) => {
                    if writer.write_all(&bytes).is_err() {
                        let _ = std::fs::remove_file(&part_path);
                        return;
                    }
                    written += bytes.len() as u64;
                    // Guard against caching unusually huge multi-hour mixes in RAM/tmp cache.
                    if written > 24 * 1024 * 1024 {
                        let _ = std::fs::remove_file(&part_path);
                        return;
                    }
                }
                Err(_) => {
                    let _ = std::fs::remove_file(&part_path);
                    return;
                }
            }
        }
        let _ = writer.flush();
        if written > 4096 && expected_len.is_none_or(|t| written >= t) {
            let _ = std::fs::rename(&part_path, &final_path);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_client_ua_detects_mweb_ios_visionos() {
        assert!(stream_client_ua("https://rr1.googlevideo.com/videoplayback?c=MWEB")
            .unwrap_or("")
            .contains("Android"));
        assert!(stream_client_ua("https://rr1.googlevideo.com/videoplayback?c=IOS")
            .unwrap_or("")
            .contains("iPhone"));
        assert!(stream_client_ua("https://rr1.googlevideo.com/videoplayback?c=VISIONOS")
            .unwrap_or("")
            .contains("Macintosh"));
        assert!(stream_client_ua("https://rr1.googlevideo.com/videoplayback?c=ANDROID_VR")
            .unwrap_or("")
            .contains("oculus"));
        assert!(stream_client_ua("https://rr1.googlevideo.com/videoplayback?c=ANDROID")
            .unwrap_or("")
            .contains("Mobile"));
    }

    #[test]
    fn prune_stream_cache_evicts_oldest_when_over_limit() {
        let dir = std::env::temp_dir().join(format!("yr_audio_prune_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.audio"), vec![0u8; 60]).unwrap();
        std::thread::sleep(Duration::from_millis(15));
        std::fs::write(dir.join("b.audio"), vec![0u8; 60]).unwrap();
        prune_stream_cache(&dir, 80, 60);
        let remaining: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|x| x.file_name().to_string_lossy().into_owned()))
            .collect();
        assert_eq!(remaining, vec!["b.audio".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
