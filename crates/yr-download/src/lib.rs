use futures_util::StreamExt;
use reqwest::header::{CONTENT_RANGE, RANGE};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::{watch, Mutex, Semaphore};
use yr_core::{AudioQuality, Track};
use yr_provider::MusicProvider;

const PROBE_CHUNK_BYTES: u64 = 256 * 1024;
const PARALLEL_CHUNK_BYTES: u64 = 1024 * 1024;
const PARALLEL_CONNECTIONS: usize = 4;

fn stream_client_ua(url: &str) -> Option<&'static str> {
    if url.contains("c=MWEB") || url.contains("c=WEB") {
        Some("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36")
    } else if url.contains("c=VISIONOS") {
        Some("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_6) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15")
    } else if url.contains("c=IOS") {
        Some("com.google.ios.youtube/20.03.02 (iPhone16,2; U; CPU iOS 18_2_1 like Mac OS X;)")
    } else if url.contains("c=ANDROID_VR") {
        Some("com.google.android.apps.youtube.vr.oculus/1.60.19 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip")
    } else if url.contains("c=ANDROID") {
        Some("com.google.android.youtube/20.10.38 (Linux; U; Android 14; Mobile) gzip")
    } else {
        Some("Mozilla/5.0 (Linux; Android 14; Mobile) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36")
    }
}

fn build_download_request(
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
        r = r.header(RANGE, rv);
    }
    r
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTask {
    pub track: Track,
    pub quality: AudioQuality,
    pub status: DownloadStatus,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: u64,
    pub output_path: Option<PathBuf>,
}

impl DownloadTask {
    pub fn progress(&self) -> f32 {
        match (self.downloaded_bytes, self.total_bytes) {
            (d, Some(t)) if t > 0 => (d as f32 / t as f32).clamp(0.0, 1.0),
            _ if self.status == DownloadStatus::Completed => 1.0,
            _ => 0.0,
        }
    }

