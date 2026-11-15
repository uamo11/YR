use futures_util::StreamExt;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
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
    pub finished_counter: u64,
}

impl Default for AudioSnapshot {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Idle,
            position_secs: 0.0,
            duration_secs: None,
            volume: 0.85,
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
}

/// Pure-Rust audio playback engine running on a dedicated OS thread.
#[derive(Clone)]
pub struct AudioEngine {
    tx: mpsc::Sender<AudioCommand>,
    snapshot: Arc<Mutex<AudioSnapshot>>,
}

impl AudioEngine {
    pub fn new(cache_dir: PathBuf, runtime: tokio::runtime::Handle, initial_volume: f32) -> Self {
        let _ = std::fs::create_dir_all(&cache_dir);
        let (tx, rx) = mpsc::channel::<AudioCommand>();
        let snapshot = Arc::new(Mutex::new(AudioSnapshot {
            volume: initial_volume.clamp(0.0, 1.0),
            ..Default::default()
        }));
        let snap_clone = Arc::clone(&snapshot);

        std::thread::Builder::new()
            .name("yr-audio-thread".to_string())
            .spawn(move || audio_thread_main(rx, snap_clone, cache_dir, runtime))
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
}

fn audio_thread_main(
    rx: mpsc::Receiver<AudioCommand>,
    snapshot: Arc<Mutex<AudioSnapshot>>,
    cache_dir: PathBuf,
    runtime: tokio::runtime::Handle,
) {
    let mut output: Option<(OutputStream, OutputStreamHandle)> = None;
    let mut sink: Option<Sink> = None;
    let mut active_cancel: Option<Arc<AtomicBool>> = None;
    let mut volume: f32 = snapshot.lock().map(|s| s.volume).unwrap_or(0.85);
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
                    set_status(&snapshot, PlaybackStatus::Loading);

                    let Some(handle) = ensure_output(&mut output) else {
                        set_status(
                            &snapshot,
                            PlaybackStatus::Error("Sin dispositivo de audio".into()),
                        );
                        continue;
                    };

                    match File::open(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|f| Decoder::new(BufReader::new(f)).map_err(|e| e.to_string()))
                    {
                        Ok(decoder) => {
                            let dur = decoder
                                .total_duration()
                                .map(|d| d.as_secs_f64())
                                .or(duration_hint.map(|d| d as f64));
                            match Sink::try_new(handle) {
                                Ok(new_sink) => {
                                    new_sink.set_volume(volume);
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
                    if let Some(s) = sink.take() {
                        s.stop();
                    }
                    set_status(&snapshot, PlaybackStatus::Buffering);

                    let Some(handle) = ensure_output(&mut output) else {
                        set_status(
                            &snapshot,
                            PlaybackStatus::Error("Sin dispositivo de audio".into()),
                        );
                        continue;
                    };

                    let cancel = Arc::new(AtomicBool::new(false));
                    active_cancel = Some(Arc::clone(&cancel));
                    let cache_file = cache_dir.join(format!("stream_{track_id}.tmp"));

                    match open_progressive_stream(&runtime, &url, cache_file, cancel) {
                        Ok(reader) => {
                            let probed_flag = Arc::clone(&reader.probed);
                            match SymphoniaStreamDecoder::new(Box::new(reader), Some(probed_flag)) {
                            Ok(decoder) => {
                                let dur = decoder
                                    .total_duration()
                                    .map(|d| d.as_secs_f64())
                                    .or(duration_hint.map(|d| d as f64));
                                match Sink::try_new(handle) {
                                    Ok(new_sink) => {
                                        new_sink.set_volume(volume);
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
                        Err(e) => {
                            set_status(&snapshot, PlaybackStatus::Error(e));
                        }
                    }
                }
                AudioCommand::Pause => {
                    if let Some(s) = &sink {
                        s.pause();
                        if let Some(started) = play_started_at.take() {
                            base_pos_secs += started.elapsed().as_secs_f64();
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
                    if let Ok(mut g) = snapshot.lock() {
                        g.status = PlaybackStatus::Idle;
                        g.position_secs = 0.0;
                    }
                }
                AudioCommand::Seek(pos) => {
                    if let Some(s) = &sink {
                        if s.try_seek(pos).is_ok() {
                            base_pos_secs = pos.as_secs_f64();
                            if !s.is_paused() {
                                play_started_at = Some(Instant::now());
                            }
                            if let Ok(mut g) = snapshot.lock() {
                                g.position_secs = base_pos_secs;
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
                            .map(|t| t.elapsed().as_secs_f64())
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

struct SymphoniaStreamDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    spec: SignalSpec,
    samples: Vec<i16>,
    sample_idx: usize,
    total_duration: Option<Duration>,
    eof: bool,
}

impl SymphoniaStreamDecoder {
    fn new(source: Box<dyn MediaSource>, probed_flag: Option<Arc<AtomicBool>>) -> Result<Self, String> {
        let mss = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        hint.with_extension("m4a");
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

        let mut out = Self {
            format: probed.format,
            decoder,
            track_id,
            spec,
            samples: Vec::new(),
            sample_idx: 0,
            total_duration,
            eof: false,
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
                    track_id: Some(self.track_id),
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

fn open_progressive_stream(
    runtime: &tokio::runtime::Handle,
    url: &str,
    path: PathBuf,
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

    runtime.spawn(async move {
        const CHUNK_SIZE: u64 = 524_288;
        let client = shared_http_client();
        let ua: Option<&'static str> = if url_owned.contains("c=MWEB") || url_owned.contains("c=WEB") {
            Some("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36")
        } else if url_owned.contains("c=VISIONOS") {
            Some("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_6) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15")
        } else if url_owned.contains("c=IOS") {
            Some("com.google.ios.youtube/19.29.1 (iPhone16,2; U; CPU iOS 17_5_1 like Mac OS X;)")
        } else {
            None
        };
        let is_web_client = url_owned.contains("c=MWEB") || url_owned.contains("c=WEB");

        let build_req = |range_val: Option<String>| {
            let mut r = client.get(&url_owned);
            if let Some(u) = ua {
                r = r.header(reqwest::header::USER_AGENT, u);
            }
            if is_web_client {
                r = r
                    .header(reqwest::header::ORIGIN, "https://m.youtube.com")
                    .header(reqwest::header::REFERER, "https://m.youtube.com/");
            }
            if let Some(rv) = range_val {
                r = r.header(reqwest::header::RANGE, rv);
            }
            r
        };

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
