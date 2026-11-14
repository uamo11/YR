use futures_util::StreamExt;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use yr_core::PlaybackStatus;

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
                        Ok(reader) => match Decoder::new(BufReader::new(reader)) {
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
                                set_status(&snapshot, PlaybackStatus::Error(e.to_string()));
                            }
                        },
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
        let mut guard = lock.lock().map_err(|_| io::Error::other("poisoned"))?;
        while target_u64 > guard.written && !guard.finished {
            if self.cancel.load(Ordering::Relaxed) {
                break;
            }
            if let Some(err) = &guard.error {
                return Err(io::Error::other(err.clone()));
            }
            guard = cvar
                .wait(guard)
                .map_err(|_| io::Error::other("poisoned"))?;
        }
        self.pos = target_u64.min(guard.written);
        Ok(self.pos)
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
        let client = reqwest::Client::new();
        let resp = match client.get(&url_owned).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                let (lock, cvar) = &*state_bg;
                if let Ok(mut g) = lock.lock() {
                    g.error = Some(format!("HTTP {}", r.status()));
                    g.finished = true;
                }
                cvar.notify_all();
                return;
            }
            Err(e) => {
                let (lock, cvar) = &*state_bg;
                if let Ok(mut g) = lock.lock() {
                    g.error = Some(e.to_string());
                    g.finished = true;
                }
                cvar.notify_all();
                return;
            }
        };

        let total = resp.content_length();
        {
            let (lock, cvar) = &*state_bg;
            if let Ok(mut g) = lock.lock() {
                g.total = total;
            }
            cvar.notify_all();
        }

        let mut stream = resp.bytes_stream();
        while let Some(item) = stream.next().await {
            if cancel_bg.load(Ordering::Relaxed) {
                break;
            }
            match item {
                Ok(bytes) => {
                    if let Err(e) = writer.write_all(&bytes) {
                        let (lock, cvar) = &*state_bg;
                        if let Ok(mut g) = lock.lock() {
                            g.error = Some(e.to_string());
                            g.finished = true;
                        }
                        cvar.notify_all();
                        return;
                    }
                    let (lock, cvar) = &*state_bg;
                    if let Ok(mut g) = lock.lock() {
                        g.written += bytes.len() as u64;
                    }
                    cvar.notify_all();
                }
                Err(e) => {
                    let (lock, cvar) = &*state_bg;
                    if let Ok(mut g) = lock.lock() {
                        g.error = Some(e.to_string());
                        g.finished = true;
                    }
                    cvar.notify_all();
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
    }

    Ok(ProgressiveFileReader {
        file: reader_file,
        pos: 0,
        state,
        path,
        cancel,
    })
}