    pub fn eta_secs(&self) -> Option<u64> {
        let total = self.total_bytes?;
        if self.speed_bps == 0 || self.downloaded_bytes >= total {
            return None;
        }
        Some((total - self.downloaded_bytes) / self.speed_bps)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SidecarMeta {
    track: Track,
    audio_filename: String,
}

struct TaskControl {
    cancel_tx: watch::Sender<bool>,
    pause_tx: watch::Sender<bool>,
}

/// Optional blocking URL resolver fallback (e.g., Android WebView extractor).
pub type FallbackUrlResolver = Arc<dyn Fn(String) -> Option<String> + Send + Sync>;

/// Concurrent download manager with Range resume, retries, and offline metadata index.
pub struct DownloadManager {
    download_dir: PathBuf,
    http: reqwest::Client,
    provider: Arc<dyn MusicProvider>,
    fallback_resolver: Option<FallbackUrlResolver>,
    semaphore: Arc<Semaphore>,
    tasks: Arc<Mutex<HashMap<String, DownloadTask>>>,
    controls: Arc<Mutex<HashMap<String, TaskControl>>>,
}

impl DownloadManager {
    pub fn new(
        download_dir: PathBuf,
        provider: Arc<dyn MusicProvider>,
        max_concurrent: usize,
    ) -> Self {
        let _ = std::fs::create_dir_all(&download_dir);
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(12))
            .timeout(Duration::from_secs(60))
            .tcp_nodelay(true)
            .pool_max_idle_per_host(12)
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();

        let mgr = Self {
            download_dir,
            http,
            provider,
            fallback_resolver: None,
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            controls: Arc::new(Mutex::new(HashMap::new())),
        };
        mgr.scan_existing_sync();
        mgr
    }

    pub fn with_fallback_resolver(mut self, resolver: FallbackUrlResolver) -> Self {
        self.fallback_resolver = Some(resolver);
        self
    }

    fn scan_existing_sync(&self) {
        let Ok(entries) = std::fs::read_dir(&self.download_dir) else {
            return;
        };
        let mut map = HashMap::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(raw) = std::fs::read_to_string(&path) {
                    if let Ok(meta) = serde_json::from_str::<SidecarMeta>(&raw) {
                        let audio_path = self.download_dir.join(&meta.audio_filename);
                        if audio_path.exists() {
                            let size = std::fs::metadata(&audio_path).map(|m| m.len()).ok();
                            let mut track = meta.track;
                            track.local_path = Some(audio_path.to_string_lossy().into_owned());
                            let id = track.id.clone();
                            map.insert(
                                id,
                                DownloadTask {
                                    track,
                                    quality: AudioQuality::High,
                                    status: DownloadStatus::Completed,
                                    downloaded_bytes: size.unwrap_or(0),
                                    total_bytes: size,
                                    speed_bps: 0,
                                    output_path: Some(audio_path),
                                },
                            );
                        }
                    }
                }
            }
        }
        if let Ok(mut guard) = self.tasks.try_lock() {
            *guard = map;
        }
    }

    pub async fn list_tasks(&self) -> Vec<DownloadTask> {
        let guard = self.tasks.lock().await;
        let mut items: Vec<_> = guard.values().cloned().collect();
        items.sort_by(|a, b| a.track.title.cmp(&b.track.title));
        items
    }

    pub async fn completed_tracks(&self) -> Vec<Track> {
        let guard = self.tasks.lock().await;
        guard
            .values()
            .filter(|t| t.status == DownloadStatus::Completed)
            .map(|t| t.track.clone())
            .collect()
    }

    pub async fn completed_count(&self) -> usize {
        let guard = self.tasks.lock().await;
        guard
            .values()
            .filter(|t| t.status == DownloadStatus::Completed)
            .count()
    }

    pub async fn find_local_path(&self, track_id: &str) -> Option<PathBuf> {
        let guard = self.tasks.lock().await;
        guard
            .get(track_id)
            .filter(|t| t.status == DownloadStatus::Completed)
            .and_then(|t| t.output_path.clone())
            .filter(|p| p.exists())
    }

    pub async fn total_downloaded_bytes(&self) -> u64 {
        let guard = self.tasks.lock().await;
        guard
            .values()
            .filter(|t| t.status == DownloadStatus::Completed)
            .map(|t| t.downloaded_bytes)
            .sum()
    }

    /// Directly registers an already-cached local audio file (e.g. from stream_cache)
    /// into the permanent downloads folder with sidecar metadata for instant offline availability.
    pub async fn register_cached_track(
        &self,
        mut track: Track,
        source_audio_file: &Path,
    ) -> Result<PathBuf, String> {
        if !source_audio_file.exists() {
            return Err("Source file does not exist".to_string());
        }
        let size = std::fs::metadata(source_audio_file)
            .map(|m| m.len())
            .unwrap_or(0);
        if size < 32_768 {
            return Err("File too small to be valid audio".to_string());
        }

        let base_name = format!(
            "{} - {} [{}]",
            sanitize_filename(&track.artist),
            sanitize_filename(&track.title),
            sanitize_filename(&track.id)
        );
        let ext = source_audio_file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("m4a");
        let final_path = self.download_dir.join(format!("{base_name}.{ext}"));

        // Copy if not already in final location
        if source_audio_file != final_path {
            if let Err(e) = std::fs::copy(source_audio_file, &final_path) {
                return Err(format!("Failed to copy cached file: {e}"));
            }
        }

        track.local_path = Some(final_path.to_string_lossy().into_owned());
        let sidecar = sidecar_path_for(&final_path);
        let audio_filename = final_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let meta = SidecarMeta {
            track: track.clone(),
            audio_filename,
        };
        if let Ok(json) = serde_json::to_string_pretty(&meta) {
            let _ = tokio::fs::write(&sidecar, json).await;
        }

        let mut guard = self.tasks.lock().await;
        guard.insert(
            track.id.clone(),
            DownloadTask {
                track,
                quality: AudioQuality::High,
                status: DownloadStatus::Completed,
                downloaded_bytes: size,
                total_bytes: Some(size),
                speed_bps: 0,
                output_path: Some(final_path.clone()),
            },
        );

        Ok(final_path)
    }

    pub async fn enqueue(self: &Arc<Self>, track: Track, quality: AudioQuality) {
        let track_id = track.id.clone();
        {
            let guard = self.tasks.lock().await;
            if let Some(existing) = guard.get(&track_id) {
                if matches!(
                    existing.status,
                    DownloadStatus::Queued | DownloadStatus::Downloading | DownloadStatus::Completed
                ) {
                    return;
                }
            }
        }

        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (pause_tx, pause_rx) = watch::channel(false);

        {
            let mut guard = self.tasks.lock().await;
            guard.insert(
                track_id.clone(),
                DownloadTask {
                    track: track.clone(),
                    quality,
                    status: DownloadStatus::Queued,
                    downloaded_bytes: 0,
                    total_bytes: None,
                    speed_bps: 0,
                    output_path: None,
                },
            );
        }
        {
            let mut ctrl = self.controls.lock().await;
            ctrl.insert(track_id.clone(), TaskControl { cancel_tx, pause_tx });
        }

        let this = Arc::clone(self);
        tokio::spawn(async move {
            this.run_download(track, quality, cancel_rx, pause_rx).await;
        });
    }

    pub async fn pause(&self, track_id: &str) {
        if let Some(ctrl) = self.controls.lock().await.get(track_id) {
            let _ = ctrl.pause_tx.send(true);
        }
        if let Some(task) = self.tasks.lock().await.get_mut(track_id) {
            if matches!(task.status, DownloadStatus::Downloading | DownloadStatus::Queued) {
                task.status = DownloadStatus::Paused;
                task.speed_bps = 0;
            }
        }
    }

    pub async fn resume(self: &Arc<Self>, track_id: &str) {
        let maybe_task = { self.tasks.lock().await.get(track_id).cloned() };
        if let Some(task) = maybe_task {
            if matches!(task.status, DownloadStatus::Paused | DownloadStatus::Failed(_)) {
                self.enqueue(task.track, task.quality).await;
            }
        }
    }

    pub async fn cancel(&self, track_id: &str) {
        if let Some(ctrl) = self.controls.lock().await.get(track_id) {
            let _ = ctrl.cancel_tx.send(true);
        }
        if let Some(task) = self.tasks.lock().await.get_mut(track_id) {
            task.status = DownloadStatus::Cancelled;
            task.speed_bps = 0;
        }
    }

    pub async fn delete_downloaded(&self, track_id: &str) {
        self.cancel(track_id).await;
        let removed = self.tasks.lock().await.remove(track_id);
        if let Some(task) = removed {
            if let Some(path) = task.output_path {
                let _ = fs::remove_file(&path).await;
                let sidecar = sidecar_path_for(&path);
                let _ = fs::remove_file(sidecar).await;
            }
        }
    }

    /// Evicts oldest unprotected completed downloads (by file mtime) until completed count <= `max_songs`.
    pub async fn prune_to_limit(
        &self,
        max_songs: usize,
        protected_ids: &HashSet<String>,
    ) -> usize {
        let mut to_remove_paths: Vec<PathBuf> = Vec::new();
        let removed_count = {
            let mut guard = self.tasks.lock().await;
            let completed_total = guard
                .values()
                .filter(|t| t.status == DownloadStatus::Completed)
                .count();
            if completed_total <= max_songs {
                return 0;
            }
            let excess = completed_total - max_songs;
            let mut candidates: Vec<(SystemTime, String)> = guard
                .iter()
                .filter(|(id, t)| {
                    t.status == DownloadStatus::Completed && !protected_ids.contains(id.as_str())
                })
                .map(|(id, t)| {
                    let mtime = t
                        .output_path
                        .as_ref()
                        .and_then(|p| std::fs::metadata(p).ok())
                        .and_then(|m| m.modified().ok())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    (mtime, id.clone())
                })
                .collect();
            candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

            let mut removed = 0usize;
            for (_, id) in candidates.into_iter().take(excess) {
                if let Some(task) = guard.remove(&id) {
                    removed += 1;
                    if let Some(path) = task.output_path {
                        to_remove_paths.push(path);
                    }
                }
            }
            removed
        };

        for path in to_remove_paths {
            let _ = fs::remove_file(&path).await;
            let _ = fs::remove_file(sidecar_path_for(&path)).await;
        }
        removed_count
    }

    async fn run_download(
        &self,
        mut track: Track,
        quality: AudioQuality,
        cancel_rx: watch::Receiver<bool>,
        pause_rx: watch::Receiver<bool>,
    ) {
        let Ok(_permit) = self.semaphore.acquire().await else {
            return;
        };

        if *cancel_rx.borrow() || *pause_rx.borrow() {
            return;
        }

        {
            let mut guard = self.tasks.lock().await;
            if let Some(t) = guard.get_mut(&track.id) {
                t.status = DownloadStatus::Downloading;
            }
        }

        let mut last_err = String::new();
        for attempt in 0..3u32 {
            if *cancel_rx.borrow() || *pause_rx.borrow() {
                return;
            }
            if attempt > 0 {
                let backoff = Duration::from_secs(1 << attempt);
                tokio::time::sleep(backoff).await;
            }

            match self
                .download_once(&mut track, quality, &cancel_rx, &pause_rx)
                .await
            {
                Ok(final_path) => {
                    track.local_path = Some(final_path.to_string_lossy().into_owned());
                    let mut guard = self.tasks.lock().await;
                    if let Some(t) = guard.get_mut(&track.id) {
                        t.track = track.clone();
                        t.status = DownloadStatus::Completed;
                        t.speed_bps = 0;
                        t.output_path = Some(final_path);
                    }
                    return;
                }
                Err(DownloadInternalError::Paused) => return,
                Err(DownloadInternalError::Cancelled) => return,
                Err(DownloadInternalError::Retryable(msg)) => {
                    self.provider.invalidate_track_streams(&track.id);
                    last_err = msg;
                }
            }
        }

        let mut guard = self.tasks.lock().await;
        if let Some(t) = guard.get_mut(&track.id) {
            t.status = DownloadStatus::Failed(last_err);
            t.speed_bps = 0;
        }
    }

    async fn download_once(
        &self,
        track: &mut Track,
        quality: AudioQuality,
        cancel_rx: &watch::Receiver<bool>,
        pause_rx: &watch::Receiver<bool>,
    ) -> Result<PathBuf, DownloadInternalError> {
        let stream = match self.provider.resolve_audio_stream(&track.id, quality).await {
            Ok(s) => s,
            Err(e) => {
                let err_msg = e.to_string();
                let mut fallback_stream = None;
                if let Some(resolver) = self.fallback_resolver.as_ref().map(Arc::clone) {
                    let tid = track.id.clone();
                    if let Ok(Some(url)) = tokio::task::spawn_blocking(move || resolver(tid)).await {
                        fallback_stream = Some(yr_core::StreamInfo {
                            track_id: track.id.clone(),
                            url,
                            mime_type: "audio/mp4".to_string(),
                            extension: "m4a".to_string(),
                            bitrate_bps: 128_000,
                            content_length: None,
                        });
                    }
                }
                fallback_stream.ok_or(DownloadInternalError::Retryable(err_msg))?
            }
        };

        let base_name = format!(
            "{} - {} [{}]",
            sanitize_filename(&track.artist),
            sanitize_filename(&track.title),
            sanitize_filename(&track.id)
        );
        let final_path = self
            .download_dir
            .join(format!("{base_name}.{}", stream.extension));
        let part_path = self
            .download_dir
            .join(format!("{base_name}.{}.part", stream.extension));

        let existing_len = fs::metadata(&part_path).await.map(|m| m.len()).unwrap_or(0);

        if existing_len == 0 {
            if let Some(done_path) = self
                .try_download_parallel_ranges(
                    track,
                    &stream.url,
                    &part_path,
                    &final_path,
                    cancel_rx,
                    pause_rx,
                )
                .await?
            {
                return Ok(done_path);
            }
        }

        let range_hdr = if existing_len > 0 {
            Some(format!("bytes={existing_len}-"))
        } else {
            None
        };
        let resp = build_download_request(&self.http, &stream.url, range_hdr)
            .send()
            .await
            .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?;

        if !resp.status().is_success() && resp.status().as_u16() != 206 {
            return Err(DownloadInternalError::Retryable(format!(
                "HTTP {}",
                resp.status()
            )));
        }

        let is_partial = resp.status().as_u16() == 206;
        let mut downloaded = if is_partial { existing_len } else { 0 };
        let total_bytes = resp
            .content_length()
            .map(|len| if is_partial { existing_len + len } else { len })
            .or(stream.content_length);

        {
            let mut guard = self.tasks.lock().await;
            if let Some(t) = guard.get_mut(&track.id) {
                t.downloaded_bytes = downloaded;
                t.total_bytes = total_bytes;
            }
        }

        let raw_file = if is_partial {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&part_path)
                .await
                .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?
        } else {
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&part_path)
                .await
                .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?
        };
        let mut file = BufWriter::with_capacity(256 * 1024, raw_file);

        let mut byte_stream = resp.bytes_stream();
        let mut last_tick = Instant::now();
        let mut bytes_since_tick = 0u64;

        while let Some(chunk) = byte_stream.next().await {
            if *cancel_rx.borrow() {
                let _ = fs::remove_file(&part_path).await;
                return Err(DownloadInternalError::Cancelled);
            }
            if *pause_rx.borrow() {
                let _ = file.flush().await;
                return Err(DownloadInternalError::Paused);
            }

            let data = chunk.map_err(|e| DownloadInternalError::Retryable(e.to_string()))?;
            file.write_all(&data)
                .await
                .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?;

            let len = data.len() as u64;
            downloaded += len;
            bytes_since_tick += len;

            let elapsed = last_tick.elapsed();
            if elapsed >= Duration::from_millis(250) {
                let speed = (bytes_since_tick as f64 / elapsed.as_secs_f64()) as u64;
                last_tick = Instant::now();
                bytes_since_tick = 0;

                let mut guard = self.tasks.lock().await;
                if let Some(t) = guard.get_mut(&track.id) {
                    t.downloaded_bytes = downloaded;
                    t.total_bytes = total_bytes;
                    t.speed_bps = speed;
                }
            }
        }

        file.flush()
            .await
            .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?;
        drop(file);

        self.finalize_downloaded_file(track, &part_path, &final_path).await
    }

    async fn try_download_parallel_ranges(
        &self,
        track: &mut Track,
        url: &str,
        part_path: &Path,
        final_path: &Path,
        cancel_rx: &watch::Receiver<bool>,
        pause_rx: &watch::Receiver<bool>,
    ) -> Result<Option<PathBuf>, DownloadInternalError> {
        let probe_end = PROBE_CHUNK_BYTES - 1;
        let first_resp = build_download_request(
            &self.http,
            url,
            Some(format!("bytes=0-{probe_end}")),
        )
        .send()
        .await
        .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?;

        if first_resp.status().as_u16() != 206 {
            return Ok(None);
        }

        let Some(total_bytes) = first_resp
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range_total)
        else {
            return Ok(None);
        };

        let first_bytes = first_resp
            .bytes()
            .await
            .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?;
        let first_len = first_bytes.len() as u64;
        if first_len == 0 {
            return Ok(None);
        }

        {
            let mut guard = self.tasks.lock().await;
            if let Some(t) = guard.get_mut(&track.id) {
                t.downloaded_bytes = first_len;
                t.total_bytes = Some(total_bytes);
            }
        }

        let part_owned = part_path.to_path_buf();
        let sync_file = tokio::task::spawn_blocking(move || -> std::io::Result<std::fs::File> {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&part_owned)?;
            f.set_len(total_bytes)?;
            f.write_all(&first_bytes)?;
            Ok(f)
        })
        .await
        .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?
        .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?;

        if first_len >= total_bytes {
            drop(sync_file);
            return self
                .finalize_downloaded_file(track, part_path, final_path)
                .await
                .map(Some);
        }

        let shared_file = Arc::new(std::sync::Mutex::new(sync_file));
        let downloaded_atomic = Arc::new(AtomicU64::new(first_len));

        let mut ranges = Vec::new();
        let mut cursor = first_len;
        while cursor < total_bytes {
            let end = (cursor + PARALLEL_CHUNK_BYTES - 1).min(total_bytes - 1);
            ranges.push((cursor, end));
            cursor = end + 1;
        }

        let progress_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let progress_flag = Arc::clone(&progress_done);
        let progress_bytes = Arc::clone(&downloaded_atomic);
        let progress_tasks = Arc::clone(&self.tasks);
        let tid = track.id.clone();
        let ticker = tokio::spawn(async move {
            let mut prev_bytes = first_len;
            let mut prev_at = Instant::now();
            while !progress_flag.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let cur = progress_bytes.load(Ordering::Relaxed);
                let dt = prev_at.elapsed().as_secs_f64().max(0.05);
                let speed = (cur.saturating_sub(prev_bytes) as f64 / dt) as u64;
                prev_bytes = cur;
                prev_at = Instant::now();
                let mut guard = progress_tasks.lock().await;
                if let Some(t) = guard.get_mut(&tid) {
                    t.downloaded_bytes = cur;
                    t.total_bytes = Some(total_bytes);
                    t.speed_bps = speed;
                }
            }
        });

        let mut chunk_stream = futures_util::stream::iter(ranges)
            .map(|(start, end)| {
                let http = self.http.clone();
                let url = url.to_string();
                let file_ref = Arc::clone(&shared_file);
                let bytes_counter = Arc::clone(&downloaded_atomic);
                let cancel_rx = cancel_rx.clone();
                let pause_rx = pause_rx.clone();
                async move {
                    if *cancel_rx.borrow() {
                        return Err(DownloadInternalError::Cancelled);
                    }
                    if *pause_rx.borrow() {
                        return Err(DownloadInternalError::Paused);
                    }

                    let mut last_chunk_err = String::new();
                    let mut body_opt = None;
                    for attempt in 0..2u8 {
                        if attempt > 0 {
                            tokio::time::sleep(Duration::from_millis(200)).await;
                        }
                        match build_download_request(
                            &http,
                            &url,
                            Some(format!("bytes={start}-{end}")),
                        )
                        .send()
                        .await
                        {
                            Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 206 => {
                                let mut buf = Vec::with_capacity((end - start + 1) as usize);
                                let mut stream = resp.bytes_stream();
                                let mut stream_ok = true;
                                let mut attempt_bytes: u64 = 0;
                                while let Some(piece) = stream.next().await {
                                    if *cancel_rx.borrow() {
                                        return Err(DownloadInternalError::Cancelled);
                                    }
                                    if *pause_rx.borrow() {
                                        return Err(DownloadInternalError::Paused);
                                    }
                                    match piece {
                                        Ok(b) => {
                                            let n = b.len() as u64;
                                            attempt_bytes += n;
                                            bytes_counter.fetch_add(n, Ordering::Relaxed);
                                            buf.extend_from_slice(&b);
                                        }
                                        Err(e) => {
                                            last_chunk_err = e.to_string();
                                            stream_ok = false;
                                            break;
                                        }
                                    }
                                }
                                if stream_ok {
                                    body_opt = Some(buf);
                                    break;
                                } else if attempt_bytes > 0 {
                                    bytes_counter.fetch_sub(attempt_bytes, Ordering::Relaxed);
                                }
                            }
                            Ok(resp) => last_chunk_err = format!("HTTP {}", resp.status()),
                            Err(e) => last_chunk_err = e.to_string(),
                        }
                    }

                    let data = body_opt.ok_or(DownloadInternalError::Retryable(last_chunk_err))?;
                    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                        let mut f = file_ref
                            .lock()
                            .map_err(|_| std::io::Error::other("file lock poisoned"))?;
                        f.seek(SeekFrom::Start(start))?;
                        f.write_all(&data)?;
                        Ok(())
                    })
                    .await
                    .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?
                    .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?;

                    Ok::<(), DownloadInternalError>(())
                }
            })
            .buffer_unordered(PARALLEL_CONNECTIONS);

        let mut chunk_err: Option<DownloadInternalError> = None;
        while let Some(res) = chunk_stream.next().await {
            if let Err(e) = res {
                chunk_err = Some(e);
                break;
            }
        }
        drop(chunk_stream);

        progress_done.store(true, Ordering::Relaxed);
        let _ = ticker.await;
        drop(shared_file);

        if let Some(err) = chunk_err {
            let _ = fs::remove_file(part_path).await;
            return Err(err);
        }

        let done = self
            .finalize_downloaded_file(track, part_path, final_path)
            .await?;
        Ok(Some(done))
    }

    async fn finalize_downloaded_file(
        &self,
        track: &mut Track,
        part_path: &Path,
        final_path: &Path,
    ) -> Result<PathBuf, DownloadInternalError> {
        fs::rename(part_path, final_path)
            .await
            .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?;

        // Write sidecar metadata for instant offline indexing.
        track.local_path = Some(final_path.to_string_lossy().into_owned());
        let sidecar = sidecar_path_for(final_path);
        let audio_filename = final_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let meta = SidecarMeta {
            track: track.clone(),
            audio_filename,
        };
        if let Ok(json) = serde_json::to_string_pretty(&meta) {
            let _ = fs::write(sidecar, json).await;
        }

        Ok(final_path.to_path_buf())
    }
}

