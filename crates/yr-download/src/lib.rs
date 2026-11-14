use futures_util::StreamExt;
use reqwest::header::RANGE;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, Mutex, Semaphore};
use yr_core::{AudioQuality, Track};
use yr_provider::MusicProvider;

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

/// Concurrent download manager with Range resume, retries, and offline metadata index.
pub struct DownloadManager {
    download_dir: PathBuf,
    http: reqwest::Client,
    provider: Arc<dyn MusicProvider>,
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
            .connect_timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();

        let mgr = Self {
            download_dir,
            http,
            provider,
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            controls: Arc::new(Mutex::new(HashMap::new())),
        };
        mgr.scan_existing_sync();
        mgr
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

    pub async fn find_local_path(&self, track_id: &str) -> Option<PathBuf> {
        let guard = self.tasks.lock().await;
        guard
            .get(track_id)
            .filter(|t| t.status == DownloadStatus::Completed)
            .and_then(|t| t.output_path.clone())
            .filter(|p| p.exists())
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
        let stream = self
            .provider
            .resolve_audio_stream(&track.id, quality)
            .await
            .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?;

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

        let mut req = self.http.get(&stream.url);
        if existing_len > 0 {
            req = req.header(RANGE, format!("bytes={existing_len}-"));
        }

        let resp = req
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

        let mut file = if is_partial {
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
            if elapsed >= Duration::from_millis(300) {
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

        fs::rename(&part_path, &final_path)
            .await
            .map_err(|e| DownloadInternalError::Retryable(e.to_string()))?;

        // Write sidecar metadata for instant offline indexing.
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
            let _ = fs::write(sidecar, json).await;
        }

        Ok(final_path)
    }
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
}