fn parse_content_range_total(header_val: &str) -> Option<u64> {
    let (_, total_str) = header_val.trim().split_once('/')?;
    if total_str == "*" {
        return None;
    }
    total_str.trim().parse::<u64>().ok().filter(|&n| n > 0)
}

enum DownloadInternalError {
    Paused,
    Cancelled,
    Retryable(String),
}

fn sidecar_path_for(audio_path: &Path) -> PathBuf {
    let mut os = audio_path.as_os_str().to_os_string();
    os.push(".json");
    PathBuf::from(os)
}

pub fn sanitize_filename(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "track".to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_illegal_chars() {
        assert_eq!(sanitize_filename("Artist: A/B*C?"), "Artist_ A_B_C_");
        assert_eq!(sanitize_filename(""), "track");
    }

    #[test]
    fn stream_client_ua_uses_active_ios_20_ua() {
        let ua = stream_client_ua("https://rr2.googlevideo.com/videoplayback?c=IOS").unwrap_or("");
        assert!(ua.contains("20.03.02") && ua.contains("iPhone"));
        assert!(!ua.contains("19.29.1"));
    }

    #[test]
    fn parse_content_range_total_parses_standard_headers() {
        assert_eq!(
            parse_content_range_total("bytes 0-262143/4123456"),
            Some(4_123_456)
        );
        assert_eq!(parse_content_range_total("bytes 0-10/*"), None);
        assert_eq!(parse_content_range_total("invalid"), None);
    }

    #[tokio::test]
    async fn register_cached_track_persists_across_restart() {
        let base = std::env::temp_dir().join(format!("yr_dl_test_{}", std::process::id()));
        let dl_dir = base.join("downloads");
        let cache_dir = base.join("stream_cache");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::create_dir_all(&cache_dir);

        let fake_audio = cache_dir.join("stream_test123.audio");
        std::fs::write(&fake_audio, vec![0xAB_u8; 40_000]).unwrap();

        let track = Track {
            id: "test123".into(),
            title: "Song Test".into(),
            artist: "Artist Test".into(),
            artist_id: None,
            album: None,
            album_id: None,
            duration_secs: Some(180),
            thumbnail_url: None,
            local_path: None,
        };

        let provider: Arc<dyn MusicProvider> = Arc::new(yr_provider::YtMusicProvider::new());
        let mgr = DownloadManager::new(dl_dir.clone(), Arc::clone(&provider), 2);
        let registered = mgr.register_cached_track(track, &fake_audio).await.unwrap();
        assert!(registered.exists());
        assert_eq!(mgr.completed_count().await, 1);
        assert_eq!(mgr.find_local_path("test123").await, Some(registered.clone()));

        // Recreate DownloadManager (simulating app restart / power loss recovery).
        let mgr2 = DownloadManager::new(dl_dir, provider, 2);
        assert_eq!(mgr2.completed_count().await, 1);
        assert_eq!(mgr2.find_local_path("test123").await, Some(registered));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn prune_to_limit_evicts_oldest_and_keeps_protected() {
        let base = std::env::temp_dir().join(format!("yr_prune_test_{}", std::process::id()));
        let dl_dir = base.join("downloads");
        let cache_dir = base.join("stream_cache");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::create_dir_all(&cache_dir);

        let provider: Arc<dyn MusicProvider> = Arc::new(yr_provider::YtMusicProvider::new());
        let mgr = DownloadManager::new(dl_dir, provider, 2);

        let make_track = |id: &str| Track {
            id: id.into(),
            title: format!("Title {id}"),
            artist: "Artist".into(),
            artist_id: None,
            album: None,
            album_id: None,
            duration_secs: Some(120),
            thumbnail_url: None,
            local_path: None,
        };

        let mut registered_paths = Vec::new();
        for (idx, id) in ["old_protected", "old_unprotected", "newest"].iter().enumerate() {
            let src = cache_dir.join(format!("stream_{id}.audio"));
            std::fs::write(&src, vec![0xCD_u8; 40_000]).unwrap();
            let dst = mgr
                .register_cached_track(make_track(id), &src)
                .await
                .unwrap();
            let ts = SystemTime::UNIX_EPOCH + Duration::from_secs((idx as u64 + 1) * 1_000);
            let f = std::fs::OpenOptions::new().write(true).open(&dst).unwrap();
            f.set_modified(ts).unwrap();
            registered_paths.push(dst);
        }

        assert_eq!(mgr.completed_count().await, 3);
        let mut protected = HashSet::new();
        protected.insert("old_protected".to_string());

        let pruned = mgr.prune_to_limit(2, &protected).await;
        assert_eq!(pruned, 1);
        assert_eq!(mgr.completed_count().await, 2);
        assert!(mgr.find_local_path("old_protected").await.is_some());
        assert!(mgr.find_local_path("old_unprotected").await.is_none());
        assert!(mgr.find_local_path("newest").await.is_some());
        assert!(!registered_paths[1].exists());
        assert!(!sidecar_path_for(&registered_paths[1]).exists());

        let _ = std::fs::remove_dir_all(&base);
    }
}
