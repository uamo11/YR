use crate::color_extract;
use crate::icons;
use crate::theme::{
    apply_ytm_theme, ACCENT_RED, BG_ELEVATED, BG_PRIMARY, BG_SURFACE, GREEN_OK, TEXT_PRIMARY,
    TEXT_SECONDARY,
};
use eframe::CreationContext;
use egui::{Align, Color32, CornerRadius, Image, Layout, RichText, Sense, Stroke, Vec2};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use yr_audio::{AudioEngine, AudioSnapshot};
use yr_core::{
    format_seconds, image_cache::track_thumb_url, optimize_thumb_url, AlbumDetail, ArtistDetail,
    AudioQuality, ExploreFeed, GoogleDeviceAuth, HomeFeed, HomeShelf, HomeShelfCard,
    HomeShelfLayout, ImageLru, LibraryStore, LyricsData, PlaybackQueue, PlaybackStatus,
    PlaylistDetail, PlaylistSummary, SearchResults, ShelfCardKind, SponsorSegment, Track,
};
use yr_download::{DownloadManager, DownloadStatus, DownloadTask};
use yr_provider::{MusicProvider, YtMusicProvider};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavTab {
    Home,
    Samples,
    Search,
    Library,
    Downloads,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchFilter {
    Songs,
    Albums,
    Artists,
    Playlists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LibraryFilter {
    All,
    Downloads,
    Playlists,
    Songs,
    Albums,
    Artists,
}

#[derive(Clone)]
enum DetailView {
    None,
    Album(AlbumDetail),
    Playlist(PlaylistDetail),
    Artist(ArtistDetail),
    Mood {
        title: String,
        playlists: Vec<PlaylistSummary>,
    },
}

enum AppEvent {
    HomeLoaded(Result<HomeFeed, String>),
    ExploreLoaded(Result<ExploreFeed, String>),
    SearchLoaded(Result<SearchResults, String>),
    AlbumLoaded(Result<AlbumDetail, String>),
    PlaylistLoaded(Result<PlaylistDetail, String>),
    ArtistLoaded(Result<ArtistDetail, String>),
    MoodLoaded {
        title: String,
        result: Result<Vec<PlaylistSummary>, String>,
    },
    RadioLoaded(Result<Vec<Track>, String>),
    RelatedLoaded {
        track_id: String,
        result: Result<Vec<Track>, String>,
    },
    LyricsLoaded {
        track_id: String,
        result: Result<Option<LyricsData>, String>,
    },
    SponsorSegmentsLoaded {
        track_id: String,
        segments: Vec<SponsorSegment>,
    },
    TrackAccentColorLoaded {
        url: String,
        color: Color32,
    },
    StreamResolved {
        track: Track,
        result: Result<String, String>,
    },
    SearchSuggestionsLoaded {
        query: String,
        suggestions: Vec<String>,
    },
    DownloadsRefreshed {
        tasks: Vec<DownloadTask>,
        total_bytes: u64,
    },
    MediaAction(crate::android_media::MediaAction),
    GoogleAuthCode(Result<GoogleDeviceAuth, String>),
    GoogleLoginDone(Result<(), String>),
    GoogleCookieReceived(String),
}

pub struct YrApp {
    runtime: tokio::runtime::Runtime,
    provider: Arc<dyn MusicProvider>,
    downloader: Arc<DownloadManager>,
    audio: AudioEngine,
    store: LibraryStore,
    queue: Arc<std::sync::Mutex<PlaybackQueue>>,
    data_dir: PathBuf,

    event_tx: mpsc::UnboundedSender<AppEvent>,
    event_rx: mpsc::UnboundedReceiver<AppEvent>,

    active_tab: NavTab,
    library_filter: LibraryFilter,
    library_search: String,
    show_new_playlist_dialog: bool,
    search_query: String,
    search_filter: SearchFilter,
    search_results: SearchResults,
    search_loading: bool,
    search_suggestions: Vec<String>,
    last_search_input_at: Option<Instant>,
    suggestions_last_query: String,
    explore_feed: ExploreFeed,
    explore_loading: bool,
    explore_loaded_once: bool,

    home_feed: HomeFeed,
    home_loading: bool,
    active_mood_title: Option<String>,
    detail_view: DetailView,
    detail_loading: bool,

    downloads: Vec<DownloadTask>,
    total_downloaded_bytes: u64,
    last_download_poll: Instant,

    now_playing_open: bool,
    cover_drag_accum: Vec2,
    show_queue: bool,
    show_lyrics: bool,
    show_related: bool,
    lyrics_track_id: Option<String>,
    lyrics_data: Option<LyricsData>,
    lyrics_loading: bool,
    lyrics_last_scrolled_idx: Option<usize>,
    related_track_id: Option<String>,
    related_tracks: Vec<Track>,
    related_loading: bool,
    sponsor_track_id: Option<String>,
    sponsor_segments: Vec<SponsorSegment>,
    sponsor_skipped_idx: Option<usize>,

    show_settings: bool,
    show_google_login: bool,
    dismiss_google_banner: bool,
    google_device_auth: Option<GoogleDeviceAuth>,
    google_login_loading: bool,
    google_cookie_input: String,
    google_auth_error: Option<String>,
    last_google_poll: Instant,
    new_playlist_name: String,

    sleep_timer_until: Arc<std::sync::Mutex<Option<Instant>>>,
    sleep_after_track: Arc<std::sync::atomic::AtomicBool>,
    show_sleep_timer_sheet: bool,
    stream_cache_bytes: u64,
    playlist_picker_track: Option<Track>,
    action_sheet_track: Option<Track>,
    action_sheet_context: Option<(Vec<Track>, usize)>,

    cover_colors: HashMap<String, Color32>,
    pending_color_urls: HashSet<String>,

    last_finished_counter: Arc<std::sync::atomic::AtomicU64>,
    last_media_sync: Option<(String, bool, u64, Instant)>,
    status_banner: Option<(String, Instant)>,
    seeking_secs: Option<f64>,
    home_v_drag_id: Option<egui::Id>,

    sample_tracks: Vec<Track>,
    sample_idx: usize,
    sample_drag_y: f32,
    sample_prewarmed: HashSet<String>,
}

async fn play_track_bg(
    track: Track,
    quality: AudioQuality,
    dl: Arc<DownloadManager>,
    provider: Arc<dyn MusicProvider>,
    audio: AudioEngine,
    tx: mpsc::UnboundedSender<AppEvent>,
    ctx: egui::Context,
) {
    crate::android_media::update_playback(
        &track.title,
        &track.artist,
        true,
        0,
        track.duration_secs.unwrap_or(0) * 1000,
    );
    let art_bg = track_thumb_url(track.thumbnail_url.as_deref(), &track.id);
    crate::android_media::set_artwork_url(art_bg.as_deref().unwrap_or(""));

    if let Some(local) = &track.local_path {
        let p = PathBuf::from(local);
        if p.exists() {
            audio.play_file(p, track.duration_secs);
            ctx.request_repaint();
            return;
        }
    }

    if let Some(local_path) = dl.find_local_path(&track.id).await {
        audio.play_file(local_path, track.duration_secs);
        ctx.request_repaint();
        return;
    }

    let res = match provider.resolve_audio_stream(&track.id, quality).await {
        Ok(s) => {
            #[cfg(target_os = "android")]
            if !s.url.contains("pot=")
                && !s.url.contains("c=VISIONOS")
                && s.content_length.unwrap_or(0) > 1_048_576
            {
                let vid = track.id.clone();
                if let Some(wv) =
                    tokio::task::spawn_blocking(move || crate::android_media::resolve_audio_url(&vid))
                        .await
                        .ok()
                        .flatten()
                {
                    Ok(wv)
                } else {
                    Ok(s.url)
                }
            } else {
                Ok(s.url)
            }
            #[cfg(not(target_os = "android"))]
            {
                Ok(s.url)
            }
        }
        Err(rust_err) => {
            let vid = track.id.clone();
            let webview_url =
                tokio::task::spawn_blocking(move || crate::android_media::resolve_audio_url(&vid))
                    .await
                    .ok()
                    .flatten();
            webview_url.ok_or_else(|| rust_err.to_string())
        }
    };
    match res {
        Ok(url) => {
            audio.play_url(url, track.id, track.duration_secs);
        }
        Err(e) => {
            let _ = tx.send(AppEvent::StreamResolved {
                track,
                result: Err(e),
            });
        }
    }
    ctx.request_repaint();
}

impl YrApp {
    pub fn new(cc: &CreationContext<'_>, data_dir: PathBuf) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        apply_ytm_theme(&cc.egui_ctx);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");

        let store = LibraryStore::load_or_default(&data_dir);
        let provider: Arc<dyn MusicProvider> =
            Arc::new(YtMusicProvider::with_cache_dir(data_dir.join("rustypipe_cache")));
        let download_dir = data_dir.join("downloads");
        let cache_dir = data_dir.join("stream_cache");

        let downloader = Arc::new(DownloadManager::new(
            download_dir,
            Arc::clone(&provider),
            store.data.config.max_concurrent_downloads,
        ));
        let audio = AudioEngine::new(
            cache_dir,
            runtime.handle().clone(),
            store.data.config.volume,
        );

        let stream_cache_bytes = dir_size_bytes(&data_dir.join("stream_cache"));
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let mut app = Self {
            runtime,
            provider,
            downloader,
            audio,
            store,
            queue: Arc::new(std::sync::Mutex::new(PlaybackQueue::new())),
            data_dir,
            event_tx,
            event_rx,
            active_tab: NavTab::Home,
            library_filter: LibraryFilter::All,
            library_search: String::new(),
            show_new_playlist_dialog: false,
            search_query: String::new(),
            search_filter: SearchFilter::Songs,
            search_results: SearchResults::default(),
            search_loading: false,
            search_suggestions: Vec::new(),
            last_search_input_at: None,
            suggestions_last_query: String::new(),
            explore_feed: ExploreFeed::default(),
            explore_loading: false,
            explore_loaded_once: false,
            home_feed: HomeFeed::default(),
            home_loading: false,
            active_mood_title: None,
            detail_view: DetailView::None,
            detail_loading: false,
            downloads: Vec::new(),
            total_downloaded_bytes: 0,
            last_download_poll: Instant::now() - Duration::from_secs(10),
            now_playing_open: false,
            cover_drag_accum: Vec2::ZERO,
            show_queue: false,
            show_lyrics: false,
            show_related: false,
            lyrics_track_id: None,
            lyrics_data: None,
            lyrics_loading: false,
            lyrics_last_scrolled_idx: None,
            related_track_id: None,
            related_tracks: Vec::new(),
            related_loading: false,
            sponsor_track_id: None,
            sponsor_segments: Vec::new(),
            sponsor_skipped_idx: None,
            show_settings: false,
            show_google_login: false,
            dismiss_google_banner: false,
            google_device_auth: None,
            google_login_loading: false,
            google_cookie_input: String::new(),
            google_auth_error: None,
            last_google_poll: Instant::now(),
            new_playlist_name: String::new(),
            sleep_timer_until: Arc::new(std::sync::Mutex::new(None)),
            sleep_after_track: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            show_sleep_timer_sheet: false,
            stream_cache_bytes,
            playlist_picker_track: None,
            action_sheet_track: None,
            action_sheet_context: None,
            cover_colors: HashMap::new(),
            pending_color_urls: HashSet::new(),
            last_finished_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            last_media_sync: None,
            status_banner: None,
            seeking_secs: None,
            home_v_drag_id: None,
            sample_tracks: Vec::new(),
            sample_idx: 0,
            sample_drag_y: 0.0,
            sample_prewarmed: HashSet::new(),
        };

        let (media_tx, mut media_rx) = mpsc::unbounded_channel();
        crate::android_media::set_action_sender(media_tx);
        crate::android_media::set_egui_ctx(cc.egui_ctx.clone());
        let (cookie_tx, mut cookie_rx) = mpsc::unbounded_channel();
        crate::android_media::set_cookie_sender(cookie_tx);
        let ev_tx_cookie = app.event_tx.clone();
        let ctx_cookie = cc.egui_ctx.clone();
        app.runtime.spawn(async move {
            while let Some(cookie) = cookie_rx.recv().await {
                let _ = ev_tx_cookie.send(AppEvent::GoogleCookieReceived(cookie));
                ctx_cookie.request_repaint();
            }
        });
        let audio_bg = app.audio.clone();
        let queue_bg = Arc::clone(&app.queue);
        let dl_bg = Arc::clone(&app.downloader);
        let provider_bg = Arc::clone(&app.provider);
        let quality_bg = app.store.data.config.stream_quality;
        let ev_tx_bg = app.event_tx.clone();
        let ctx_bg = cc.egui_ctx.clone();
        app.runtime.spawn(async move {
            use crate::android_media::MediaAction;
            while let Some(act) = media_rx.recv().await {
                match act {
                    MediaAction::Play | MediaAction::Pause | MediaAction::Toggle => {
                        let playing_now = match act {
                            MediaAction::Play => {
                                audio_bg.resume();
                                true
                            }
                            MediaAction::Pause => {
                                audio_bg.pause();
                                false
                            }
                            MediaAction::Toggle => {
                                let was_playing =
                                    audio_bg.snapshot().status == PlaybackStatus::Playing;
                                if was_playing {
                                    audio_bg.pause();
                                } else {
                                    audio_bg.resume();
                                }
                                !was_playing
                            }
                            _ => false,
                        };
                        if let Some(t) =
                            queue_bg.lock().ok().and_then(|q| q.current_track().cloned())
                        {
                            let snap = audio_bg.snapshot();
                            let dur_ms = snap
                                .duration_secs
                                .map(|d| (d * 1000.0).max(0.0) as u64)
                                .or_else(|| t.duration_secs.map(|d| d * 1000))
                                .unwrap_or(0);
                            crate::android_media::update_playback(
                                &t.title,
                                &t.artist,
                                playing_now,
                                (snap.position_secs * 1000.0).max(0.0) as u64,
                                dur_ms,
                            );
                        }
                    }
                    MediaAction::SeekMs(ms) => {
                        audio_bg.seek(Duration::from_millis(ms));
                        if let Some(t) =
                            queue_bg.lock().ok().and_then(|q| q.current_track().cloned())
                        {
                            let snap = audio_bg.snapshot();
                            let dur_ms = snap
                                .duration_secs
                                .map(|d| (d * 1000.0).max(0.0) as u64)
                                .or_else(|| t.duration_secs.map(|d| d * 1000))
                                .unwrap_or(0);
                            crate::android_media::update_playback(
                                &t.title,
                                &t.artist,
                                snap.status == PlaybackStatus::Playing,
                                ms,
                                dur_ms,
                            );
                        }
                    }
                    MediaAction::Stop => audio_bg.stop(),
                    MediaAction::Next => {
                        let next = queue_bg.lock().ok().and_then(|mut q| q.next_track(true).cloned());
                        if let Some(track) = next {
                            play_track_bg(
                                track,
                                quality_bg,
                                Arc::clone(&dl_bg),
                                Arc::clone(&provider_bg),
                                audio_bg.clone(),
                                ev_tx_bg.clone(),
                                ctx_bg.clone(),
                            )
                            .await;
                        }
                    }
                    MediaAction::Prev => {
                        let prev = queue_bg.lock().ok().and_then(|mut q| q.prev_track().cloned());
                        if let Some(track) = prev {
                            play_track_bg(
                                track,
                                quality_bg,
                                Arc::clone(&dl_bg),
                                Arc::clone(&provider_bg),
                                audio_bg.clone(),
                                ev_tx_bg.clone(),
                                ctx_bg.clone(),
                            )
                            .await;
                        }
                    }
                }
                let _ = ev_tx_bg.send(AppEvent::MediaAction(act));
                ctx_bg.request_repaint();
            }
        });

        let audio_tick = app.audio.clone();
        let queue_tick = Arc::clone(&app.queue);
        let dl_tick = Arc::clone(&app.downloader);
        let provider_tick = Arc::clone(&app.provider);
        let ev_tx_tick = app.event_tx.clone();
        let ctx_tick = cc.egui_ctx.clone();
        let fin_tick = Arc::clone(&app.last_finished_counter);
        let sleep_tick = Arc::clone(&app.sleep_after_track);
        let timer_tick = Arc::clone(&app.sleep_timer_until);
        app.runtime.spawn(async move {
            use std::sync::atomic::Ordering;
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                let timer_expired = timer_tick
                    .lock()
                    .ok()
                    .and_then(|mut g| {
                        if let Some(until) = *g {
                            if Instant::now() >= until {
                                *g = None;
                                return Some(true);
                            }
                        }
                        None
                    })
                    .unwrap_or(false);
                if timer_expired {
                    audio_tick.pause();
                    if let Some(t) = queue_tick.lock().ok().and_then(|q| q.current_track().cloned()) {
                        let snap = audio_tick.snapshot();
                        crate::android_media::update_playback(
                            &t.title,
                            &t.artist,
                            false,
                            (snap.position_secs * 1000.0) as u64,
                            t.duration_secs.unwrap_or(0) * 1000,
                        );
                    }
                    continue;
                }
                let snap = audio_tick.snapshot();
                if snap.finished_counter > fin_tick.load(Ordering::SeqCst) {
                    fin_tick.store(snap.finished_counter, Ordering::SeqCst);
                    if sleep_tick.swap(false, Ordering::SeqCst) {
                        audio_tick.pause();
                        if let Some(t) = queue_tick.lock().ok().and_then(|q| q.current_track().cloned()) {
                            crate::android_media::update_playback(
                                &t.title,
                                &t.artist,
                                false,
                                0,
                                t.duration_secs.unwrap_or(0) * 1000,
                            );
                        }
                        continue;
                    }
                    let next = queue_tick.lock().ok().and_then(|mut q| q.next_track(false).cloned());
                    if let Some(track) = next {
                        let dl_next = Arc::clone(&dl_tick);
                        let prov_next = Arc::clone(&provider_tick);
                        let audio_next = audio_tick.clone();
                        let ev_next = ev_tx_tick.clone();
                        let ctx_next = ctx_tick.clone();
                        tokio::spawn(async move {
                            play_track_bg(
                                track,
                                quality_bg,
                                dl_next,
                                prov_next,
                                audio_next,
                                ev_next,
                                ctx_next,
                            )
                            .await;
                        });
                        continue;
                    }
                }
                if snap.status == PlaybackStatus::Playing {
                    if let Some(t) = queue_tick.lock().ok().and_then(|q| q.current_track().cloned()) {
                        let pos_ms = (snap.position_secs * 1000.0).max(0.0) as u64;
                        let dur_ms = snap
                            .duration_secs
                            .or(t.duration_secs.map(|d| d as f64))
                            .map(|d| (d * 1000.0).max(0.0) as u64)
                            .unwrap_or(0);
                        crate::android_media::update_playback(
                            &t.title,
                            &t.artist,
                            true,
                            pos_ms,
                            dur_ms,
                        );
                    }
                }
            }
        });

        app.hydrate_cached_feeds();
        if (app.store.data.config.playback_speed - 1.0).abs() > 0.02 {
            app.audio.set_speed(app.store.data.config.playback_speed);
        }
        if !app.store.data.last_queue.is_empty() {
            let idx = app
                .store
                .data
                .last_queue_index
                .unwrap_or(0)
                .min(app.store.data.last_queue.len().saturating_sub(1));
            app.queue
                .lock()
                .unwrap()
                .set_queue(app.store.data.last_queue.clone(), idx);
        }
        app.refresh_home(&cc.egui_ctx);
        app.refresh_downloads(&cc.egui_ctx);
        app
    }

    /// Populates Home and Explore feeds from disk cache on cold start for instant offline-ready UI.
    fn hydrate_cached_feeds(&mut self) {
        if let Some(mut cached_home) =
            yr_core::models::load_json_cache::<HomeFeed>(&self.data_dir.join("home_cache.json"))
        {
            self.store.personalize_home_feed(&mut cached_home);
            self.home_feed = cached_home;
        }
        if let Some(cached_explore) = yr_core::models::load_json_cache::<ExploreFeed>(
            &self.data_dir.join("explore_cache.json"),
        ) {
            self.explore_feed = cached_explore;
        }
        if !self.home_feed.shelves.is_empty()
            || !self.home_feed.listen_again.is_empty()
            || !self.explore_feed.trending_tracks.is_empty()
        {
            self.rebuild_samples_deck();
        }
    }

    fn handle_sample_vertical_drag(
        &mut self,
        resp: &egui::Response,
        idx: usize,
        total_samples: usize,
        ctx: &egui::Context,
    ) {
        if resp.dragged() {
            self.sample_drag_y += resp.drag_delta().y;
            ctx.request_repaint();
        }
        if resp.drag_stopped() {
            if self.sample_drag_y < -44.0 && idx + 1 < total_samples {
                self.activate_sample_at(idx + 1, ctx);
            } else if self.sample_drag_y > 44.0 && idx > 0 {
                self.activate_sample_at(idx - 1, ctx);
            }
            self.sample_drag_y = 0.0;
        }
    }

    fn set_banner(&mut self, msg: impl Into<String>) {
        self.status_banner = Some((msg.into(), Instant::now()));
    }

    fn refresh_home(&mut self, ctx: &egui::Context) {
        self.active_mood_title = None;
        self.refresh_home_with_params(None, ctx);
    }

    fn refresh_home_with_params(&mut self, mood_params: Option<String>, ctx: &egui::Context) {
        self.home_loading = true;
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider
                .get_home_feed_with_params(mood_params.as_deref())
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::HomeLoaded(res));
            ctx.request_repaint();
        });
    }

    fn trigger_search(&mut self, ctx: &egui::Context) {
        let q = self.search_query.trim().to_string();
        if q.is_empty() {
            return;
        }
        self.store.record_search(&q);
        self.search_suggestions.clear();
        self.suggestions_last_query = q.clone();
        self.search_loading = true;
        self.detail_view = DetailView::None;
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider.search(&q).await.map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::SearchLoaded(res));
            ctx.request_repaint();
        });
    }

    fn maybe_fetch_suggestions(&mut self, ctx: &egui::Context) {
        let q = self.search_query.trim().to_string();
        if q.len() < 2 {
            self.search_suggestions.clear();
            return;
        }
        if q == self.suggestions_last_query {
            return;
        }
        let Some(at) = self.last_search_input_at else {
            return;
        };
        if at.elapsed() < Duration::from_millis(300) {
            ctx.request_repaint_after(Duration::from_millis(120));
            return;
        }
        self.suggestions_last_query = q.clone();
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            if let Ok(suggestions) = provider.search_suggestions(&q).await {
                let _ = tx.send(AppEvent::SearchSuggestionsLoaded {
                    query: q,
                    suggestions,
                });
                ctx.request_repaint();
            }
        });
    }

    fn open_album(&mut self, album_id: String, ctx: &egui::Context) {
        self.detail_loading = true;
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider.get_album(&album_id).await.map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::AlbumLoaded(res));
            ctx.request_repaint();
        });
    }

    fn open_playlist(&mut self, playlist_id: String, ctx: &egui::Context) {
        self.detail_loading = true;
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider
                .get_playlist(&playlist_id)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::PlaylistLoaded(res));
            ctx.request_repaint();
        });
    }

    fn open_artist(&mut self, artist_id: String, ctx: &egui::Context) {
        self.detail_loading = true;
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider
                .get_artist(&artist_id)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::ArtistLoaded(res));
            ctx.request_repaint();
        });
    }

    fn open_mood(&mut self, genre_id: String, title: String, ctx: &egui::Context) {
        self.detail_loading = true;
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider
                .get_mood_playlists(&genre_id)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::MoodLoaded { title, result: res });
            ctx.request_repaint();
        });
    }

    fn start_radio(&mut self, track_id: String, ctx: &egui::Context) {
        self.set_banner("Generando radio...");
        self.fetch_radio_internal(track_id, ctx);
    }

    fn fetch_radio_internal(&mut self, track_id: String, ctx: &egui::Context) {
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider.get_radio(&track_id).await.map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::RadioLoaded(res));
            ctx.request_repaint();
        });
    }

    fn load_explore(&mut self, ctx: &egui::Context) {
        if self.explore_loading {
            return;
        }
        self.explore_loading = true;
        self.explore_loaded_once = true;
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider
                .get_explore_feed()
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::ExploreLoaded(res));
            ctx.request_repaint();
        });
    }

    fn load_related(&mut self, track_id: String, ctx: &egui::Context) {
        self.related_loading = true;
        self.related_track_id = Some(track_id.clone());
        self.related_tracks.clear();
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider
                .get_related_tracks(&track_id)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::RelatedLoaded {
                track_id,
                result: res,
            });
            ctx.request_repaint();
        });
    }

    fn load_lyrics(&mut self, track: Track, ctx: &egui::Context) {
        self.lyrics_track_id = Some(track.id.clone());
        self.lyrics_data = None;
        if track.local_path.is_some() || track.id.starts_with("local_") {
            self.lyrics_loading = false;
            return;
        }
        self.lyrics_loading = true;
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let tid = track.id.clone();
            let res = provider
                .get_synced_lyrics(&track)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::LyricsLoaded {
                track_id: tid,
                result: res,
            });
            ctx.request_repaint();
        });
    }

    fn ensure_cover_color(&mut self, url: Option<&str>, ctx: &egui::Context) {
        let Some(url) = url.filter(|u| !u.is_empty()) else {
            return;
        };
        if self.cover_colors.contains_key(url) || self.pending_color_urls.contains(url) {
            return;
        }
        let u = url.to_string();
        self.pending_color_urls.insert(u.clone());
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            if let Ok(bytes) = provider.fetch_bytes(&u).await {
                if let Some(color) = color_extract::dominant_ambient_color(&bytes) {
                    let _ = tx.send(AppEvent::TrackAccentColorLoaded { url: u, color });
                    ctx.request_repaint();
                }
            }
        });
    }

    fn refresh_downloads(&mut self, ctx: &egui::Context) {
        self.last_download_poll = Instant::now();
        let dl = Arc::clone(&self.downloader);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let tasks = dl.list_tasks().await;
            let total_bytes = dl.total_downloaded_bytes().await;
            let _ = tx.send(AppEvent::DownloadsRefreshed { tasks, total_bytes });
            ctx.request_repaint();
        });
    }

    fn play_track_in_list(&mut self, tracks: Vec<Track>, index: usize, ctx: &egui::Context) {
        let single_track_id = if tracks.len() == 1 {
            tracks.first().map(|t| t.id.clone())
        } else {
            None
        };
        self.queue.lock().unwrap().set_queue(tracks, index);
        self.now_playing_open = true;
        self.show_settings = false;
        self.play_current_queue_track(ctx);
        if self.store.data.config.autoplay_radio {
            if let Some(track_id) = single_track_id {
                self.fetch_radio_internal(track_id, ctx);
            }
        }
    }

    fn play_current_queue_track(&mut self, ctx: &egui::Context) {
        let (track_opt, next_track_id) = {
            let q = self.queue.lock().unwrap();
            let curr = q.current_track().cloned();
            let ordered = q.ordered_tracks();
            let next_id = curr.as_ref().and_then(|c| {
                ordered
                    .iter()
                    .position(|(_, t)| t.id == c.id)
                    .and_then(|p| ordered.get(p + 1))
                    .map(|(_, t)| t.id.clone())
            });
            (curr, next_id)
        };
        let Some(track) = track_opt else {
            return;
        };
        self.store.record_play(track.clone());
        self.sync_last_queue_snapshot();
        self.seeking_secs = None;
        self.audio.stop();
        self.lyrics_data = None;
        self.lyrics_track_id = None;
        self.lyrics_last_scrolled_idx = None;
        self.related_tracks.clear();
        self.related_track_id = None;
        self.sponsor_track_id = Some(track.id.clone());
        self.sponsor_segments.clear();
        self.sponsor_skipped_idx = None;
        let art_cover = track_thumb_url(track.thumbnail_url.as_deref(), &track.id);
        self.ensure_cover_color(art_cover.as_deref(), ctx);

        // Preload synced lyrics and related recommendations when the player is visible.
        self.load_lyrics(track.clone(), ctx);
        if self.show_related || self.now_playing_open {
            self.load_related(track.id.clone(), ctx);
        }

        crate::android_media::update_playback(
            &track.title,
            &track.artist,
            true,
            0,
            track.duration_secs.unwrap_or(0) * 1000,
        );
        let art_bg = track_thumb_url(track.thumbnail_url.as_deref(), &track.id);
    crate::android_media::set_artwork_url(art_bg.as_deref().unwrap_or(""));

        if self.store.data.config.sponsorblock_enabled {
            let prov_sb = Arc::clone(&self.provider);
            let tx_sb = self.event_tx.clone();
            let sb_tid = track.id.clone();
            let ctx_sb = ctx.clone();
            self.runtime.spawn(async move {
                if let Ok(segs) = prov_sb.get_sponsor_segments(&sb_tid).await {
                    if !segs.is_empty() {
                        let _ = tx_sb.send(AppEvent::SponsorSegmentsLoaded {
                            track_id: sb_tid,
                            segments: segs,
                        });
                        ctx_sb.request_repaint();
                    }
                }
            });
        }

        if let Some(local) = &track.local_path {
            let p = PathBuf::from(local);
            if p.exists() {
                self.audio.play_file(p, track.duration_secs);
                return;
            }
        }

        let dl = Arc::clone(&self.downloader);
        let provider = Arc::clone(&self.provider);
        let quality = self.store.data.config.stream_quality;
        let tx = self.event_tx.clone();
        let ctx_bg = ctx.clone();
        let audio = self.audio.clone();

        self.runtime.spawn(async move {
            if let Some(local_path) = dl.find_local_path(&track.id).await {
                audio.play_file(local_path, track.duration_secs);
                ctx_bg.request_repaint();
                return;
            }
            let res = match provider.resolve_audio_stream(&track.id, quality).await {
                Ok(s) => Ok(s.url),
                Err(rust_err) => {
                    let vid = track.id.clone();
                    let wv = tokio::task::spawn_blocking(move || {
                        crate::android_media::resolve_audio_url(&vid)
                    })
                    .await
                    .ok()
                    .flatten();
                    wv.ok_or_else(|| rust_err.to_string())
                }
            };
            let _ = tx.send(AppEvent::StreamResolved { track, result: res });
            ctx_bg.request_repaint();

            // Pre-resolve next queued track's URL in background so Next starts instantly.
            if let Some(nid) = next_track_id {
                tokio::time::sleep(Duration::from_millis(600)).await;
                let _ = provider.resolve_audio_stream(&nid, quality).await;
            }
        });
    }

    fn enqueue_download(&mut self, track: Track, ctx: &egui::Context) {
        let dl = Arc::clone(&self.downloader);
        let quality = self.store.data.config.download_quality;
        let title = track.title.clone();
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            dl.enqueue(track, quality).await;
            let tasks = dl.list_tasks().await;
            let total_bytes = dl.total_downloaded_bytes().await;
            let _ = tx.send(AppEvent::DownloadsRefreshed { tasks, total_bytes });
            ctx.request_repaint();
        });
        self.set_banner(format!("Descargando: {title}"));
    }

    fn enqueue_download_all(&mut self, tracks: Vec<Track>, ctx: &egui::Context) {
        let dl = Arc::clone(&self.downloader);
        let quality = self.store.data.config.download_quality;
        let count = tracks.len();
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            for track in tracks {
                dl.enqueue(track, quality).await;
            }
            let tasks = dl.list_tasks().await;
            let total_bytes = dl.total_downloaded_bytes().await;
            let _ = tx.send(AppEvent::DownloadsRefreshed { tasks, total_bytes });
            ctx.request_repaint();
        });
        self.set_banner(format!("Descargando {count} canciones..."));
    }

    fn process_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.event_rx.try_recv() {
            match ev {
                AppEvent::HomeLoaded(res) => {
                    self.home_loading = false;
                    match res {
                        Ok(mut feed) => {
                            if self.active_mood_title.is_none()
                                && (!feed.shelves.is_empty() || !feed.listen_again.is_empty())
                            {
                                yr_core::models::save_json_cache(
                                    &self.data_dir.join("home_cache.json"),
                                    &feed,
                                );
                            }
                            self.store.personalize_home_feed(&mut feed);
                            self.home_feed = feed;
                            self.rebuild_samples_deck();
                            if !self.explore_loaded_once {
                                self.load_explore(ctx);
                            }
                        }
                        Err(e) => {
                            if self.home_feed.shelves.is_empty()
                                && self.home_feed.listen_again.is_empty()
                            {
                                self.set_banner(format!("Sin conexión: {e}"));
                            } else {
                                self.set_banner("Sin conexión: mostrando inicio guardado");
                            }
                        }
                    }
                }
                AppEvent::ExploreLoaded(res) => {
                    self.explore_loading = false;
                    if let Ok(feed) = res {
                        if !feed.trending_tracks.is_empty()
                            || !feed.new_albums.is_empty()
                            || !feed.moods.is_empty()
                        {
                            yr_core::models::save_json_cache(
                                &self.data_dir.join("explore_cache.json"),
                                &feed,
                            );
                        }
                        self.explore_feed = feed;
                        self.rebuild_samples_deck();
                    }
                }
                AppEvent::SearchLoaded(res) => {
                    self.search_loading = false;
                    match res {
                        Ok(r) => {
                            let first_thumb = r
                                .songs
                                .first()
                                .and_then(|s| s.thumbnail_url.clone())
                                .or_else(|| r.albums.first().and_then(|a| a.thumbnail_url.clone()))
                                .or_else(|| r.artists.first().and_then(|a| a.thumbnail_url.clone()));
                            let q = self.search_query.trim().to_string();
                            if !q.is_empty() && first_thumb.is_some() {
                                self.store.record_search_with_thumb(&q, first_thumb);
                            }
                            self.search_results = r;
                        }
                        Err(e) => self.set_banner(friendly_error_message("Error al buscar", &e)),
                    }
                }
                AppEvent::AlbumLoaded(res) => {
                    self.detail_loading = false;
                    match res {
                        Ok(a) => self.detail_view = DetailView::Album(a),
                        Err(e) => self.set_banner(friendly_error_message("Error al abrir álbum", &e)),
                    }
                }
                AppEvent::PlaylistLoaded(res) => {
                    self.detail_loading = false;
                    match res {
                        Ok(p) => self.detail_view = DetailView::Playlist(p),
                        Err(e) => self.set_banner(friendly_error_message("Error al abrir playlist", &e)),
                    }
                }
                AppEvent::ArtistLoaded(res) => {
                    self.detail_loading = false;
                    match res {
                        Ok(a) => self.detail_view = DetailView::Artist(a),
                        Err(e) => self.set_banner(friendly_error_message("Error al abrir artista", &e)),
                    }
                }
                AppEvent::MoodLoaded { title, result } => {
                    self.detail_loading = false;
                    match result {
                        Ok(playlists) => {
                            self.detail_view = DetailView::Mood { title, playlists }
                        }
                        Err(e) => self.set_banner(friendly_error_message("Error al abrir género", &e)),
                    }
                }
                AppEvent::RadioLoaded(res) => match res {
                    Ok(tracks) => {
                        let tracks = self.store.rank_recommendations(tracks);
                        if tracks.is_empty() {
                            self.set_banner("No se encontraron canciones similares");
                        } else {
                            {
                                let mut q = self.queue.lock().unwrap();
                                for t in &tracks {
                                    q.enqueue_or_enrich(t.clone());
                                }
                            }
                            let mut existing: HashSet<String> =
                                self.sample_tracks.iter().map(|t| t.id.clone()).collect();
                            for t in tracks {
                                if !t.id.starts_with("local_demo_") && existing.insert(t.id.clone())
                                {
                                    self.sample_tracks.push(t);
                                }
                            }
                            self.set_banner("Radio añadida a la cola");
                        }
                    }
                    Err(e) => self.set_banner(friendly_error_message("Error de radio", &e)),
                },
                AppEvent::RelatedLoaded { track_id, result } => {
                    if self.related_track_id.as_deref() == Some(&track_id) {
                        self.related_loading = false;
                        if let Ok(tracks) = result {
                            self.related_tracks = self.store.rank_recommendations(tracks);
                        }
                    }
                }
                AppEvent::LyricsLoaded { track_id, result } => {
                    if self.lyrics_track_id.as_deref() == Some(&track_id) {
                        self.lyrics_loading = false;
                        self.lyrics_data = match result {
                            Ok(Some(data)) => Some(data),
                            Ok(None) => Some(LyricsData {
                                plain: "Letra no disponible para esta canción.".into(),
                                synced: Vec::new(),
                                source: String::new(),
                            }),
                            Err(e) => Some(LyricsData {
                                plain: friendly_error_message("No se pudo cargar la letra", &e),
                                synced: Vec::new(),
                                source: String::new(),
                            }),
                        };
                    }
                }
                AppEvent::SponsorSegmentsLoaded { track_id, segments } => {
                    if self.sponsor_track_id.as_deref() == Some(&track_id) {
                        self.sponsor_segments = segments;
                        self.sponsor_skipped_idx = None;
                    }
                }
                AppEvent::TrackAccentColorLoaded { url, color } => {
                    self.pending_color_urls.remove(&url);
                    if self.cover_colors.len() >= 32 && !self.cover_colors.contains_key(&url) {
                        if let Some(evict_key) = self.cover_colors.keys().next().cloned() {
                            self.cover_colors.remove(&evict_key);
                        }
                    }
                    self.cover_colors.insert(url, color);
                }
                AppEvent::StreamResolved { track, result } => match result {
                    Ok(url) => {
                        let is_curr = self
                            .queue
                            .lock()
                            .unwrap()
                            .current_track()
                            .map(|t| t.id.as_str())
                            == Some(&track.id);
                        if is_curr {
                            self.audio.play_url(url, track.id, track.duration_secs);
                        }
                    }
                    Err(e) => self.set_banner(friendly_error_message("No se pudo reproducir", &e)),
                },
                AppEvent::SearchSuggestionsLoaded { query, suggestions } => {
                    if self.search_query.trim() == query {
                        self.search_suggestions = suggestions;
                    }
                }
                AppEvent::DownloadsRefreshed { tasks, total_bytes } => {
                    self.downloads = tasks;
                    self.total_downloaded_bytes = total_bytes;
                }
                AppEvent::MediaAction(act) => {
                    use crate::android_media::MediaAction;
                    if matches!(act, MediaAction::Next | MediaAction::Prev) {
                        let maybe_track = self.queue.lock().unwrap().current_track().cloned();
                        if let Some(track) = maybe_track {
                            self.store.record_play(track.clone());
                            self.seeking_secs = None;
                            self.lyrics_data = None;
                            self.lyrics_track_id = None;
                            self.lyrics_last_scrolled_idx = None;
                            self.related_tracks.clear();
                            self.related_track_id = None;
                            self.sponsor_track_id = Some(track.id.clone());
                            self.sponsor_segments.clear();
                            self.sponsor_skipped_idx = None;
                            let art_cover = track_thumb_url(track.thumbnail_url.as_deref(), &track.id);
        self.ensure_cover_color(art_cover.as_deref(), ctx);
                            crate::android_media::set_artwork_url(
                                track.thumbnail_url.as_deref().unwrap_or(""),
                            );
                            self.load_lyrics(track.clone(), ctx);
                            if self.show_related || self.now_playing_open {
                                self.load_related(track.id, ctx);
                            }
                        }
                    }
                }
                AppEvent::GoogleAuthCode(res) => {
                    self.google_login_loading = false;
                    match res {
                        Ok(auth) => {
                            self.google_device_auth = Some(auth);
                            self.google_auth_error = None;
                            self.last_google_poll = Instant::now();
                        }
                        Err(e) => {
                            self.google_auth_error = Some(e);
                        }
                    }
                }
                AppEvent::GoogleLoginDone(res) => {
                    self.google_login_loading = false;
                    match res {
                        Ok(()) => {
                            self.home_feed.is_logged_in = true;
                            self.google_device_auth = None;
                            self.google_auth_error = None;
                            self.show_google_login = false;
                            self.google_cookie_input.clear();
                            self.set_banner("Sesión de Google conectada. Cargando tus recomendaciones...");
                            self.refresh_home(ctx);
                        }
                        Err(e) => {
                            if e.contains("authorization_pending") || e.contains("slow_down") {
                                // Still pending user authorization
                            } else {
                                self.google_auth_error = Some(e);
                            }
                        }
                    }
                }
                AppEvent::GoogleCookieReceived(cookie) => {
                    self.login_with_cookie(cookie, ctx);
                }
            }
        }

        if self.google_device_auth.is_some()
            && !self.google_login_loading
            && self.last_google_poll.elapsed() >= Duration::from_secs(4)
        {
            self.poll_google_device_auth(ctx);
        }

        if self.active_tab == NavTab::Search {
            self.maybe_fetch_suggestions(ctx);
        }

        let active_timer = *self.sleep_timer_until.lock().unwrap();
        if let Some(until) = active_timer {
            if Instant::now() >= until {
                *self.sleep_timer_until.lock().unwrap() = None;
                self.audio.pause();
                self.set_banner("Temporizador de apagado: reproducción pausada");
            } else {
                ctx.request_repaint_after(Duration::from_secs(1));
            }
        }

        advance_and_prune_image_cache(ctx);

        let has_active_dl = self.downloads.iter().any(|d| {
            matches!(
                d.status,
                DownloadStatus::Queued | DownloadStatus::Downloading
            )
        });
        let poll_interval = if has_active_dl || self.active_tab == NavTab::Downloads {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(4)
        };
        if self.last_download_poll.elapsed() >= poll_interval {
            self.refresh_downloads(ctx);
        }

        let snap = self.audio.snapshot();
        if self.store.data.config.sponsorblock_enabled
            && snap.status == PlaybackStatus::Playing
            && !self.sponsor_segments.is_empty()
        {
            let pos = snap.position_secs;
            if let Some((idx, seg)) = self
                .sponsor_segments
                .iter()
                .enumerate()
                .find(|(i, s)| self.sponsor_skipped_idx != Some(*i) && pos >= s.start_secs && pos < s.end_secs - 0.5)
            {
                self.sponsor_skipped_idx = Some(idx);
                let target_end = seg.end_secs;
                let cat = seg.category.clone();
                self.audio.seek(Duration::from_secs_f64(target_end));
                self.set_banner(format!("SponsorBlock: saltado segmento ({cat})"));
            }
        }
        if snap.finished_counter > self.last_finished_counter.load(std::sync::atomic::Ordering::SeqCst) {
            self.last_finished_counter
                .store(snap.finished_counter, std::sync::atomic::Ordering::SeqCst);
            if self
                .sleep_after_track
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                self.audio.pause();
                self.set_banner("Temporizador: detenido al finalizar la canción");
            } else if self.queue.lock().unwrap().next_track(false).is_some() {
                self.play_current_queue_track(ctx);
            }
        }
        if matches!(
            snap.status,
            PlaybackStatus::Playing | PlaybackStatus::Buffering | PlaybackStatus::Loading
        ) {
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        let maybe_current = {
            let mut q = self.queue.lock().unwrap();
            if let Some(dur) = snap.duration_secs.filter(|&d| d > 1.0) {
                if let Some(tid) = q.current_track().map(|t| t.id.clone()) {
                    q.fill_track_duration(&tid, dur.round() as u64);
                }
            }
            q.current_track().cloned()
        };
        if let Some(track) = maybe_current {
            let is_playing = snap.status == PlaybackStatus::Playing;
            let pos_ms = (snap.position_secs * 1000.0).max(0.0) as u64;
            let dur_ms = snap
                .duration_secs
                .or(track.duration_secs.map(|d| d as f64))
                .map(|d| (d * 1000.0).max(0.0) as u64)
                .unwrap_or(0);
            let should_sync = match &self.last_media_sync {
                Some((id, prev_playing, prev_dur_ms, at)) => {
                    id != &track.id
                        || *prev_playing != is_playing
                        || (*prev_dur_ms == 0 && dur_ms > 0)
                        || (is_playing && at.elapsed() >= Duration::from_secs(8))
                }
                None => true,
            };
            if should_sync {
                crate::android_media::update_playback(
                    &track.title,
                    &track.artist,
                    is_playing,
                    pos_ms,
                    dur_ms,
                );
                self.last_media_sync =
                    Some((track.id.clone(), is_playing, dur_ms, Instant::now()));
            }
        }
    }
}

impl eframe::App for YrApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if crate::android_media::take_back_pressed() {
            self.pop_back_navigation();
        }
        self.process_events(ctx);
        let snap = self.audio.snapshot();
        let fullscreen_player = self.now_playing_open && !self.show_settings;

        if !fullscreen_player {
            // Top Header Bar
            egui::TopBottomPanel::top("yr_top_bar")
                .frame(egui::Frame::new().fill(BG_PRIMARY).inner_margin(12.0))
                .show(ctx, |ui| {
                    #[cfg(target_os = "android")]
                    ui.add_space(22.0);

                    ui.horizontal(|ui| {
                        let badge_center = ui.cursor().min + Vec2::new(13.0, 13.0);
                        ui.painter().circle_filled(badge_center, 12.0, ACCENT_RED);
                        icons::draw_icon(
                            ui.painter(),
                            egui::Rect::from_center_size(
                                badge_center + Vec2::new(1.0, 0.0),
                                Vec2::splat(11.0),
                            ),
                            icons::Icon::Play,
                            Color32::WHITE,
                        );
                        ui.add_space(28.0);
                        ui.label(
                            RichText::new("Music")
                                .size(21.0)
                                .strong()
                                .color(TEXT_PRIMARY),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icons::icon_button(
                                ui,
                                icons::Icon::Settings,
                                32.0,
                                18.0,
                                if self.show_settings {
                                    BG_ELEVATED
                                } else {
                                    Color32::TRANSPARENT
                                },
                                if self.show_settings {
                                    ACCENT_RED
                                } else {
                                    TEXT_PRIMARY
                                },
                                16.0,
                            )
                            .clicked()
                            {
                                self.show_settings = !self.show_settings;
                                if self.show_settings {
                                    self.refresh_cache_stats();
                                }
                            }
                            if self.home_feed.is_logged_in {
                                let avatar_url = self
                                    .home_feed
                                    .account
                                    .as_ref()
                                    .and_then(|a| a.avatar_url.as_deref());
                                if let Some(url) = avatar_url {
                                    let thumb_uri = tracked_thumb_uri(url, 192);
                                    let av = ui.add(
                                        Image::new(thumb_uri)
                                            .fit_to_exact_size(Vec2::splat(28.0))
                                            .corner_radius(CornerRadius::same(14))
                                            .sense(Sense::click()),
                                    );
                                    if av.clicked() {
                                        self.show_google_login = true;
                                    }
                                } else {
                                    let initial = self
                                        .home_feed
                                        .account
                                        .as_ref()
                                        .and_then(|a| a.name.chars().next())
                                        .unwrap_or('G')
                                        .to_uppercase()
                                        .to_string();
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(initial)
                                                    .strong()
                                                    .size(13.0)
                                                    .color(Color32::WHITE),
                                            )
                                            .fill(Color32::from_rgb(40, 120, 90))
                                            .corner_radius(CornerRadius::same(14))
                                            .min_size(Vec2::splat(28.0)),
                                        )
                                        .clicked()
                                    {
                                        self.show_google_login = true;
                                    }
                                }
                            } else if icons::icon_pill_button(
                                ui,
                                icons::Icon::Account,
                                "Acceder",
                                BG_ELEVATED,
                                TEXT_PRIMARY,
                            )
                            .clicked()
                            {
                                self.show_google_login = true;
                            }
                        });
                    });
                });

            // Bottom Navigation + Mini-Player
            egui::TopBottomPanel::bottom("yr_bottom_bar")
                .frame(egui::Frame::new().fill(Color32::from_rgb(16, 16, 18)).inner_margin(6.0))
                .show(ctx, |ui| {
                    let in_samples_root = self.active_tab == NavTab::Samples
                        && !self.show_settings
                        && matches!(self.detail_view, DetailView::None);
                    let maybe_track = self.queue.lock().unwrap().current_track().cloned();
                    if !in_samples_root {
                        if let Some(track) = maybe_track {
                            self.render_mini_player(ui, ctx, &track, &snap);
                            ui.add_space(4.0);
                        }
                    }

                    let dl_active = self
                        .downloads
                        .iter()
                        .filter(|d| {
                            matches!(
                                d.status,
                                DownloadStatus::Queued | DownloadStatus::Downloading
                            )
                        })
                        .count();
                    let lib_label = if dl_active > 0 {
                        format!("Biblioteca ({dl_active})")
                    } else {
                        "Biblioteca".to_string()
                    };

                    ui.columns(4, |cols| {
                        self.nav_button(&mut cols[0], NavTab::Home, "Principal");
                        self.nav_button(&mut cols[1], NavTab::Samples, "Samples");
                        self.nav_button(&mut cols[2], NavTab::Search, "Buscar");
                        self.nav_button(&mut cols[3], NavTab::Library, &lib_label);
                    });

                    #[cfg(target_os = "android")]
                    ui.add_space(46.0);
                });
        }

        // Main Content Area
        let samples_full_bleed = self.active_tab == NavTab::Samples
            && !self.show_settings
            && matches!(self.detail_view, DetailView::None);
        let panel_frame = if fullscreen_player || samples_full_bleed {
            egui::Frame::new().fill(Color32::from_rgb(12, 12, 14)).inner_margin(0.0)
        } else {
            egui::Frame::new().fill(BG_PRIMARY).inner_margin(12.0)
        };
        egui::CentralPanel::default()
            .frame(panel_frame)
            .show(ctx, |ui| {
                if self.show_settings {
                    self.render_settings(ui, ctx);
                    return;
                }

                if self.now_playing_open {
                    self.render_full_player(ui, ctx, &snap);
                    return;
                }

                if self.render_detail_if_active(ui, ctx) {
                    return;
                }

                match self.active_tab {
                    NavTab::Home => self.render_home(ui, ctx),
                    NavTab::Samples => self.render_samples(ui, ctx, &snap),
                    NavTab::Search => self.render_search(ui, ctx),
                    NavTab::Library => self.render_library(ui, ctx),
                    NavTab::Downloads => {
                        self.active_tab = NavTab::Library;
                        self.library_filter = LibraryFilter::Downloads;
                        self.render_library(ui, ctx);
                    }
                }
            });

        self.render_action_sheet_modal(ctx);
        self.render_sleep_timer_modal(ctx);
        self.render_playlist_picker_modal(ctx);
        self.render_google_login_modal(ctx);
        self.render_status_toast(ctx);
        crate::android_media::set_ui_at_root(self.is_at_root());
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.sync_last_queue_snapshot();
        let _ = self.store.save();
    }
}

impl YrApp {
    fn render_status_toast(&self, ctx: &egui::Context) {
        let Some((msg, at)) = &self.status_banner else {
            return;
        };
        if at.elapsed() >= Duration::from_secs(3) {
            return;
        }
        let max_w = (ctx.screen_rect().width() - 48.0).clamp(180.0, 420.0);
        egui::Area::new(egui::Id::new("yr_status_toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -140.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(30, 30, 36, 240))
                    .stroke(Stroke::new(0.8_f32, Color32::from_white_alpha(38)))
                    .corner_radius(CornerRadius::same(18))
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.set_max_width(max_w);
                        ui.add(
                            egui::Label::new(
                                RichText::new(msg).size(13.0).color(Color32::WHITE),
                            )
                            .truncate()
                            .selectable(false),
                        );
                    });
            });
    }

    fn sync_last_queue_snapshot(&mut self) {
        let (tracks, idx) = {
            let q = self.queue.lock().unwrap();
            (q.tracks().to_vec(), q.current_index())
        };
        if !tracks.is_empty() {
            self.store.set_last_queue(tracks, idx);
        }
    }

    fn refresh_cache_stats(&mut self) {
        self.stream_cache_bytes = dir_size_bytes(&self.data_dir.join("stream_cache"));
    }

    fn is_at_root(&self) -> bool {
        !self.show_settings
            && !self.now_playing_open
            && matches!(self.detail_view, DetailView::None)
            && self.active_tab == NavTab::Home
            && self.action_sheet_track.is_none()
            && self.playlist_picker_track.is_none()
            && !self.show_sleep_timer_sheet
            && !self.show_google_login
            && !self.show_new_playlist_dialog
    }

    fn pop_back_navigation(&mut self) {
        let has_search_results = !self.search_results.songs.is_empty()
            || !self.search_results.albums.is_empty()
            || !self.search_results.artists.is_empty()
            || !self.search_results.playlists.is_empty();

        if self.show_sleep_timer_sheet {
            self.show_sleep_timer_sheet = false;
        } else if self.action_sheet_track.take().is_some() {
            self.action_sheet_context = None;
        } else if self.playlist_picker_track.take().is_some() {
        } else if self.show_google_login {
            self.show_google_login = false;
        } else if self.show_new_playlist_dialog {
            self.show_new_playlist_dialog = false;
        } else if self.show_settings {
            self.show_settings = false;
        } else if self.now_playing_open {
            if self.show_queue || self.show_lyrics || self.show_related {
                self.show_queue = false;
                self.show_lyrics = false;
                self.show_related = false;
            } else {
                self.now_playing_open = false;
            }
        } else if !matches!(self.detail_view, DetailView::None) {
            self.detail_view = DetailView::None;
        } else if self.active_tab == NavTab::Search
            && (!self.search_query.trim().is_empty() || has_search_results)
        {
            self.search_query.clear();
            self.search_results = SearchResults::default();
            self.search_suggestions.clear();
        } else if self.active_tab != NavTab::Home {
            self.active_tab = NavTab::Home;
        }
    }
    fn start_google_device_auth(&mut self, ctx: &egui::Context) {
        self.google_login_loading = true;
        self.google_auth_error = None;
        let provider = self.provider.clone();
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider
                .start_google_device_login()
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::GoogleAuthCode(res));
            ctx.request_repaint();
        });
    }

    fn poll_google_device_auth(&mut self, ctx: &egui::Context) {
        self.google_login_loading = true;
        self.last_google_poll = Instant::now();
        let provider = self.provider.clone();
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = match provider.poll_google_device_login().await {
                Ok(true) => Ok(()),
                Ok(false) => Err("authorization_pending".to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(AppEvent::GoogleLoginDone(res));
            ctx.request_repaint();
        });
    }

    fn login_with_cookie(&mut self, cookie: String, ctx: &egui::Context) {
        let trimmed = cookie.trim().to_string();
        if trimmed.is_empty() {
            self.google_auth_error = Some("Pega una cookie válida de YouTube Music".to_string());
            return;
        }
        self.google_login_loading = true;
        self.google_auth_error = None;
        let provider = self.provider.clone();
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider
                .login_with_cookie(&trimmed)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::GoogleLoginDone(res));
            ctx.request_repaint();
        });
    }

    fn logout_google(&mut self, ctx: &egui::Context) {
        let provider = self.provider.clone();
        let ctx_bg = ctx.clone();
        self.home_feed.is_logged_in = false;
        self.google_device_auth = None;
        self.google_auth_error = None;
        self.runtime.spawn(async move {
            let _ = provider.logout().await;
            ctx_bg.request_repaint();
        });
        self.set_banner("Sesión de Google cerrada");
        self.refresh_home(ctx);
    }

    fn nav_button(&mut self, ui: &mut egui::Ui, tab: NavTab, label: &str) {
        let selected = self.active_tab == tab && !self.now_playing_open && !self.show_settings;
        let fg = if selected { ACCENT_RED } else { TEXT_SECONDARY };
        let bg = if selected { BG_ELEVATED } else { Color32::TRANSPARENT };
        let icon = match tab {
            NavTab::Home => icons::Icon::Home,
            NavTab::Samples => icons::Icon::Samples,
            NavTab::Search => icons::Icon::Search,
            NavTab::Library => icons::Icon::Library,
            NavTab::Downloads => icons::Icon::DownloadsNav,
        };
        let desired = Vec2::new(ui.available_width().max(56.0), 46.0);
        let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            let fill = if resp.hovered() && !selected {
                Color32::from_rgb(32, 32, 32)
            } else {
                bg
            };
            p.rect_filled(rect, 10.0, fill);
            let icon_rect = egui::Rect::from_center_size(
                egui::pos2(rect.center().x, rect.top() + 14.0),
                Vec2::splat(18.0),
            );
            icons::draw_icon(p, icon_rect, icon, fg);
            p.text(
                egui::pos2(rect.center().x, rect.bottom() - 10.0),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(11.0),
                fg,
            );
        }
        if resp.clicked() {
            self.active_tab = tab;
            self.now_playing_open = false;
            self.show_settings = false;
            self.detail_view = DetailView::None;
        }
    }

    fn play_tracks_shuffled(&mut self, tracks: Vec<Track>, ctx: &egui::Context) {
        if tracks.is_empty() {
            return;
        }
        let len = tracks.len();
        {
            let mut q = self.queue.lock().unwrap();
            q.set_queue(tracks, 0);
            if !q.shuffle {
                q.toggle_shuffle();
            }
            if len > 1 {
                q.next_track(true);
            }
        }
        self.now_playing_open = true;
        self.show_settings = false;
        self.play_current_queue_track(ctx);
    }

    fn render_collection_actions(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        tracks: &[Track],
        show_download: bool,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            if icons::icon_pill_button(
                ui,
                icons::Icon::Play,
                "Reproducir",
                ACCENT_RED,
                Color32::WHITE,
            )
            .clicked()
                && !tracks.is_empty()
            {
                self.play_track_in_list(tracks.to_vec(), 0, ctx);
            }
            if icons::icon_pill_button(
                ui,
                icons::Icon::Shuffle,
                "Aleatorio",
                BG_ELEVATED,
                TEXT_PRIMARY,
            )
            .clicked()
                && !tracks.is_empty()
            {
                self.play_tracks_shuffled(tracks.to_vec(), ctx);
            }
            if show_download
                && icons::icon_pill_button(
                    ui,
                    icons::Icon::Download,
                    "Descargar todo",
                    BG_ELEVATED,
                    TEXT_PRIMARY,
                )
                .clicked()
                && !tracks.is_empty()
            {
                self.enqueue_download_all(tracks.to_vec(), ctx);
            }
        });
    }

    fn render_detail_if_active(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        if self.detail_loading {
            ui.horizontal(|ui| {
                if icons::icon_pill_button(
                    ui,
                    icons::Icon::ArrowBack,
                    "Volver",
                    BG_ELEVATED,
                    TEXT_PRIMARY,
                )
                .clicked() {
                    self.detail_loading = false;
                    self.detail_view = DetailView::None;
                }
                ui.spinner();
                ui.label("Cargando...");
            });
            return true;
        }

        let view = self.detail_view.clone();
        match view {
            DetailView::None => false,
            DetailView::Album(album) => {
                ui.horizontal(|ui| {
                    if icons::icon_pill_button(
                    ui,
                    icons::Icon::ArrowBack,
                    "Volver",
                    BG_ELEVATED,
                    TEXT_PRIMARY,
                )
                .clicked() {
                        self.detail_view = DetailView::None;
                    }
                    ui.heading(&album.summary.title);
                });
                ui.horizontal(|ui| {
                    render_thumb(ui, album.summary.thumbnail_url.as_deref(), 64.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&album.summary.artist).color(TEXT_SECONDARY));
                        self.render_collection_actions(ui, ctx, &album.tracks, true);
                    });
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (idx, track) in album.tracks.iter().enumerate() {
                        self.render_track_row(ui, ctx, track, &album.tracks, idx, false);
                    }
                });
                true
            }
            DetailView::Playlist(pl) => {
                ui.horizontal(|ui| {
                    if icons::icon_pill_button(
                    ui,
                    icons::Icon::ArrowBack,
                    "Volver",
                    BG_ELEVATED,
                    TEXT_PRIMARY,
                )
                .clicked() {
                        self.detail_view = DetailView::None;
                    }
                    ui.heading(&pl.summary.title);
                });
                self.render_collection_actions(ui, ctx, &pl.tracks, true);
                ui.add_space(6.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (idx, track) in pl.tracks.iter().enumerate() {
                        self.render_track_row(ui, ctx, track, &pl.tracks, idx, false);
                    }
                });
                true
            }
            DetailView::Artist(artist) => {
                ui.horizontal(|ui| {
                    if icons::icon_pill_button(
                    ui,
                    icons::Icon::ArrowBack,
                    "Volver",
                    BG_ELEVATED,
                    TEXT_PRIMARY,
                )
                .clicked() {
                        self.detail_view = DetailView::None;
                    }
                    ui.heading(&artist.summary.name);
                });
                if let Some(subs) = &artist.summary.subscribers {
                    ui.label(RichText::new(subs).small().color(TEXT_SECONDARY));
                }
                if !artist.top_tracks.is_empty() {
                    ui.add_space(4.0);
                    self.render_collection_actions(ui, ctx, &artist.top_tracks, false);
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if !artist.top_tracks.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new("Canciones populares").size(16.0).strong());
                        for (idx, track) in artist.top_tracks.iter().enumerate() {
                            self.render_track_row(ui, ctx, track, &artist.top_tracks, idx, false);
                        }
                    }
                    if !artist.albums.is_empty() {
                        ui.add_space(10.0);
                        ui.label(RichText::new("Álbumes").size(16.0).strong());
                        for album in &artist.albums {
                            self.render_album_row(ui, ctx, album);
                        }
                    }
                });
                true
            }
            DetailView::Mood { title, playlists } => {
                ui.horizontal(|ui| {
                    if icons::icon_pill_button(
                    ui,
                    icons::Icon::ArrowBack,
                    "Volver",
                    BG_ELEVATED,
                    TEXT_PRIMARY,
                )
                .clicked() {
                        self.detail_view = DetailView::None;
                    }
                    ui.heading(&title);
                });
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for pl in &playlists {
                        self.render_playlist_row(ui, ctx, pl);
                    }
                });
                true
            }
        }
    }

    fn render_shelf_header(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        title: &str,
        eyebrow: Option<&str>,
        show_avatar: bool,
        play_tracks: Option<&[Track]>,
    ) {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            if show_avatar {
                let avatar_url = self
                    .home_feed
                    .account
                    .as_ref()
                    .and_then(|a| a.avatar_url.as_deref());
                if let Some(url) = avatar_url {
                    let thumb_uri = tracked_thumb_uri(url, 192);
                    ui.add(
                        Image::new(thumb_uri)
                            .fit_to_exact_size(Vec2::splat(30.0))
                            .corner_radius(CornerRadius::same(15)),
                    );
                } else {
                    let (av_rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                    ui.painter()
                        .circle_filled(av_rect.center(), 15.0, BG_ELEVATED);
                    icons::draw_icon(
                        ui.painter(),
                        egui::Rect::from_center_size(av_rect.center(), Vec2::splat(16.0)),
                        icons::Icon::Account,
                        TEXT_SECONDARY,
                    );
                }
                ui.add_space(4.0);
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                if let Some(eb) = eyebrow {
                    ui.label(
                        RichText::new(eb.to_uppercase())
                            .size(11.0)
                            .color(TEXT_SECONDARY),
                    );
                }
                ui.label(
                    RichText::new(title)
                        .size(19.5)
                        .strong()
                        .color(TEXT_PRIMARY),
                );
            });
            if let Some(tracks) = play_tracks.filter(|t| !t.is_empty()) {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let btn = egui::Button::new(
                        RichText::new("Reproducir todo")
                            .size(12.0)
                            .color(TEXT_PRIMARY),
                    )
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(72, 72, 76)))
                    .corner_radius(CornerRadius::same(16));
                    if ui.add(btn).clicked() {
                        self.play_track_in_list(tracks.to_vec(), 0, ctx);
                    }
                });
            }
        });
        ui.add_space(6.0);
    }

    fn render_compact_ytm_row(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        track: &Track,
        subtitle: &str,
        list: &[Track],
        index: usize,
        col_width: f32,
    ) {
        let is_current = self
            .queue
            .lock()
            .unwrap()
            .current_track()
            .map(|t| t.id == track.id)
            .unwrap_or(false);
        let row_bg = if is_current {
            Color32::from_rgb(26, 26, 28)
        } else {
            Color32::TRANSPARENT
        };
        let is_downloaded = self
            .downloads
            .iter()
            .any(|d| d.track.id == track.id && d.status == DownloadStatus::Completed);

        egui::Frame::new()
            .fill(row_bg)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(4, 4))
            .show(ui, |ui| {
                ui.set_width(col_width);
                ui.horizontal(|ui| {
                    let row_start = ui.cursor().min;
                    let thumb_sz = 48.0;
                    let row_h = 54.0;
                    render_track_thumb(ui, track, thumb_sz);
                    let text_w = (col_width - (thumb_sz + 48.0)).max(115.0);
                    let title_color = if is_current { ACCENT_RED } else { TEXT_PRIMARY };
                    ui.allocate_ui(Vec2::new(text_w, row_h), |ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.set_max_width(text_w);
                            render_clamped_label(ui, &track.title, 13.8, title_color, text_w, 2);
                            let sub_txt = if subtitle.is_empty() {
                                track.artist.clone()
                            } else {
                                subtitle.to_string()
                            };
                            ui.add(
                                egui::Label::new(
                                    RichText::new(sub_txt)
                                        .size(12.0)
                                        .color(if is_downloaded {
                                            GREEN_OK
                                        } else {
                                            TEXT_SECONDARY
                                        }),
                                )
                                .truncate()
                                .selectable(false),
                            );
                        });
                    });

                    let hit_rect = egui::Rect::from_min_size(
                        row_start,
                        Vec2::new(thumb_sz + 8.0 + text_w, row_h),
                    );
                    let row_hit = ui.interact(
                        hit_rect,
                        ui.id().with(("ytm_row", &track.id, index)),
                        Sense::click(),
                    );
                    if row_hit.clicked() {
                        self.play_track_in_list(list.to_vec(), index, ctx);
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icons::icon_button(
                            ui,
                            icons::Icon::MoreVert,
                            34.0,
                            17.0,
                            Color32::TRANSPARENT,
                            TEXT_SECONDARY,
                            17.0,
                        )
                        .clicked()
                        {
                            self.action_sheet_track = Some(track.clone());
                            self.action_sheet_context = Some((list.to_vec(), index));
                        }
                    });
                });
            });
    }

    fn activate_shelf_card(
        &mut self,
        card: &HomeShelfCard,
        sibling_tracks: &[Track],
        ctx: &egui::Context,
    ) {
        match &card.kind {
            ShelfCardKind::Track(track) => {
                if let Some(pos) = sibling_tracks.iter().position(|t| t.id == track.id) {
                    self.play_track_in_list(sibling_tracks.to_vec(), pos, ctx);
                } else {
                    self.play_track_in_list(vec![track.clone()], 0, ctx);
                }
            }
            ShelfCardKind::Album(album) => {
                self.open_album(album.id.clone(), ctx);
            }
            ShelfCardKind::Playlist(pl) => {
                self.open_playlist(pl.id.clone(), ctx);
            }
        }
    }

    fn render_quick_picks_4row_shelf(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        salt: &str,
        shelf: &HomeShelf,
    ) {
        if shelf.tracks.is_empty() {
            return;
        }
        self.render_shelf_header(
            ui,
            ctx,
            &shelf.title,
            shelf.eyebrow.as_deref(),
            false,
            Some(&shelf.tracks),
        );

        let col_width = (ui.available_width() - 48.0).clamp(248.0, 330.0);
        let all_tracks = shelf.tracks.clone();
        let all_cards = shelf.cards.clone();

        egui::ScrollArea::horizontal()
            .id_salt(salt)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for (chunk_idx, chunk) in all_tracks.chunks(4).enumerate() {
                        ui.vertical(|ui| {
                            ui.set_width(col_width);
                            ui.spacing_mut().item_spacing.y = 2.0;
                            for (row_i, track) in chunk.iter().enumerate() {
                                let abs_idx = chunk_idx * 4 + row_i;
                                let subtitle = all_cards
                                    .get(abs_idx)
                                    .map(|c| c.subtitle.as_str())
                                    .unwrap_or(&track.artist);
                                self.render_compact_ytm_row(
                                    ui,
                                    ctx,
                                    track,
                                    subtitle,
                                    &all_tracks,
                                    abs_idx,
                                    col_width,
                                );
                            }
                        });
                    }
                });
            });
    }

    fn render_speed_dial_grid_shelf(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        salt: &str,
        shelf: &HomeShelf,
    ) {
        if shelf.cards.is_empty() {
            return;
        }
        let eyebrow: Option<String> = shelf
            .eyebrow
            .clone()
            .or_else(|| self.home_feed.account.as_ref().map(|a| a.name.clone()))
            .or_else(|| Some("PARA TI".to_string()));

        self.render_shelf_header(ui, ctx, &shelf.title, eyebrow.as_deref(), true, None);

        let avail = ui.available_width();
        let gap = 8.0_f32;
        let tile_size = ((avail - gap * 2.0 - 8.0) / 3.0).clamp(90.0, 140.0);
        let page_width = tile_size * 3.0 + gap * 2.0;

        let mut cards = shelf.cards.clone();
        if cards.len() < 9 {
            let mut seen_titles: HashSet<String> = cards
                .iter()
                .map(|c| c.title.trim().to_lowercase())
                .collect();
            for extra in self.synthesize_speed_dial_cards() {
                if cards.len() >= 18 {
                    break;
                }
                if seen_titles.insert(extra.title.trim().to_lowercase()) {
                    cards.push(extra);
                }
            }
        }
        let mut sibling_tracks = shelf.tracks.clone();
        let mut seen_track_ids: HashSet<String> =
            sibling_tracks.iter().map(|t| t.id.clone()).collect();
        for c in &cards {
            if let ShelfCardKind::Track(t) = &c.kind {
                if seen_track_ids.insert(t.id.clone()) {
                    sibling_tracks.push(t.clone());
                }
            }
        }
        let pages: Vec<&[HomeShelfCard]> = cards.chunks(9).collect();
        let draw_page = |ui: &mut egui::Ui, page: &[HomeShelfCard], app: &mut Self| {
            ui.vertical(|ui| {
                ui.set_width(page_width);
                ui.spacing_mut().item_spacing.y = gap;
                for row in page.chunks(3) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for card in row {
                            let (rect, resp) =
                                ui.allocate_exact_size(Vec2::splat(tile_size), Sense::click());
                            if ui.is_rect_visible(rect) {
                                ui.painter().rect_filled(
                                    rect,
                                    CornerRadius::same(8),
                                    BG_SURFACE,
                                );
                                let note_rect = egui::Rect::from_center_size(
                                    rect.center(),
                                    Vec2::splat((tile_size * 0.34).clamp(18.0, 42.0)),
                                );
                                icons::draw_icon(
                                    ui.painter(),
                                    note_rect,
                                    icons::Icon::MusicNote,
                                    Color32::from_white_alpha(80),
                                );
                                if let Some(url) = &card.thumbnail_url {
                                    let thumb_uri = tracked_thumb_uri(url, 224);
                                    paint_ready_image(
                                        ui,
                                        rect,
                                        thumb_uri,
                                        Vec2::splat(tile_size),
                                        CornerRadius::same(8),
                                    );
                                }
                                let title_job = clamped_layout_job(
                                    &card.title,
                                    11.5,
                                    Color32::WHITE,
                                    tile_size - 12.0,
                                    1,
                                );
                                let galley = ui.painter().layout_job(title_job);
                                let overlay_h = (galley.size().y + 10.0).clamp(24.0, 28.0);
                                let overlay_rect = egui::Rect::from_min_max(
                                    egui::pos2(rect.left(), rect.bottom() - overlay_h),
                                    rect.max,
                                );
                                let band_h = 5.0;
                                for (idx, alpha) in [24_u8, 64, 118, 168].into_iter().enumerate() {
                                    let band_bottom = overlay_rect.top() - (3 - idx) as f32 * band_h;
                                    let band_top = (band_bottom - band_h).max(rect.top());
                                    if band_bottom > band_top {
                                        ui.painter().rect_filled(
                                            egui::Rect::from_min_max(
                                                egui::pos2(rect.left(), band_top),
                                                egui::pos2(rect.right(), band_bottom),
                                            ),
                                            CornerRadius::ZERO,
                                            Color32::from_rgba_unmultiplied(0, 0, 0, alpha),
                                        );
                                    }
                                }
                                ui.painter().rect_filled(
                                    overlay_rect,
                                    CornerRadius {
                                        nw: 0,
                                        ne: 0,
                                        sw: 8,
                                        se: 8,
                                    },
                                    Color32::from_rgba_unmultiplied(0, 0, 0, 200),
                                );
                                let clipped_painter = ui.painter().with_clip_rect(overlay_rect);
                                clipped_painter.galley(
                                    egui::pos2(
                                        rect.left() + 6.0,
                                        overlay_rect.center().y - galley.size().y * 0.5,
                                    ),
                                    galley,
                                    Color32::WHITE,
                                );
                            }
                            if resp.clicked() {
                                app.activate_shelf_card(card, &sibling_tracks, ctx);
                            }
                        }
                    });
                }
            });
        };

        let mut active_page = 0usize;
        if pages.len() <= 1 {
            if let Some(page) = pages.first() {
                draw_page(ui, page, self);
            }
        } else {
            let hscroll = egui::ScrollArea::horizontal()
                .id_salt(salt)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        for page in &pages {
                            draw_page(ui, page, self);
                        }
                    });
                });
            let step = (page_width + 12.0).max(1.0);
            active_page = ((hscroll.state.offset.x / step).round() as usize)
                .min(pages.len().saturating_sub(1));
        }

        if pages.len() > 1 {
            ui.add_space(4.0);
            let dot_count = pages.len().min(4);
            ui.horizontal(|ui| {
                ui.add_space(((avail - dot_count as f32 * 14.0) * 0.5).max(0.0));
                for i in 0..dot_count {
                    let (dot_r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(
                        dot_r.center(),
                        3.0,
                        if i == active_page {
                            Color32::WHITE
                        } else {
                            TEXT_SECONDARY
                        },
                    );
                }
            });
        }
    }

    fn render_carousel_cards_shelf(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        salt: &str,
        shelf: &HomeShelf,
    ) {
        if shelf.cards.is_empty() {
            return;
        }
        let play_tracks = if !shelf.tracks.is_empty() {
            Some(shelf.tracks.as_slice())
        } else {
            None
        };
        self.render_shelf_header(
            ui,
            ctx,
            &shelf.title,
            shelf.eyebrow.as_deref(),
            false,
            play_tracks,
        );

        let card_w = 148.0_f32;
        let sibling_tracks = shelf.tracks.clone();
        egui::ScrollArea::horizontal()
            .id_salt(salt)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    for (cidx, card) in shelf.cards.iter().take(16).enumerate() {
                        let card_start = ui.next_widget_position();
                        ui.vertical(|ui| {
                            ui.set_width(card_w);
                            ui.spacing_mut().item_spacing.y = 4.0;
                            render_thumb(ui, card.thumbnail_url.as_deref(), card_w);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&card.title)
                                        .strong()
                                        .size(13.5)
                                        .color(TEXT_PRIMARY),
                                )
                                .truncate()
                                .selectable(false),
                            );
                            if !card.subtitle.is_empty() {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&card.subtitle)
                                            .size(11.5)
                                            .color(TEXT_SECONDARY),
                                    )
                                    .truncate()
                                    .selectable(false),
                                );
                            }
                        });
                        let card_rect = egui::Rect::from_min_size(
                            card_start,
                            Vec2::new(card_w, card_w + 42.0),
                        );
                        if ui
                            .interact(
                                card_rect,
                                ui.make_persistent_id(("carousel_card", salt, cidx, &card.title)),
                                Sense::click(),
                            )
                            .clicked()
                        {
                            self.activate_shelf_card(card, &sibling_tracks, ctx);
                        }
                    }
                });
            });
    }

    fn synthesize_speed_dial_cards(&self) -> Vec<HomeShelfCard> {
        let mut cards = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        let mut seen_thumbs = std::collections::HashSet::new();

        let mut push_unique = |id_key: String, card: HomeShelfCard| {
            if cards.len() >= 18 || !seen_ids.insert(id_key) {
                return;
            }
            if let Some(t) = card.thumbnail_url.as_deref() {
                if !t.is_empty() && !seen_thumbs.insert(t.to_string()) {
                    return;
                }
            }
            cards.push(card);
        };

        for card in &self.store.data.pinned_cards {
            push_unique(format!("pin:{}", card.title), card.clone());
        }

        for (idx, pl) in self.home_feed.user_playlists.iter().enumerate() {
            let sub = if idx == 0 {
                "Lista".to_string()
            } else {
                pl.author.clone().unwrap_or_default()
            };
            push_unique(
                format!("pl:{}", pl.id),
                HomeShelfCard {
                    title: pl.title.clone(),
                    subtitle: sub,
                    thumbnail_url: pl.thumbnail_url.clone(),
                    kind: ShelfCardKind::Playlist(pl.clone()),
                },
            );
        }

        for t in self
            .home_feed
            .listen_again
            .iter()
            .chain(
                self.store
                    .data
                    .recent_tracks
                    .iter()
                    .filter(|t| !t.id.starts_with("local_demo_")),
            )
        {
            push_unique(
                format!("tr:{}", t.id),
                HomeShelfCard {
                    title: t.title.clone(),
                    subtitle: t.artist.clone(),
                    thumbnail_url: t.thumbnail_url.clone(),
                    kind: ShelfCardKind::Track(t.clone()),
                },
            );
        }

        for shelf in &self.home_feed.shelves {
            if shelf.layout == HomeShelfLayout::CarouselCards {
                for c in shelf.cards.iter().take(4) {
                    push_unique(format!("card:{}", c.title), c.clone());
                }
            }
        }

        for t in self
            .home_feed
            .recommended_tracks
            .iter()
            .chain(self.home_feed.quick_picks.iter().skip(4))
        {
            push_unique(
                format!("tr:{}", t.id),
                HomeShelfCard {
                    title: t.title.clone(),
                    subtitle: t.artist.clone(),
                    thumbnail_url: t.thumbnail_url.clone(),
                    kind: ShelfCardKind::Track(t.clone()),
                },
            );
        }
        cards
    }

    fn render_home(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let expected_v_drag = self.home_v_drag_id;
        let scroll_out = egui::ScrollArea::vertical()
            .id_salt("home_vertical_scroll")
            .show(ui, |ui| {
                // Mood / Genre Chips row (YT Music style)
            if !self.home_feed.moods.is_empty() {
                let moods = self.home_feed.moods.clone();
                egui::ScrollArea::horizontal()
                    .id_salt("home_moods")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            for mood in moods {
                                let is_selected =
                                    self.active_mood_title.as_deref() == Some(mood.title.as_str());
                                let btn = egui::Button::new(
                                    RichText::new(&mood.title).size(13.0).color(if is_selected {
                                        Color32::BLACK
                                    } else {
                                        TEXT_PRIMARY
                                    }),
                                )
                                .fill(if is_selected {
                                    Color32::WHITE
                                } else {
                                    Color32::from_rgb(32, 32, 35)
                                })
                                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(55, 55, 60)))
                                .corner_radius(CornerRadius::same(8));
                                if ui.add(btn).clicked() {
                                    if is_selected {
                                        self.active_mood_title = None;
                                        self.refresh_home(ctx);
                                    } else if mood.params.is_some() {
                                        self.active_mood_title = Some(mood.title);
                                        self.refresh_home_with_params(mood.params, ctx);
                                    } else {
                                        self.open_mood(mood.id, mood.title, ctx);
                                    }
                                }
                            }
                        });
                    });
                ui.add_space(6.0);
            }

            if !self.home_feed.is_logged_in && !self.dismiss_google_banner {
                egui::Frame::new()
                    .fill(BG_SURFACE)
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(48, 48, 54)))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        let text_w = (ui.available_width() - 154.0).max(120.0);
                        ui.horizontal(|ui| {
                            ui.allocate_ui(Vec2::new(text_w, 36.0), |ui| {
                                ui.vertical(|ui| {
                                    ui.set_max_width(text_w);
                                    ui.label(
                                        RichText::new("Conecta tu cuenta de Google")
                                            .strong()
                                            .size(13.5)
                                            .color(TEXT_PRIMARY),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(
                                                "Carga tus mixes, accesos directos y listas",
                                            )
                                            .size(11.0)
                                            .color(TEXT_SECONDARY),
                                        )
                                        .wrap(),
                                    );
                                });
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icons::icon_button(
                                    ui,
                                    icons::Icon::Close,
                                    26.0,
                                    13.0,
                                    Color32::TRANSPARENT,
                                    TEXT_SECONDARY,
                                    13.0,
                                )
                                .clicked()
                                {
                                    self.dismiss_google_banner = true;
                                }
                                ui.add_space(4.0);
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Account,
                                    "Conectar",
                                    ACCENT_RED,
                                    Color32::WHITE,
                                )
                                .clicked()
                                {
                                    self.show_google_login = true;
                                }
                            });
                        });
                    });
            }

            let home_empty =
                self.home_feed.shelves.is_empty() && self.home_feed.listen_again.is_empty();
            if self.home_loading && home_empty {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Cargando YouTube Music...").color(TEXT_SECONDARY));
                });
            }

            let mut shelves = self.home_feed.shelves.clone();
            if shelves.is_empty() && !self.home_feed.quick_picks.is_empty() {
                shelves.push(HomeShelf {
                    title: "Selección rápida".to_string(),
                    eyebrow: None,
                    layout: HomeShelfLayout::QuickPicks4Row,
                    tracks: self.home_feed.quick_picks.clone(),
                    cards: Vec::new(),
                });
            }

            let has_speed_dial = shelves
                .iter()
                .any(|s| s.layout == HomeShelfLayout::SpeedDialGrid && !s.cards.is_empty());

            let mut injected_speed_dial = false;
            for (i, shelf) in shelves.iter().enumerate() {
                let salt = format!("home_shelf_{i}");
                match shelf.layout {
                    HomeShelfLayout::QuickPicks4Row => {
                        self.render_quick_picks_4row_shelf(ui, ctx, &salt, shelf);
                    }
                    HomeShelfLayout::SpeedDialGrid => {
                        self.render_speed_dial_grid_shelf(ui, ctx, &salt, shelf);
                    }
                    HomeShelfLayout::CarouselCards => {
                        self.render_carousel_cards_shelf(ui, ctx, &salt, shelf);
                    }
                }
                if !has_speed_dial && !injected_speed_dial && i == 0 {
                    injected_speed_dial = true;
                    let sd_cards = self.synthesize_speed_dial_cards();
                    if !sd_cards.is_empty() {
                        let sd_shelf = HomeShelf {
                            title: "Accesos directos".to_string(),
                            eyebrow: self
                                .home_feed
                                .account
                                .as_ref()
                                .map(|a| a.name.to_uppercase())
                                .or_else(|| Some("PARA TI".to_string())),
                            layout: HomeShelfLayout::SpeedDialGrid,
                            tracks: sd_cards
                                .iter()
                                .filter_map(|c| match &c.kind {
                                    ShelfCardKind::Track(t) => Some(t.clone()),
                                    _ => None,
                                })
                                .collect(),
                            cards: sd_cards,
                        };
                        self.render_speed_dial_grid_shelf(ui, ctx, "home_synth_sd", &sd_shelf);
                    }
                }
            }

            let has_listen_again = shelves.iter().any(|s| {
                let low = s.title.to_lowercase();
                low.contains("volver a escuchar")
                    || low.contains("escucha de nuevo")
                    || low.contains("listen again")
            });
            if !has_listen_again {
                let mut seen_la = std::collections::HashSet::new();
                let mut la_tracks = Vec::new();
                for t in self
                    .home_feed
                    .listen_again
                    .iter()
                    .chain(
                        self.store
                            .data
                            .recent_tracks
                            .iter()
                            .filter(|t| !t.id.starts_with("local_demo_")),
                    )
                {
                    if seen_la.insert(t.id.clone()) {
                        la_tracks.push(t.clone());
                        if la_tracks.len() >= 12 {
                            break;
                        }
                    }
                }
                if la_tracks.len() >= 2 {
                    let la_cards = la_tracks
                        .iter()
                        .map(|t| HomeShelfCard {
                            title: t.title.clone(),
                            subtitle: t.artist.clone(),
                            thumbnail_url: t.thumbnail_url.clone(),
                            kind: ShelfCardKind::Track(t.clone()),
                        })
                        .collect();
                    let la_shelf = HomeShelf {
                        title: "Volver a escuchar".to_string(),
                        eyebrow: Some("TU HISTORIAL".to_string()),
                        layout: HomeShelfLayout::CarouselCards,
                        tracks: la_tracks,
                        cards: la_cards,
                    };
                    self.render_carousel_cards_shelf(ui, ctx, "home_listen_again", &la_shelf);
                }
            }

            let offline_tracks: Vec<Track> = self
                .downloads
                .iter()
                .filter(|d| d.status == DownloadStatus::Completed)
                .map(|d| d.track.clone())
                .collect();

            if !offline_tracks.is_empty() {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Disponibles sin conexión")
                        .size(16.0)
                        .strong()
                        .color(GREEN_OK),
                );
                for (idx, track) in offline_tracks.iter().take(6).enumerate() {
                    self.render_track_row(ui, ctx, track, &offline_tracks, idx, true);
                }
            }

            if !self.store.data.recent_tracks.is_empty() {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Escuchadas recientemente")
                        .size(16.0)
                        .strong()
                        .color(TEXT_PRIMARY),
                );
                let recents = self.store.data.recent_tracks.clone();
                for (idx, track) in recents.iter().take(8).enumerate() {
                    self.render_track_row(ui, ctx, track, &recents, idx, false);
                }
            }

            if let Some(v_id) = expected_v_drag {
                let other_dragged = ctx.dragged_id().is_some_and(|id| id != v_id);
                if other_dragged {
                    let clip = ui.clip_rect();
                    let stolen_vertical_dy = ui.input(|i| {
                        let origin_inside =
                            i.pointer.press_origin().is_some_and(|p| clip.contains(p));
                        if origin_inside && i.pointer.any_down() {
                            let d = i.pointer.delta();
                            if d.y.abs() > d.x.abs() * 0.75 && d.y.abs() > 0.25 {
                                Some(d.y)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    });
                    if let Some(dy) = stolen_vertical_dy {
                        ui.scroll_with_delta_animation(
                            Vec2::new(0.0, dy),
                            egui::style::ScrollAnimation::none(),
                        );
                    }
                }
            }
        });
        self.home_v_drag_id = Some(scroll_out.id.with("area"));
    }

    /// Rebuilds the deduplicated vertical Samples discovery deck from loaded explore and home feeds.
    fn rebuild_samples_deck(&mut self) {
        let mut seen = HashSet::new();
        let mut deck = Vec::new();
        let candidates = self
            .explore_feed
            .trending_tracks
            .iter()
            .chain(self.home_feed.recommended_tracks.iter())
            .chain(self.home_feed.quick_picks.iter())
            .chain(self.home_feed.shelves.iter().flat_map(|s| s.tracks.iter()));
        for t in candidates {
            if t.id.starts_with("local_demo_") || !seen.insert(t.id.clone()) {
                continue;
            }
            deck.push(t.clone());
        }
        if !deck.is_empty() {
            let ranked = self.store.rank_recommendations(deck);
            if !ranked.is_empty() {
                self.sample_tracks = ranked;
                if self.sample_idx >= self.sample_tracks.len() {
                    self.sample_idx = 0;
                }
            }
        }
    }

    fn prewarm_sample_neighbor(&mut self, idx: usize) {
        let Some(track) = self.sample_tracks.get(idx).cloned() else {
            return;
        };
        if !self.sample_prewarmed.insert(track.id.clone()) {
            return;
        }
        let provider = Arc::clone(&self.provider);
        let quality = self.store.data.config.stream_quality;
        self.runtime.spawn(async move {
            let _ = provider.resolve_audio_stream(&track.id, quality).await;
        });
    }

    fn activate_sample_at(&mut self, idx: usize, ctx: &egui::Context) {
        if self.sample_tracks.is_empty() {
            return;
        }
        let clamped = idx.min(self.sample_tracks.len() - 1);
        self.sample_idx = clamped;
        self.sample_drag_y = 0.0;
        let deck = self.sample_tracks.clone();
        self.play_track_in_list(deck, clamped, ctx);
        self.now_playing_open = false;
        if clamped + 1 < self.sample_tracks.len() {
            self.prewarm_sample_neighbor(clamped + 1);
        }
    }

    fn render_samples(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        snap: &AudioSnapshot,
    ) {
        if !self.explore_loaded_once {
            self.load_explore(ctx);
        }
        if self.sample_tracks.is_empty() {
            self.rebuild_samples_deck();
        }

        if self.sample_tracks.is_empty() {
            let rect = ui.available_rect_before_wrap();
            paint_ambient_gradient(ui.painter(), rect, Color32::from_rgb(64, 24, 32));
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space((rect.height() * 0.34).max(40.0));
                    if self.explore_loading || self.home_loading {
                        ui.spinner();
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new("Cargando Samples...")
                                .size(15.0)
                                .color(TEXT_SECONDARY),
                        );
                    } else {
                        ui.label(
                            RichText::new("Sin muestras disponibles")
                                .size(16.0)
                                .strong()
                                .color(TEXT_PRIMARY),
                        );
                        ui.add_space(10.0);
                        if icons::icon_pill_button(
                            ui,
                            icons::Icon::Refresh,
                            "Reintentar",
                            BG_ELEVATED,
                            Color32::WHITE,
                        )
                        .clicked()
                        {
                            self.load_explore(ctx);
                            self.refresh_home(ctx);
                        }
                    }
                });
            });
            return;
        }

        if self.sample_idx >= self.sample_tracks.len() {
            self.sample_idx = 0;
        }
        let idx = self.sample_idx;
        let total_samples = self.sample_tracks.len();
        let track = self.sample_tracks[idx].clone();

        if let Some(url) = track.thumbnail_url.as_deref() {
            self.ensure_cover_color(Some(url), ctx);
        }
        self.prewarm_sample_neighbor(idx);
        if idx + 1 < total_samples {
            self.prewarm_sample_neighbor(idx + 1);
        }

        let ambient = track
            .thumbnail_url
            .as_deref()
            .and_then(|u| self.cover_colors.get(u).copied())
            .unwrap_or(Color32::from_rgb(76, 36, 44));
        let card_rect = ui.available_rect_before_wrap();
        paint_ambient_gradient(ui.painter(), card_rect, ambient);
        let bg_swipe_resp = ui.interact(card_rect, ui.id().with("samples_bg_swipe"), Sense::drag());

        let current_playing_id = self
            .queue
            .lock()
            .unwrap()
            .current_track()
            .map(|t| t.id.clone());
        let is_current = current_playing_id.as_deref() == Some(track.id.as_str());
        let is_playing = is_current && snap.status == PlaybackStatus::Playing;

        let pad_x = 16.0;
        let inner_rect = card_rect.shrink2(Vec2::new(pad_x, 10.0));
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(inner_rect), |ui| {
            // Top bar inside Samples card: badge + counter + live status
            ui.horizontal(|ui| {
                egui::Frame::new()
                    .fill(Color32::from_black_alpha(110))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(egui::Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let (ic_r, _) = ui.allocate_exact_size(Vec2::splat(13.0), Sense::hover());
                            icons::draw_icon(ui.painter(), ic_r, icons::Icon::Samples, ACCENT_RED);
                            ui.label(
                                RichText::new(format!("Samples • {} / {}", idx + 1, total_samples))
                                    .size(12.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                        });
                    });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let status_text = if is_current {
                        match &snap.status {
                            PlaybackStatus::Playing => "Reproduciendo",
                            PlaybackStatus::Paused => "En pausa",
                            PlaybackStatus::Loading => "Cargando audio...",
                            PlaybackStatus::Buffering => "Almacenando en búfer...",
                            PlaybackStatus::Error(_) => "Error de audio",
                            PlaybackStatus::Idle => "Listo",
                        }
                    } else {
                        "Toca para reproducir"
                    };
                    ui.label(
                        RichText::new(status_text)
                            .size(11.5)
                            .color(Color32::from_white_alpha(200)),
                    );
                });
            });

            let rail_w = 52.0;
            let body_h = (ui.available_height() - 92.0).max(160.0);
            ui.add_space(8.0);
            ui.allocate_ui(Vec2::new(inner_rect.width(), body_h), |ui| {
                ui.horizontal(|ui| {
                    let art_zone_w = (inner_rect.width() - rail_w - 8.0).max(150.0);
                    ui.allocate_ui(Vec2::new(art_zone_w, body_h), |ui| {
                        ui.vertical_centered(|ui| {
                            let art_size = (art_zone_w.min(body_h) - 8.0).clamp(150.0, 300.0);
                            let y_off = ((body_h - art_size) * 0.5).max(0.0);
                            ui.add_space(y_off);
                            let (art_rect, art_resp) =
                                ui.allocate_exact_size(Vec2::splat(art_size), Sense::click_and_drag());
                            let drag_y = (self.sample_drag_y * 0.3).clamp(-32.0, 32.0);
                            let visual_art_rect = art_rect.translate(Vec2::new(0.0, drag_y));
                            ui.painter().rect_filled(
                                visual_art_rect,
                                CornerRadius::same(16),
                                Color32::from_rgb(28, 28, 32),
                            );
                            let note_r = egui::Rect::from_center_size(
                                visual_art_rect.center(),
                                Vec2::splat((art_size * 0.28).clamp(32.0, 68.0)),
                            );
                            icons::draw_icon(
                                ui.painter(),
                                note_r,
                                icons::Icon::MusicNote,
                                Color32::from_white_alpha(120),
                            );
                            if let Some(url) = &track.thumbnail_url {
                                let hi_res = tracked_thumb_uri(url, 720);
                                paint_ready_image(
                                    ui,
                                    visual_art_rect,
                                    hi_res,
                                    Vec2::splat(art_size),
                                    CornerRadius::same(16),
                                );
                            }
                            if is_current
                                && matches!(
                                    snap.status,
                                    PlaybackStatus::Loading | PlaybackStatus::Buffering
                                )
                            {
                                ui.painter().circle_filled(
                                    visual_art_rect.center(),
                                    26.0,
                                    Color32::from_black_alpha(165),
                                );
                                let spin_r = egui::Rect::from_center_size(
                                    visual_art_rect.center(),
                                    Vec2::splat(26.0),
                                );
                                ui.put(spin_r, egui::Spinner::new().size(22.0));
                            } else if !is_playing {
                                ui.painter().circle_filled(
                                    visual_art_rect.center(),
                                    26.0,
                                    Color32::from_black_alpha(145),
                                );
                                let play_r = egui::Rect::from_center_size(
                                    visual_art_rect.center() + Vec2::new(2.0, 0.0),
                                    Vec2::splat(22.0),
                                );
                                icons::draw_icon(
                                    ui.painter(),
                                    play_r,
                                    icons::Icon::Play,
                                    Color32::WHITE,
                                );
                            }
                            self.handle_sample_vertical_drag(&art_resp, idx, total_samples, ctx);
                            if art_resp.clicked() {
                                if is_current
                                    && !matches!(
                                        snap.status,
                                        PlaybackStatus::Error(_) | PlaybackStatus::Idle
                                    )
                                {
                                    if is_playing {
                                        self.audio.pause();
                                    } else {
                                        self.audio.resume();
                                    }
                                } else {
                                    self.activate_sample_at(idx, ctx);
                                }
                            }
                        });
                    });

                    // Right-hand vertical action rail (Like, Save, Radio, More)
                    ui.allocate_ui(Vec2::new(rail_w, body_h), |ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space((body_h - 206.0).max(0.0) * 0.5);
                            let liked = self.store.is_liked(&track.id);
                            let rail_bg = Color32::from_black_alpha(120);
                            if icons::icon_button(
                                ui,
                                icons::Icon::ThumbUp,
                                42.0,
                                20.0,
                                rail_bg,
                                if liked { ACCENT_RED } else { Color32::WHITE },
                                21.0,
                            )
                            .clicked()
                            {
                                self.store.toggle_like(track.clone());
                            }
                            ui.add_space(12.0);
                            if icons::icon_button(
                                ui,
                                icons::Icon::PlaylistAdd,
                                42.0,
                                20.0,
                                rail_bg,
                                Color32::WHITE,
                                21.0,
                            )
                            .clicked()
                            {
                                self.playlist_picker_track = Some(track.clone());
                            }
                            ui.add_space(12.0);
                            if icons::icon_button(
                                ui,
                                icons::Icon::Radio,
                                42.0,
                                20.0,
                                rail_bg,
                                Color32::WHITE,
                                21.0,
                            )
                            .clicked()
                            {
                                self.start_radio(track.id.clone(), ctx);
                            }
                            ui.add_space(12.0);
                            if icons::icon_button(
                                ui,
                                icons::Icon::MoreVert,
                                42.0,
                                20.0,
                                rail_bg,
                                Color32::WHITE,
                                21.0,
                            )
                            .clicked()
                            {
                                self.action_sheet_track = Some(track.clone());
                                self.action_sheet_context =
                                    Some((self.sample_tracks.clone(), idx));
                            }
                        });
                    });
                });
            });

            // Bottom title, artist, step buttons & progress strip
            ui.horizontal(|ui| {
                let info_w = (inner_rect.width() - 96.0).max(120.0);
                let info_resp = ui.allocate_ui(Vec2::new(info_w, 66.0), |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        render_clamped_label(ui, &track.title, 17.0, Color32::WHITE, info_w, 2);
                        let sub = match track.duration_secs.filter(|&s| s > 0) {
                            Some(s) => format!("{} • {}", track.artist, format_seconds(s)),
                            None => track.artist.clone(),
                        };
                        ui.add(
                            egui::Label::new(
                                RichText::new(sub)
                                    .size(13.0)
                                    .color(Color32::from_white_alpha(200)),
                            )
                            .truncate(),
                        );
                        if let Some(next_t) = self.sample_tracks.get(idx + 1) {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("Siguiente: {}", next_t.title))
                                        .size(11.0)
                                        .color(Color32::from_white_alpha(145)),
                                )
                                .truncate(),
                            );
                        }
                    });
                });
                let info_hit = info_resp.response.interact(Sense::click_and_drag());
                self.handle_sample_vertical_drag(&info_hit, idx, total_samples, ctx);
                if info_hit.clicked() {
                    if !is_current {
                        self.activate_sample_at(idx, ctx);
                    }
                    self.now_playing_open = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let btn_bg = Color32::from_black_alpha(130);
                    let next_fg = if idx + 1 < total_samples {
                        Color32::WHITE
                    } else {
                        TEXT_SECONDARY
                    };
                    if icons::icon_button(
                        ui,
                        icons::Icon::ChevronDown,
                        38.0,
                        18.0,
                        btn_bg,
                        next_fg,
                        19.0,
                    )
                    .clicked()
                        && idx + 1 < total_samples
                    {
                        self.activate_sample_at(idx + 1, ctx);
                    }
                    ui.add_space(6.0);
                    let prev_fg = if idx > 0 {
                        Color32::WHITE
                    } else {
                        TEXT_SECONDARY
                    };
                    if icons::icon_button(
                        ui,
                        icons::Icon::ChevronUp,
                        38.0,
                        18.0,
                        btn_bg,
                        prev_fg,
                        19.0,
                    )
                    .clicked()
                        && idx > 0
                    {
                        self.activate_sample_at(idx - 1, ctx);
                    }
                });
            });

            if is_current {
                let total = snap
                    .duration_secs
                    .or(track.duration_secs.map(|d| d as f64))
                    .unwrap_or(1.0)
                    .max(1.0);
                let frac = (snap.position_secs / total).clamp(0.0, 1.0) as f32;
                let (bar_r, _) =
                    ui.allocate_exact_size(Vec2::new(inner_rect.width(), 3.0), Sense::hover());
                ui.painter()
                    .rect_filled(bar_r, 1.5, Color32::from_white_alpha(55));
                if frac > 0.0 {
                    let fill_r = egui::Rect::from_min_max(
                        bar_r.min,
                        egui::pos2(bar_r.left() + bar_r.width() * frac, bar_r.bottom()),
                    );
                    ui.painter().rect_filled(fill_r, 1.5, Color32::WHITE);
                }
            }
        });
        self.handle_sample_vertical_drag(&bg_swipe_resp, idx, total_samples, ctx);
    }

    fn render_search(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if !self.explore_loaded_once {
            self.load_explore(ctx);
        }

        egui::Frame::new()
            .fill(BG_ELEVATED)
            .corner_radius(CornerRadius::same(22))
            .inner_margin(egui::Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let input_w = (ui.available_width() - 78.0).max(120.0);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Escribe lo que quieres buscar...")
                            .frame(false)
                            .desired_width(input_w),
                    );
                    if resp.changed() {
                        self.last_search_input_at = Some(Instant::now());
                    }
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if !self.search_query.is_empty()
                        && icons::icon_button(
                            ui,
                            icons::Icon::Close,
                            28.0,
                            14.0,
                            Color32::TRANSPARENT,
                            TEXT_SECONDARY,
                            14.0,
                        )
                        .clicked()
                    {
                        self.search_query.clear();
                        self.search_results = SearchResults::default();
                        self.search_suggestions.clear();
                    }
                    if icons::icon_button(
                        ui,
                        icons::Icon::Search,
                        32.0,
                        17.0,
                        ACCENT_RED,
                        Color32::WHITE,
                        16.0,
                    )
                    .clicked()
                        || enter
                    {
                        self.trigger_search(ctx);
                    }
                });
            });

        if !self.search_suggestions.is_empty() && !self.search_loading {
            ui.add_space(6.0);
            let suggestions = self.search_suggestions.clone();
            egui::ScrollArea::horizontal()
                .id_salt("search_suggestions")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for s in suggestions.iter().take(8) {
                            let btn = egui::Button::new(
                                RichText::new(s).size(12.5).color(TEXT_PRIMARY),
                            )
                            .fill(BG_ELEVATED)
                            .corner_radius(CornerRadius::same(14));
                            if ui.add(btn).clicked() {
                                self.search_query = s.clone();
                                self.trigger_search(ctx);
                            }
                        }
                    });
                });
        }

        let has_results = !self.search_results.songs.is_empty()
            || !self.search_results.albums.is_empty()
            || !self.search_results.artists.is_empty()
            || !self.search_results.playlists.is_empty();

        if has_results || self.search_loading {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (filt, label) in [
                    (SearchFilter::Songs, "Canciones"),
                    (SearchFilter::Albums, "Álbumes"),
                    (SearchFilter::Artists, "Artistas"),
                    (SearchFilter::Playlists, "Playlists"),
                ] {
                    let active = self.search_filter == filt;
                    let btn = egui::Button::new(
                        RichText::new(label)
                            .size(13.0)
                            .color(if active { Color32::BLACK } else { TEXT_PRIMARY }),
                    )
                    .fill(if active { Color32::WHITE } else { BG_ELEVATED })
                    .corner_radius(CornerRadius::same(16));
                    if ui.add(btn).clicked() {
                        self.search_filter = filt;
                    }
                }
            });
        }

        if self.search_loading {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Buscando en YT Music...").color(TEXT_SECONDARY));
            });
        }

        egui::ScrollArea::vertical()
            .id_salt("search_scroll")
            .show(ui, |ui| {
                let mut shown_local_ids: HashSet<String> = HashSet::new();
                let q_trimmed = self.search_query.trim().to_string();
                if q_trimmed.len() >= 2 {
                    let mut local_hits = self.store.search_local(&q_trimmed);
                    let q_low = q_trimmed.to_lowercase();
                    for dl in &self.downloads {
                        if dl.status == DownloadStatus::Completed
                            && (dl.track.title.to_lowercase().contains(&q_low)
                                || dl.track.artist.to_lowercase().contains(&q_low))
                            && !local_hits.iter().any(|t| t.id == dl.track.id)
                        {
                            local_hits.push(dl.track.clone());
                        }
                    }
                    if !local_hits.is_empty() {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("En tu biblioteca (instantáneo)")
                                .size(13.0)
                                .strong()
                                .color(GREEN_OK),
                        );
                        ui.add_space(4.0);
                        for (idx, t) in local_hits.iter().take(4).enumerate() {
                            shown_local_ids.insert(t.id.clone());
                            self.render_track_row(ui, ctx, t, &local_hits, idx, true);
                            ui.add_space(3.0);
                        }
                    }
                }

                if !has_results && !self.search_loading {
                    // Búsquedas recientes visual strip + history rows (matching YT Music)
                    let mut recent_thumbs: Vec<(String, Option<String>)> = self
                        .store
                        .data
                        .search_history_items
                        .iter()
                        .filter(|i| i.thumbnail_url.is_some())
                        .map(|i| (i.query.clone(), i.thumbnail_url.clone()))
                        .collect();
                    if recent_thumbs.is_empty() {
                        for t in self.store.data.recent_tracks.iter().take(6) {
                            recent_thumbs.push((t.title.clone(), t.thumbnail_url.clone()));
                        }
                    }

                    if !recent_thumbs.is_empty() {
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new("Búsquedas recientes")
                                .size(13.5)
                                .color(TEXT_SECONDARY),
                        );
                        ui.add_space(6.0);
                        egui::ScrollArea::horizontal()
                            .id_salt("recent_search_thumbs")
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 10.0;
                                    for (q, thumb) in recent_thumbs.iter().take(10) {
                                        ui.vertical(|ui| {
                                            ui.set_width(88.0);
                                            let r = ui
                                                .allocate_ui(Vec2::splat(88.0), |ui| {
                                                    render_thumb(ui, thumb.as_deref(), 88.0);
                                                })
                                                .response
                                                .interact(Sense::click());
                                            if r.clicked() {
                                                self.search_query = q.clone();
                                                self.trigger_search(ctx);
                                            }
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(q)
                                                        .size(12.0)
                                                        .color(TEXT_PRIMARY),
                                                )
                                                .truncate(),
                                            );
                                        });
                                    }
                                });
                            });
                    }

                    let history = self.store.data.search_history.clone();
                    if !history.is_empty() {
                        ui.add_space(8.0);
                        for item in history.iter().take(6) {
                            ui.horizontal(|ui| {
                                let row_start = ui.cursor().min;
                                let (icon_r, _) =
                                    ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                                icons::draw_icon(
                                    ui.painter(),
                                    egui::Rect::from_center_size(icon_r.center(), Vec2::splat(16.0)),
                                    icons::Icon::History,
                                    TEXT_SECONDARY,
                                );
                                let text_w = (ui.available_width() - 44.0).max(120.0);
                                ui.allocate_ui(Vec2::new(text_w, 34.0), |ui| {
                                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(item)
                                                    .size(14.5)
                                                    .color(TEXT_PRIMARY),
                                            )
                                            .truncate()
                                            .selectable(false),
                                        );
                                    });
                                });
                                let hit = ui.interact(
                                    egui::Rect::from_min_size(row_start, Vec2::new(text_w + 30.0, 34.0)),
                                    ui.id().with(("hist_row", item.as_str())),
                                    Sense::click(),
                                );
                                if hit.clicked() {
                                    self.search_query = item.clone();
                                    self.trigger_search(ctx);
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if icons::icon_button(
                                        ui,
                                        icons::Icon::ArrowUpLeft,
                                        32.0,
                                        15.0,
                                        Color32::TRANSPARENT,
                                        TEXT_SECONDARY,
                                        15.0,
                                    )
                                    .clicked()
                                    {
                                        self.search_query = item.clone();
                                    }
                                });
                            });
                        }
                    }

                    // 3 Explore cards: Nuevos lanzamientos | Rankings | Estados de ánimo
                    ui.add_space(14.0);
                    let card_w = ((ui.available_width() - 16.0) / 3.0).clamp(88.0, 140.0);
                    ui.columns(3, |cols| {
                        let explore_cards = [
                            (icons::Icon::NewReleases, "Nuevos lanzamientos", 0_u8),
                            (icons::Icon::Trending, "Rankings", 1_u8),
                            (icons::Icon::Mood, "Estados de ánimo", 2_u8),
                        ];
                        for (idx, (icon, label, mode)) in explore_cards.into_iter().enumerate() {
                            let col_ui = &mut cols[idx];
                            let (rect, resp) = col_ui.allocate_exact_size(
                                Vec2::new(card_w, 92.0),
                                Sense::click(),
                            );
                            if col_ui.is_rect_visible(rect) {
                                col_ui.painter().rect_filled(
                                    rect,
                                    CornerRadius::same(10),
                                    Color32::from_rgb(24, 24, 27),
                                );
                                icons::draw_icon(
                                    col_ui.painter(),
                                    egui::Rect::from_center_size(
                                        egui::pos2(rect.left() + 24.0, rect.top() + 24.0),
                                        Vec2::splat(19.0),
                                    ),
                                    icon,
                                    TEXT_PRIMARY,
                                );
                                let galley = col_ui.painter().layout(
                                    label.to_string(),
                                    egui::FontId::proportional(13.0),
                                    TEXT_PRIMARY,
                                    card_w - 20.0,
                                );
                                col_ui.painter().galley(
                                    egui::pos2(rect.left() + 10.0, rect.bottom() - galley.size().y - 10.0),
                                    galley,
                                    TEXT_PRIMARY,
                                );
                            }
                            if resp.clicked() {
                                match mode {
                                    0 => {
                                        self.search_results = SearchResults {
                                            albums: if !self.explore_feed.new_albums.is_empty() {
                                                self.explore_feed.new_albums.clone()
                                            } else {
                                                self.home_feed.new_albums.clone()
                                            },
                                            ..Default::default()
                                        };
                                        self.search_filter = SearchFilter::Albums;
                                    }
                                    1 => {
                                        self.search_results = SearchResults {
                                            songs: if !self.explore_feed.trending_tracks.is_empty() {
                                                self.explore_feed.trending_tracks.clone()
                                            } else {
                                                self.home_feed.trending_tracks.clone()
                                            },
                                            ..Default::default()
                                        };
                                        self.search_filter = SearchFilter::Songs;
                                    }
                                    _ => {
                                        if let Some(m) = self
                                            .explore_feed
                                            .moods
                                            .first()
                                            .or_else(|| self.home_feed.moods.first())
                                            .cloned()
                                        {
                                            if m.params.is_some() {
                                                self.active_tab = NavTab::Home;
                                                self.active_mood_title = Some(m.title);
                                                self.refresh_home_with_params(m.params, ctx);
                                            } else {
                                                self.open_mood(m.id, m.title, ctx);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    });

                    let trending = if !self.explore_feed.trending_tracks.is_empty() {
                        self.explore_feed.trending_tracks.clone()
                    } else if !self.home_feed.trending_tracks.is_empty() {
                        self.home_feed.trending_tracks.clone()
                    } else if !self.home_feed.recommended_tracks.is_empty() {
                        self.home_feed.recommended_tracks.clone()
                    } else {
                        self.home_feed.quick_picks.clone()
                    };
                    if !trending.is_empty() {
                        let ranked = self.store.rank_recommendations(trending);
                        let top: Vec<Track> = ranked.into_iter().take(4).collect();
                        self.render_shelf_header(
                            ui,
                            ctx,
                            "Tendencias ahora",
                            Some("EXPLORAR"),
                            false,
                            Some(&top),
                        );
                        ui.add_space(6.0);
                        for (idx, t) in top.iter().enumerate() {
                            self.render_track_row(ui, ctx, t, &top, idx, false);
                            ui.add_space(4.0);
                        }
                    }

                    let albums = if !self.explore_feed.new_albums.is_empty() {
                        self.explore_feed.new_albums.clone()
                    } else {
                        self.home_feed.new_albums.clone()
                    };
                    if !albums.is_empty() {
                        self.render_shelf_header(
                            ui,
                            ctx,
                            "Nuevos álbumes y sencillos",
                            None,
                            false,
                            None,
                        );
                        ui.add_space(6.0);
                        egui::ScrollArea::horizontal()
                            .id_salt("explore_albums_h")
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 12.0;
                                    for a in albums.iter().take(10) {
                                        ui.vertical(|ui| {
                                            ui.set_width(120.0);
                                            let r = ui
                                                .allocate_ui(Vec2::splat(120.0), |ui| {
                                                    render_thumb(
                                                        ui,
                                                        a.thumbnail_url.as_deref(),
                                                        120.0,
                                                    );
                                                })
                                                .response
                                                .interact(Sense::click());
                                            if r.clicked() {
                                                self.open_album(a.id.clone(), ctx);
                                            }
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&a.title)
                                                        .size(13.0)
                                                        .strong()
                                                        .color(TEXT_PRIMARY),
                                                )
                                                .truncate()
                                                .selectable(false),
                                            );
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&a.artist)
                                                        .size(11.5)
                                                        .color(TEXT_SECONDARY),
                                                )
                                                .truncate()
                                                .selectable(false),
                                            );
                                        });
                                    }
                                });
                            });
                    }

                    let moods = if !self.explore_feed.moods.is_empty() {
                        self.explore_feed.moods.clone()
                    } else {
                        self.home_feed.moods.clone()
                    };
                    if !moods.is_empty() {
                        self.render_shelf_header(
                            ui,
                            ctx,
                            "Estados de ánimo y géneros",
                            None,
                            false,
                            None,
                        );
                        ui.add_space(6.0);
                        egui::ScrollArea::horizontal()
                            .id_salt("explore_moods_h")
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    for m in moods.iter().take(12) {
                                        let btn = egui::Button::new(
                                            RichText::new(&m.title).size(13.0).color(TEXT_PRIMARY),
                                        )
                                        .fill(BG_ELEVATED)
                                        .corner_radius(CornerRadius::same(16));
                                        if ui.add(btn).clicked() {
                                            if m.params.is_some() {
                                                self.active_tab = NavTab::Home;
                                                self.active_mood_title = Some(m.title.clone());
                                                self.refresh_home_with_params(
                                                    m.params.clone(),
                                                    ctx,
                                                );
                                            } else {
                                                self.open_mood(
                                                    m.id.clone(),
                                                    m.title.clone(),
                                                    ctx,
                                                );
                                            }
                                        }
                                    }
                                });
                            });
                    }

                    return;
                }

                match self.search_filter {
                    SearchFilter::Songs => {
                        let songs: Vec<Track> = self
                            .search_results
                            .songs
                            .iter()
                            .filter(|t| !shown_local_ids.contains(&t.id))
                            .cloned()
                            .collect();
                        if !shown_local_ids.is_empty() && !songs.is_empty() {
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("En YouTube Music")
                                    .size(13.0)
                                    .color(TEXT_SECONDARY),
                            );
                            ui.add_space(4.0);
                        }
                        for (idx, track) in songs.iter().enumerate() {
                            self.render_track_row(ui, ctx, track, &songs, idx, false);
                        }
                    }
                    SearchFilter::Albums => {
                        let albums = self.search_results.albums.clone();
                        for album in &albums {
                            self.render_album_row(ui, ctx, album);
                        }
                    }
                    SearchFilter::Artists => {
                        let artists = self.search_results.artists.clone();
                        for artist in artists {
                            egui::Frame::new()
                                .fill(BG_SURFACE)
                                .corner_radius(CornerRadius::same(10))
                                .inner_margin(10.0)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        render_thumb(ui, artist.thumbnail_url.as_deref(), 48.0);
                                        ui.vertical(|ui| {
                                            ui.label(RichText::new(&artist.name).strong());
                                            if let Some(subs) = &artist.subscribers {
                                                ui.label(
                                                    RichText::new(subs)
                                                        .small()
                                                        .color(TEXT_SECONDARY),
                                                );
                                            }
                                        });
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            if ui.button("Ver").clicked() {
                                                self.open_artist(artist.id.clone(), ctx);
                                            }
                                        });
                                    });
                                });
                        }
                    }
                    SearchFilter::Playlists => {
                        let playlists = self.search_results.playlists.clone();
                        for pl in &playlists {
                            self.render_playlist_row(ui, ctx, pl);
                        }
                    }
                }
            });
    }

    fn render_album_row(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        album: &yr_core::AlbumSummary,
    ) {
        if render_collection_row(
            ui,
            ("album_row", &album.id),
            album.thumbnail_url.as_deref(),
            &album.title,
            Some(album.artist.as_str()),
        ) {
            self.open_album(album.id.clone(), ctx);
        }
    }

    fn render_playlist_row(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        pl: &PlaylistSummary,
    ) {
        if render_collection_row(
            ui,
            ("playlist_row", &pl.id),
            pl.thumbnail_url.as_deref(),
            &pl.title,
            pl.author.as_deref(),
        ) {
            self.open_playlist(pl.id.clone(), ctx);
        }
    }

    fn render_library(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Biblioteca")
                    .size(22.0)
                    .strong()
                    .color(TEXT_PRIMARY),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let plus_btn = egui::Button::new(
                    RichText::new("+ Nueva")
                        .size(13.0)
                        .strong()
                        .color(Color32::BLACK),
                )
                .fill(Color32::WHITE)
                .corner_radius(CornerRadius::same(16));
                if ui.add(plus_btn).clicked() {
                    self.show_new_playlist_dialog = !self.show_new_playlist_dialog;
                }
            });
        });

        ui.add_space(8.0);
        egui::ScrollArea::horizontal()
            .id_salt("lib_filters")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    for (filt, label) in [
                        (LibraryFilter::All, "Todo"),
                        (LibraryFilter::Downloads, "Descargas"),
                        (LibraryFilter::Playlists, "Playlists"),
                        (LibraryFilter::Songs, "Canciones"),
                        (LibraryFilter::Albums, "Álbumes"),
                        (LibraryFilter::Artists, "Artistas"),
                    ] {
                        let active = self.library_filter == filt;
                        let btn = egui::Button::new(
                            RichText::new(label)
                                .size(13.0)
                                .color(if active { Color32::BLACK } else { TEXT_PRIMARY }),
                        )
                        .fill(if active { Color32::WHITE } else { BG_ELEVATED })
                        .corner_radius(CornerRadius::same(8));
                        if ui.add(btn).clicked() {
                            self.library_filter = filt;
                        }
                    }
                });
            });

        if self.show_new_playlist_dialog {
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(BG_SURFACE)
                .corner_radius(CornerRadius::same(10))
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let input_w = (ui.available_width() - 88.0).max(120.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_playlist_name)
                                .desired_width(input_w)
                                .hint_text("Nombre de la playlist..."),
                        );
                        if ui.button("Crear").clicked() && !self.new_playlist_name.trim().is_empty()
                        {
                            let name = self.new_playlist_name.trim().to_string();
                            self.store.create_playlist(name);
                            self.new_playlist_name.clear();
                            self.show_new_playlist_dialog = false;
                        }
                    });
                });
        }

        ui.add_space(6.0);
        egui::Frame::new()
            .fill(BG_SURFACE)
            .corner_radius(CornerRadius::same(18))
            .inner_margin(egui::Margin::symmetric(12, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let input_w = (ui.available_width() - 36.0).max(120.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.library_search)
                            .hint_text("Filtrar tu biblioteca...")
                            .frame(false)
                            .desired_width(input_w),
                    );
                    if !self.library_search.is_empty()
                        && icons::icon_button(
                            ui,
                            icons::Icon::Close,
                            24.0,
                            12.0,
                            Color32::TRANSPARENT,
                            TEXT_SECONDARY,
                            12.0,
                        )
                        .clicked()
                    {
                        self.library_search.clear();
                    }
                });
            });

        let lib_q = self.library_search.trim().to_lowercase();
        let lib_matches = |text: &str| lib_q.is_empty() || text.to_lowercase().contains(&lib_q);

        ui.add_space(8.0);
        if self.library_filter == LibraryFilter::Downloads {
            self.render_downloads(ui, ctx);
            return;
        }

        egui::ScrollArea::vertical()
            .id_salt("lib_scroll")
            .show(ui, |ui| {
                // Pinned "Música que te gustó" gradient card
                if matches!(self.library_filter, LibraryFilter::All | LibraryFilter::Playlists | LibraryFilter::Songs) {
                    let liked_count = self.store.data.liked_tracks.len();
                    egui::Frame::new()
                        .fill(Color32::from_rgb(20, 20, 23))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (thumb_rect, _) =
                                    ui.allocate_exact_size(Vec2::splat(54.0), Sense::hover());
                                ui.painter().rect_filled(
                                    thumb_rect,
                                    CornerRadius::same(8),
                                    Color32::from_rgb(172, 92, 236),
                                );
                                icons::draw_icon(
                                    ui.painter(),
                                    egui::Rect::from_center_size(
                                        thumb_rect.center(),
                                        Vec2::splat(26.0),
                                    ),
                                    icons::Icon::ThumbUp,
                                    Color32::WHITE,
                                );
                                ui.add_space(8.0);
                                let btns_w = if liked_count > 0 { 82.0 } else { 0.0 };
                                let text_w = (ui.available_width() - 12.0 - btns_w).max(110.0);
                                ui.allocate_ui(Vec2::new(text_w, 54.0), |ui| {
                                    ui.vertical(|ui| {
                                        ui.label(
                                            RichText::new("Música que te gustó")
                                                .strong()
                                                .size(15.0)
                                                .color(TEXT_PRIMARY),
                                        );
                                        ui.label(
                                            RichText::new(format!(
                                                "Playlist autogenerada • {} canciones",
                                                liked_count
                                            ))
                                            .size(12.0)
                                            .color(TEXT_SECONDARY),
                                        );
                                    });
                                });
                                if liked_count > 0 {
                                    let liked_all = self.store.data.liked_tracks.clone();
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if icons::icon_button(
                                            ui,
                                            icons::Icon::Shuffle,
                                            34.0,
                                            17.0,
                                            BG_ELEVATED,
                                            TEXT_PRIMARY,
                                            16.0,
                                        )
                                        .clicked()
                                        {
                                            self.play_tracks_shuffled(liked_all.clone(), ctx);
                                        }
                                        if icons::icon_button(
                                            ui,
                                            icons::Icon::Play,
                                            34.0,
                                            17.0,
                                            ACCENT_RED,
                                            Color32::WHITE,
                                            16.0,
                                        )
                                        .clicked()
                                        {
                                            self.play_track_in_list(liked_all, 0, ctx);
                                        }
                                    });
                                }
                            });
                        });

                    let liked: Vec<Track> = self
                        .store
                        .data
                        .liked_tracks
                        .iter()
                        .filter(|t| lib_matches(&t.title) || lib_matches(&t.artist))
                        .cloned()
                        .collect();
                    if !liked.is_empty() && matches!(self.library_filter, LibraryFilter::All | LibraryFilter::Songs) {
                        ui.add_space(6.0);
                        for (idx, track) in liked.iter().enumerate() {
                            self.render_track_row(ui, ctx, track, &liked, idx, false);
                        }
                    }
                }

                if matches!(self.library_filter, LibraryFilter::All | LibraryFilter::Playlists) {
                    if !self.store.data.playlists.is_empty() {
                        ui.add_space(10.0);
                        ui.label(RichText::new("Tus playlists").size(16.0).strong());
                        let pls: Vec<_> = self
                            .store
                            .data
                            .playlists
                            .iter()
                            .filter(|p| {
                                lib_matches(&p.name)
                                    || p.tracks
                                        .iter()
                                        .any(|t| lib_matches(&t.title) || lib_matches(&t.artist))
                            })
                            .cloned()
                            .collect();
                        for pl in pls {
                            ui.horizontal(|ui| {
                                egui::CollapsingHeader::new(format!(
                                    "{} ({})",
                                    pl.name,
                                    pl.tracks.len()
                                ))
                                .id_salt(&pl.id)
                                .show(ui, |ui| {
                                    for (idx, track) in pl.tracks.iter().enumerate() {
                                        if !lib_matches(&pl.name)
                                            && !lib_matches(&track.title)
                                            && !lib_matches(&track.artist)
                                        {
                                            continue;
                                        }
                                        ui.horizontal(|ui| {
                                            let row_w = (ui.available_width() - 32.0).max(160.0);
                                            ui.allocate_ui(Vec2::new(row_w, 58.0), |ui| {
                                                self.render_track_row(
                                                    ui,
                                                    ctx,
                                                    track,
                                                    &pl.tracks,
                                                    idx,
                                                    false,
                                                );
                                            });
                                            if icons::icon_button(
                                                ui,
                                                icons::Icon::Close,
                                                26.0,
                                                12.0,
                                                Color32::TRANSPARENT,
                                                TEXT_SECONDARY,
                                                12.0,
                                            )
                                            .clicked()
                                            {
                                                self.store.remove_from_playlist(&pl.id, &track.id);
                                                self.set_banner(format!(
                                                    "Quitada de {}",
                                                    pl.name
                                                ));
                                            }
                                        });
                                    }
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if icons::icon_button(
                                        ui,
                                        icons::Icon::Trash,
                                        30.0,
                                        15.0,
                                        Color32::TRANSPARENT,
                                        TEXT_SECONDARY,
                                        15.0,
                                    )
                                    .clicked()
                                    {
                                        self.store.delete_playlist(&pl.id);
                                        self.set_banner(format!(
                                            "Playlist '{}' eliminada",
                                            pl.name
                                        ));
                                    }
                                });
                            });
                        }
                    }

                    if !self.home_feed.user_playlists.is_empty() {
                        ui.add_space(10.0);
                        ui.label(RichText::new("Playlists de YouTube Music").size(16.0).strong());
                        let ypls = self.home_feed.user_playlists.clone();
                        for pl in &ypls {
                            self.render_playlist_row(ui, ctx, pl);
                        }
                    }
                }

                if matches!(self.library_filter, LibraryFilter::All | LibraryFilter::Albums) && !self.home_feed.new_albums.is_empty() {
                    let albums: Vec<_> = self
                        .home_feed
                        .new_albums
                        .iter()
                        .filter(|a| lib_matches(&a.title) || lib_matches(&a.artist))
                        .cloned()
                        .collect();
                    if !albums.is_empty() {
                        ui.add_space(10.0);
                        ui.label(RichText::new("Álbumes").size(16.0).strong());
                        for a in &albums {
                            self.render_album_row(ui, ctx, a);
                        }
                    }
                }

                if self.library_filter == LibraryFilter::Artists {
                    ui.add_space(10.0);
                    ui.label(RichText::new("Artistas").size(16.0).strong());
                    let mut seen_artists = std::collections::HashSet::new();
                    let mut artists = Vec::new();
                    for t in self
                        .store
                        .data
                        .liked_tracks
                        .iter()
                        .chain(self.store.data.recent_tracks.iter())
                        .chain(self.home_feed.quick_picks.iter())
                    {
                        if let Some(aid) = t.artist_id.as_ref() {
                            if lib_matches(&t.artist) && seen_artists.insert(aid.clone()) {
                                artists.push((
                                    aid.clone(),
                                    t.artist.clone(),
                                    t.thumbnail_url.clone(),
                                ));
                            }
                        }
                    }
                    if artists.is_empty() {
                        ui.label(
                            RichText::new("Escucha canciones para ver tus artistas aquí.")
                                .small()
                                .color(TEXT_SECONDARY),
                        );
                    }
                    for (aid, name, thumb) in artists {
                        ui.horizontal(|ui| {
                            render_thumb(ui, thumb.as_deref(), 44.0);
                            ui.add_space(8.0);
                            if ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(&name)
                                            .size(15.0)
                                            .strong()
                                            .color(TEXT_PRIMARY),
                                    )
                                    .sense(Sense::click()),
                                )
                                .clicked()
                            {
                                self.open_artist(aid, ctx);
                            }
                        });
                        ui.add_space(4.0);
                    }
                }

                if self.library_filter == LibraryFilter::All && !self.downloads.is_empty() {
                    ui.add_space(10.0);
                    let completed: Vec<Track> = self
                        .downloads
                        .iter()
                        .filter(|d| d.status == DownloadStatus::Completed)
                        .map(|d| d.track.clone())
                        .collect();
                    if !completed.is_empty() {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("Descargas offline ({})", completed.len()))
                                    .size(16.0)
                                    .strong()
                                    .color(GREEN_OK),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Shuffle,
                                    "Aleatorio",
                                    BG_ELEVATED,
                                    TEXT_PRIMARY,
                                )
                                .clicked()
                                {
                                    self.play_tracks_shuffled(completed.clone(), ctx);
                                }
                                ui.add_space(6.0);
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Play,
                                    "Todo",
                                    BG_ELEVATED,
                                    TEXT_PRIMARY,
                                )
                                .clicked()
                                {
                                    self.play_track_in_list(completed.clone(), 0, ctx);
                                }
                            });
                        });
                        for (idx, t) in completed.iter().enumerate() {
                            self.render_track_row(ui, ctx, t, &completed, idx, true);
                        }
                    }
                }
            });
    }

    fn render_downloads(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.heading("Descargas");
            let mb = self.total_downloaded_bytes as f64 / (1024.0 * 1024.0);
            if mb > 0.0 {
                ui.label(
                    RichText::new(format!("({mb:.1} MB)"))
                        .small()
                        .color(TEXT_SECONDARY),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icons::icon_button(
                    ui,
                    icons::Icon::Refresh,
                    32.0,
                    16.0,
                    BG_ELEVATED,
                    TEXT_PRIMARY,
                    16.0,
                )
                .clicked()
                {
                    self.refresh_downloads(ctx);
                }
            });
        });

        let completed: Vec<Track> = self
            .downloads
            .iter()
            .filter(|d| d.status == DownloadStatus::Completed)
            .map(|d| d.track.clone())
            .collect();

        if !completed.is_empty()
            && icons::icon_pill_button(
                ui,
                icons::Icon::Play,
                "Reproducir todo offline",
                BG_ELEVATED,
                TEXT_PRIMARY,
            )
            .clicked()
        {
            self.play_track_in_list(completed.clone(), 0, ctx);
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            if self.downloads.is_empty() {
                ui.add_space(16.0);
                ui.label(
                    RichText::new("Aún no tienes descargas. Toca Descargar en cualquier canción.")
                        .color(TEXT_SECONDARY),
                );
                return;
            }

            let items = self.downloads.clone();
            for task in items {
                egui::Frame::new()
                    .fill(BG_SURFACE)
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            render_thumb(ui, task.track.thumbnail_url.as_deref(), 44.0);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&task.track.title).strong());
                                ui.label(
                                    RichText::new(&task.track.artist)
                                        .small()
                                        .color(TEXT_SECONDARY),
                                );
                                match &task.status {
                                    DownloadStatus::Queued => {
                                        ui.label(RichText::new("En cola...").small());
                                    }
                                    DownloadStatus::Downloading => {
                                        let pct = (task.progress() * 100.0) as u32;
                                        let mb_done =
                                            task.downloaded_bytes as f64 / (1024.0 * 1024.0);
                                        let speed_kb = task.speed_bps as f64 / 1024.0;
                                        let eta = task
                                            .eta_secs()
                                            .map(|s| format!(" • {s}s"))
                                            .unwrap_or_default();
                                        ui.add(
                                            egui::ProgressBar::new(task.progress()).text(format!(
                                                "{pct}% • {mb_done:.1} MB • {speed_kb:.0} KB/s{eta}"
                                            )),
                                        );
                                    }
                                    DownloadStatus::Paused => {
                                        ui.label(
                                            RichText::new("Pausada").small().color(TEXT_SECONDARY),
                                        );
                                    }
                                    DownloadStatus::Completed => {
                                        let mb = task.downloaded_bytes as f64 / (1024.0 * 1024.0);
                                        ui.label(
                                            RichText::new(format!("Offline • {mb:.1} MB"))
                                                .small()
                                                .color(GREEN_OK),
                                        );
                                    }
                                    DownloadStatus::Failed(err) => {
                                        ui.label(
                                            RichText::new(format!("Error: {err}"))
                                                .small()
                                                .color(ACCENT_RED),
                                        );
                                    }
                                    DownloadStatus::Cancelled => {
                                        ui.label(
                                            RichText::new("Cancelada")
                                                .small()
                                                .color(TEXT_SECONDARY),
                                        );
                                    }
                                }
                            });

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                match &task.status {
                                    DownloadStatus::Downloading | DownloadStatus::Queued => {
                                        if icons::icon_button(
                                            ui,
                                            icons::Icon::Close,
                                            30.0,
                                            14.0,
                                            BG_ELEVATED,
                                            TEXT_PRIMARY,
                                            15.0,
                                        )
                                        .clicked()
                                        {
                                            let dl = Arc::clone(&self.downloader);
                                            let id = task.track.id.clone();
                                            self.runtime.spawn(async move { dl.cancel(&id).await });
                                        }
                                        if icons::icon_button(
                                            ui,
                                            icons::Icon::Pause,
                                            30.0,
                                            14.0,
                                            BG_ELEVATED,
                                            TEXT_PRIMARY,
                                            15.0,
                                        )
                                        .clicked()
                                        {
                                            let dl = Arc::clone(&self.downloader);
                                            let id = task.track.id.clone();
                                            self.runtime.spawn(async move { dl.pause(&id).await });
                                        }
                                    }
                                    DownloadStatus::Paused | DownloadStatus::Failed(_) => {
                                        if icons::icon_button(
                                            ui,
                                            icons::Icon::Close,
                                            30.0,
                                            14.0,
                                            BG_ELEVATED,
                                            TEXT_PRIMARY,
                                            15.0,
                                        )
                                        .clicked()
                                        {
                                            let dl = Arc::clone(&self.downloader);
                                            let id = task.track.id.clone();
                                            self.runtime.spawn(async move { dl.cancel(&id).await });
                                        }
                                        if icons::icon_button(
                                            ui,
                                            icons::Icon::Play,
                                            30.0,
                                            14.0,
                                            BG_ELEVATED,
                                            TEXT_PRIMARY,
                                            15.0,
                                        )
                                        .clicked()
                                        {
                                            let dl = Arc::clone(&self.downloader);
                                            let id = task.track.id.clone();
                                            self.runtime.spawn(async move { dl.resume(&id).await });
                                        }
                                    }
                                    DownloadStatus::Completed => {
                                        if icons::icon_button(
                                            ui,
                                            icons::Icon::Trash,
                                            30.0,
                                            14.0,
                                            BG_ELEVATED,
                                            TEXT_SECONDARY,
                                            15.0,
                                        )
                                        .clicked()
                                        {
                                            let dl = Arc::clone(&self.downloader);
                                            let id = task.track.id.clone();
                                            self.runtime.spawn(async move {
                                                dl.delete_downloaded(&id).await;
                                            });
                                        }
                                        if icons::icon_button(
                                            ui,
                                            icons::Icon::Play,
                                            30.0,
                                            14.0,
                                            ACCENT_RED,
                                            Color32::WHITE,
                                            15.0,
                                        )
                                        .clicked()
                                        {
                                            if let Some(pos) = completed
                                                .iter()
                                                .position(|t| t.id == task.track.id)
                                            {
                                                self.play_track_in_list(
                                                    completed.clone(),
                                                    pos,
                                                    ctx,
                                                );
                                            }
                                        }
                                    }
                                    DownloadStatus::Cancelled => {}
                                }
                            });
                        });
                    });
            }
        });
    }

    fn render_track_row(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        track: &Track,
        list: &[Track],
        index: usize,
        offline_badge: bool,
    ) {
        let is_current = self
            .queue
            .lock()
            .unwrap()
            .current_track()
            .map(|t| t.id == track.id)
            .unwrap_or(false);
        let fill = if is_current { BG_ELEVATED } else { BG_SURFACE };

        egui::Frame::new()
            .fill(fill)
            .corner_radius(CornerRadius::same(10))
            .stroke(if is_current {
                Stroke::new(1.0_f32, ACCENT_RED)
            } else {
                Stroke::NONE
            })
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let row_start = ui.cursor().min;
                    render_track_thumb(ui, track, 48.0);

                    let text_width = (ui.available_width() - 42.0).max(120.0);
                    let is_downloaded = self.downloads.iter().any(|d| {
                        d.track.id == track.id && d.status == DownloadStatus::Completed
                    });
                    let title_color = if is_current { ACCENT_RED } else { TEXT_PRIMARY };
                    let row_h = 56.0;

                    ui.allocate_ui(Vec2::new(text_width, row_h), |ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.set_max_width(text_width);
                            render_clamped_label(ui, &track.title, 14.5, title_color, text_width, 2);
                            let dur_part = track
                                .duration_secs
                                .filter(|&s| s > 0)
                                .map(|s| format!(" • {}:{:02}", s / 60, s % 60))
                                .unwrap_or_default();
                            let sub = format!("{}{}", track.artist, dur_part);
                            let sub_color = if is_downloaded {
                                GREEN_OK
                            } else {
                                TEXT_SECONDARY
                            };
                            if offline_badge || is_downloaded {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    let (icon_rect, _) =
                                        ui.allocate_exact_size(Vec2::splat(13.0), Sense::hover());
                                    icons::draw_icon(
                                        ui.painter(),
                                        icon_rect,
                                        icons::Icon::Check,
                                        GREEN_OK,
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(sub).small().color(sub_color),
                                        )
                                        .truncate()
                                        .selectable(false),
                                    );
                                });
                            } else {
                                ui.add(
                                    egui::Label::new(RichText::new(sub).small().color(sub_color))
                                        .truncate()
                                        .selectable(false),
                                );
                            }
                        });
                    });

                    let hit_rect = egui::Rect::from_min_size(
                        row_start,
                        Vec2::new(48.0 + 8.0 + text_width, row_h),
                    );
                    let info_resp = ui.interact(
                        hit_rect,
                        ui.id().with(("track_row", &track.id, index)),
                        Sense::click(),
                    );
                    if info_resp.clicked() {
                        self.play_track_in_list(list.to_vec(), index, ctx);
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icons::icon_button(
                            ui,
                            icons::Icon::MoreVert,
                            34.0,
                            17.0,
                            Color32::TRANSPARENT,
                            TEXT_SECONDARY,
                            17.0,
                        )
                        .clicked()
                        {
                            self.action_sheet_track = Some(track.clone());
                            self.action_sheet_context = Some((list.to_vec(), index));
                        }
                    });
                });
            });
    }

    fn render_mini_player(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        track: &Track,
        snap: &AudioSnapshot,
    ) {
        let total = snap
            .duration_secs
            .or(track.duration_secs.map(|d| d as f64))
            .unwrap_or(1.0)
            .max(1.0);
        let progress = (snap.position_secs / total).clamp(0.0, 1.0) as f32;

        let frame_resp = egui::Frame::new()
            .fill(Color32::from_rgb(24, 24, 28))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let row_start = ui.next_widget_position();
                    render_track_thumb(ui, track, 42.0);
                    ui.add_space(6.0);

                    let text_width = (ui.available_width() - 122.0).max(80.0);
                    ui.allocate_ui(Vec2::new(text_width, 42.0), |ui| {
                        ui.vertical(|ui| {
                            ui.set_max_width(text_width);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&track.title)
                                        .size(14.0)
                                        .strong()
                                        .color(TEXT_PRIMARY),
                                )
                                .truncate()
                                .selectable(false),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&track.artist)
                                        .size(12.0)
                                        .color(TEXT_SECONDARY),
                                )
                                .truncate()
                                .selectable(false),
                            );
                        });
                    });

                    let tap_rect = egui::Rect::from_min_size(
                        row_start,
                        Vec2::new(42.0 + 6.0 + text_width, 42.0),
                    );
                    let open_resp = ui.interact(
                        tap_rect,
                        ui.make_persistent_id(("mini_open", &track.id)),
                        Sense::click_and_drag(),
                    );
                    if open_resp.clicked() {
                        self.now_playing_open = true;
                    }
                    if open_resp.drag_started() {
                        self.cover_drag_accum = Vec2::ZERO;
                    }
                    if open_resp.dragged() {
                        self.cover_drag_accum += open_resp.drag_delta();
                    }
                    if open_resp.drag_stopped() {
                        let d = self.cover_drag_accum;
                        self.cover_drag_accum = Vec2::ZERO;
                        if d.y < -36.0 && d.y.abs() > d.x.abs() {
                            self.now_playing_open = true;
                        } else if d.x < -55.0
                            && self.queue.lock().unwrap().next_track(true).is_some()
                        {
                            self.play_current_queue_track(ctx);
                        } else if d.x > 55.0 && self.queue.lock().unwrap().prev_track().is_some() {
                            self.play_current_queue_track(ctx);
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icons::icon_button(
                            ui,
                            icons::Icon::SkipNext,
                            34.0,
                            17.0,
                            Color32::TRANSPARENT,
                            TEXT_PRIMARY,
                            17.0,
                        )
                        .clicked()
                            && self.queue.lock().unwrap().next_track(true).is_some()
                        {
                            self.play_current_queue_track(ctx);
                        }
                        let play_icon = match snap.status {
                            PlaybackStatus::Playing => icons::Icon::Pause,
                            _ => icons::Icon::Play,
                        };
                        if icons::icon_button(
                            ui,
                            play_icon,
                            36.0,
                            18.0,
                            Color32::TRANSPARENT,
                            TEXT_PRIMARY,
                            19.0,
                        )
                        .clicked()
                        {
                            match snap.status {
                                PlaybackStatus::Playing => self.audio.pause(),
                                PlaybackStatus::Paused => self.audio.resume(),
                                _ => self.play_current_queue_track(ctx),
                            }
                        }
                        if icons::icon_button(
                            ui,
                            icons::Icon::SkipPrev,
                            34.0,
                            17.0,
                            Color32::TRANSPARENT,
                            TEXT_PRIMARY,
                            17.0,
                        )
                        .clicked()
                            && self.queue.lock().unwrap().prev_track().is_some()
                        {
                            self.play_current_queue_track(ctx);
                        }
                    });
                });
            });

        let track_bg = egui::Rect::from_min_size(
            frame_resp.response.rect.left_top(),
            Vec2::new(frame_resp.response.rect.width(), 2.5),
        );
        ui.painter()
            .rect_filled(track_bg, CornerRadius::same(1), Color32::from_white_alpha(36));
        let bar_rect = egui::Rect::from_min_size(
            frame_resp.response.rect.left_top(),
            Vec2::new(frame_resp.response.rect.width() * progress, 2.5),
        );
        ui.painter()
            .rect_filled(bar_rect, CornerRadius::same(1), Color32::WHITE);
    }

    fn render_full_player(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        snap: &AudioSnapshot,
    ) {
        let Some(track) = self.queue.lock().unwrap().current_track().cloned() else {
            self.now_playing_open = false;
            return;
        };

        let resolved_cover = track_thumb_url(track.thumbnail_url.as_deref(), &track.id);
        if let Some(url) = resolved_cover.as_deref() {
            self.ensure_cover_color(Some(url), ctx);
        }
        if self.related_track_id.as_deref() != Some(&track.id) && !self.related_loading {
            self.load_related(track.id.clone(), ctx);
        }

        let ambient = resolved_cover
            .as_deref()
            .and_then(|u| self.cover_colors.get(u).copied())
            .unwrap_or(Color32::from_rgb(56, 40, 26));

        // Ambient cover backdrop gradient fading into BG_PRIMARY at bottom.
        let bg_rect = ui.available_rect_before_wrap();
        paint_ambient_gradient(ui.painter(), bg_rect, ambient);

        let pad_x = 16.0;
        #[cfg(target_os = "android")]
        let (safe_top, safe_bottom) = (36.0_f32, 68.0_f32);
        #[cfg(not(target_os = "android"))]
        let (safe_top, safe_bottom) = (10.0_f32, 12.0_f32);
        let inner_rect = egui::Rect::from_min_max(
            egui::pos2(bg_rect.left() + pad_x, bg_rect.top() + safe_top),
            egui::pos2(bg_rect.right() - pad_x, bg_rect.bottom() - safe_bottom),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(inner_rect), |ui| {
            // Top bar: v chevron | optional sleep badge | more ⋮ button
            ui.horizontal(|ui| {
                if icons::icon_button(
                    ui,
                    icons::Icon::ChevronDown,
                    38.0,
                    19.0,
                    Color32::TRANSPARENT,
                    Color32::WHITE,
                    20.0,
                )
                .clicked()
                {
                    self.now_playing_open = false;
                }

                let active_until = *self.sleep_timer_until.lock().unwrap();
                let timer_text = if let Some(until) = active_until {
                    let rem = until.saturating_duration_since(Instant::now()).as_secs();
                    Some(format!("{}:{:02}", rem / 60, rem % 60))
                } else if self
                    .sleep_after_track
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    Some("Al terminar".to_string())
                } else {
                    None
                };

                let has_timer = timer_text.is_some();
                if let Some(txt) = timer_text {
                    ui.add_space(6.0);
                    let timer_resp = egui::Frame::new()
                        .fill(Color32::from_white_alpha(28))
                        .corner_radius(CornerRadius::same(14))
                        .inner_margin(egui::Margin::symmetric(10, 4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (r, _) =
                                    ui.allocate_exact_size(Vec2::splat(13.0), Sense::hover());
                                icons::draw_icon(ui.painter(), r, icons::Icon::Moon, ACCENT_RED);
                                ui.label(RichText::new(txt).size(12.0).color(Color32::WHITE));
                            });
                        })
                        .response
                        .interact(Sense::click());
                    if timer_resp.clicked() {
                        self.show_sleep_timer_sheet = true;
                    }
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icons::icon_button(
                        ui,
                        icons::Icon::MoreVert,
                        38.0,
                        19.0,
                        Color32::TRANSPARENT,
                        Color32::WHITE,
                        20.0,
                    )
                    .clicked()
                    {
                        self.action_sheet_track = Some(track.clone());
                    }
                    if !has_timer
                        && icons::icon_button(
                            ui,
                            icons::Icon::Moon,
                            34.0,
                            17.0,
                            Color32::TRANSPARENT,
                            Color32::from_white_alpha(180),
                            17.0,
                        )
                        .clicked()
                    {
                        self.show_sleep_timer_sheet = true;
                    }
                });
            });

            let avail_w = ui.available_width();
            let bottom_tabs_h = 52.0;
            let body_h = (ui.available_height() - bottom_tabs_h).max(200.0);

            ui.allocate_ui(Vec2::new(avail_w, body_h), |ui| {
                ui.set_min_height(body_h);
                if self.show_queue || self.show_lyrics || self.show_related {
                    // Compact header row in drawers so current track stays visible + controllable.
                    ui.horizontal(|ui| {
                        render_track_thumb(ui, &track, 42.0);
                        let tw = (ui.available_width() - 96.0).max(80.0);
                        ui.allocate_ui(Vec2::new(tw, 42.0), |ui| {
                            ui.vertical(|ui| {
                                ui.set_max_width(tw);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&track.title)
                                            .size(14.5)
                                            .strong()
                                            .color(Color32::WHITE),
                                    )
                                    .truncate(),
                                );
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&track.artist)
                                            .size(12.5)
                                            .color(Color32::from_white_alpha(185)),
                                    )
                                    .truncate(),
                                );
                            });
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let play_icon = if snap.status == PlaybackStatus::Playing {
                                icons::Icon::Pause
                            } else {
                                icons::Icon::Play
                            };
                            if icons::icon_button(
                                ui,
                                play_icon,
                                38.0,
                                19.0,
                                Color32::from_white_alpha(28),
                                Color32::WHITE,
                                18.0,
                            )
                            .clicked()
                            {
                                match snap.status {
                                    PlaybackStatus::Playing => self.audio.pause(),
                                    PlaybackStatus::Paused => self.audio.resume(),
                                    _ => self.play_current_queue_track(ctx),
                                }
                            }
                        });
                    });
                    ui.add_space(10.0);

                    if self.show_queue {
                        let (queue_len, ordered, current_idx) = {
                            let q = self.queue.lock().unwrap();
                            let ordered: Vec<(usize, Track)> = q
                                .ordered_tracks()
                                .into_iter()
                                .map(|(i, t)| (i, t.clone()))
                                .collect();
                            (q.len(), ordered, q.current_index())
                        };
                        ui.horizontal(|ui| {
                            let hdr_w = (ui.available_width() - 168.0).max(120.0);
                            ui.allocate_ui(Vec2::new(hdr_w, 30.0), |ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(format!("A continuación ({queue_len})"))
                                                .size(15.0)
                                                .strong()
                                                .color(Color32::WHITE),
                                        )
                                        .truncate()
                                        .selectable(false),
                                    );
                                });
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Radio,
                                    "Radio",
                                    Color32::from_white_alpha(24),
                                    Color32::WHITE,
                                )
                                .clicked()
                                {
                                    self.start_radio(track.id.clone(), ctx);
                                }
                                ui.add_space(6.0);
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::PlaylistAdd,
                                    "Guardar",
                                    Color32::from_white_alpha(24),
                                    Color32::WHITE,
                                )
                                .clicked()
                                {
                                    let q_tracks: Vec<Track> = ordered
                                        .iter()
                                        .map(|(_, qt)| qt.clone())
                                        .collect();
                                    let pl = self
                                        .store
                                        .create_playlist(format!("Mix • {}", track.title));
                                    let count = q_tracks.len();
                                    for qt in q_tracks {
                                        self.store.add_to_playlist(&pl.id, qt);
                                    }
                                    self.set_banner(format!(
                                        "Playlist guardada ({count} canciones)"
                                    ));
                                }
                            });
                        });
                        ui.add_space(6.0);
                        let total_ordered = ordered.len();
                        egui::ScrollArea::vertical()
                            .id_salt("full_queue_scroll")
                            .show(ui, |ui| {
                                for (order_pos, (idx, t)) in ordered.into_iter().enumerate() {
                                    let is_curr = current_idx == Some(idx);
                                    let drag_id = ui.make_persistent_id(("q_drag", &t.id, idx));
                                    let is_dragging =
                                        ui.ctx().is_being_dragged(drag_id);
                                    egui::Frame::new()
                                        .fill(if is_dragging {
                                            Color32::from_white_alpha(48)
                                        } else if is_curr {
                                            Color32::from_white_alpha(26)
                                        } else {
                                            Color32::TRANSPARENT
                                        })
                                        .corner_radius(CornerRadius::same(8))
                                        .inner_margin(6.0)
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                let (grip_rect, grip_resp) =
                                                    ui.allocate_exact_size(
                                                        Vec2::new(20.0, 48.0),
                                                        Sense::drag(),
                                                    );
                                                let grip_color = if is_dragging {
                                                    Color32::WHITE
                                                } else {
                                                    Color32::from_white_alpha(120)
                                                };
                                                let cx = grip_rect.center().x;
                                                let cy = grip_rect.center().y;
                                                for dy in [-5.0_f32, 0.0, 5.0] {
                                                    ui.painter().rect_filled(
                                                        egui::Rect::from_center_size(
                                                            egui::pos2(cx, cy + dy),
                                                            Vec2::new(12.0, 1.8),
                                                        ),
                                                        CornerRadius::same(1),
                                                        grip_color,
                                                    );
                                                }
                                                let step = consume_vertical_drag_step(
                                                    ui, drag_id, &grip_resp, 38.0, false,
                                                );
                                                if step != 0 {
                                                    self.queue
                                                        .lock()
                                                        .unwrap()
                                                        .move_in_order(order_pos, step as isize);
                                                }

                                                let row_start = ui.next_widget_position();
                                                render_track_thumb(ui, &t, 44.0);
                                                ui.add_space(6.0);
                                                let right_reserve = if is_curr { 66.0 } else { 98.0 };
                                                let tw = (ui.available_width() - right_reserve - 6.0).max(70.0);
                                                ui.allocate_ui(Vec2::new(tw, 48.0), |ui| {
                                                    ui.vertical(|ui| {
                                                        ui.spacing_mut().item_spacing.y = 1.5;
                                                        ui.set_max_width(tw);
                                                        render_clamped_label(
                                                            ui,
                                                            &t.title,
                                                            13.5,
                                                            if is_curr {
                                                                Color32::WHITE
                                                            } else {
                                                                TEXT_PRIMARY
                                                            },
                                                            tw,
                                                            2,
                                                        );
                                                        let q_sub = match t.duration_secs.filter(|&s| s > 0) {
                                                            Some(s) => format!(
                                                                "{} • {}:{:02}",
                                                                t.artist,
                                                                s / 60,
                                                                s % 60
                                                            ),
                                                            None => t.artist.clone(),
                                                        };
                                                        ui.add(
                                                            egui::Label::new(
                                                                RichText::new(q_sub)
                                                                    .size(12.0)
                                                                    .color(Color32::from_white_alpha(175)),
                                                            )
                                                            .truncate()
                                                            .selectable(false),
                                                        );
                                                    });
                                                });
                                                let hit_rect = egui::Rect::from_min_size(
                                                    row_start,
                                                    Vec2::new(44.0 + 6.0 + tw, 48.0),
                                                );
                                                if ui
                                                    .interact(
                                                        hit_rect,
                                                        ui.make_persistent_id(("q_row", order_pos, &t.id)),
                                                        Sense::click(),
                                                    )
                                                    .clicked()
                                                {
                                                    self.queue.lock().unwrap().jump_to(idx);
                                                    self.play_current_queue_track(ctx);
                                                }
                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        ui.spacing_mut().item_spacing.x = 4.0;
                                                        if !is_curr
                                                            && icons::icon_button(
                                                                ui,
                                                                icons::Icon::Close,
                                                                24.0,
                                                                12.0,
                                                                Color32::TRANSPARENT,
                                                                TEXT_SECONDARY,
                                                                12.0,
                                                            )
                                                            .clicked()
                                                        {
                                                            self.queue.lock().unwrap().remove_at(idx);
                                                        }
                                                        if order_pos + 1 < total_ordered
                                                            && icons::icon_button(
                                                                ui,
                                                                icons::Icon::ChevronDown,
                                                                24.0,
                                                                12.0,
                                                                Color32::from_white_alpha(20),
                                                                Color32::WHITE,
                                                                12.0,
                                                            )
                                                            .clicked()
                                                        {
                                                            self.queue
                                                                .lock()
                                                                .unwrap()
                                                                .move_in_order(order_pos, 1);
                                                        }
                                                        if order_pos > 0
                                                            && icons::icon_button(
                                                                ui,
                                                                icons::Icon::ChevronUp,
                                                                24.0,
                                                                12.0,
                                                                Color32::from_white_alpha(20),
                                                                Color32::WHITE,
                                                                12.0,
                                                            )
                                                            .clicked()
                                                        {
                                                            self.queue
                                                                .lock()
                                                                .unwrap()
                                                                .move_in_order(order_pos, -1);
                                                        }
                                                    },
                                                );
                                            });
                                        });
                                }
                            });
                    } else if self.show_lyrics {
                        ui.horizontal(|ui| {
                            let badge = match &self.lyrics_data {
                                Some(d) if !d.synced.is_empty() => "Letra sincronizada",
                                _ => "Letra",
                            };
                            let hdr_w = (ui.available_width() - 134.0).max(110.0);
                            ui.allocate_ui(Vec2::new(hdr_w, 30.0), |ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(badge)
                                                .size(15.5)
                                                .strong()
                                                .color(Color32::WHITE),
                                        )
                                        .truncate()
                                        .selectable(false),
                                    );
                                });
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icons::icon_button(
                                    ui,
                                    icons::Icon::Refresh,
                                    32.0,
                                    16.0,
                                    Color32::from_white_alpha(24),
                                    Color32::WHITE,
                                    16.0,
                                )
                                .clicked()
                                {
                                    self.load_lyrics(track.clone(), ctx);
                                }
                                if let Some(d) = &self.lyrics_data {
                                    let has_lyrics = !d.synced.is_empty()
                                        || (!d.plain.is_empty() && !d.plain.starts_with("No se pudo"));
                                    if has_lyrics {
                                        ui.add_space(6.0);
                                        if icons::icon_pill_button(
                                            ui,
                                            icons::Icon::Share,
                                            "Copiar",
                                            Color32::from_white_alpha(24),
                                            Color32::WHITE,
                                        )
                                        .clicked()
                                        {
                                            let body = if !d.synced.is_empty() {
                                                d.synced
                                                    .iter()
                                                    .map(|l| l.text.as_str())
                                                    .filter(|s| !s.is_empty())
                                                    .collect::<Vec<_>>()
                                                    .join("\n")
                                            } else {
                                                d.plain.clone()
                                            };
                                            ctx.copy_text(format!(
                                                "{} — {}\n\n{}",
                                                track.title, track.artist, body
                                            ));
                                            self.set_banner("Letra copiada al portapapeles");
                                        }
                                    }
                                }
                            });
                        });
                        ui.add_space(6.0);
                        if self.lyrics_loading {
                            ui.vertical_centered(|ui| {
                                ui.add_space(40.0);
                                ui.spinner();
                            });
                        } else if let Some(data) = self.lyrics_data.clone() {
                            if !data.synced.is_empty() {
                                let pos = snap.position_secs;
                                let active_idx = data
                                    .synced
                                    .iter()
                                    .rposition(|l| l.time_secs <= pos + 0.25);
                                egui::ScrollArea::vertical()
                                    .id_salt("synced_lyrics_scroll")
                                    .show(ui, |ui| {
                                        ui.add_space(6.0);
                                        for (i, line) in data.synced.iter().enumerate() {
                                            let is_active = active_idx == Some(i);
                                            let text = if line.text.is_empty() {
                                                "(Instrumental)"
                                            } else {
                                                line.text.as_str()
                                            };
                                            let ts_str = format_seconds(
                                                line.time_secs.max(0.0).round() as u64,
                                            );
                                            let row_bg = if is_active {
                                                Color32::from_white_alpha(30)
                                            } else {
                                                Color32::from_white_alpha(8)
                                            };
                                            let mut row_clicked = false;
                                            let frame_resp = egui::Frame::new()
                                                .fill(row_bg)
                                                .corner_radius(CornerRadius::same(10))
                                                .inner_margin(egui::Margin::symmetric(10, 7))
                                                .show(ui, |ui| {
                                                    ui.set_min_width(ui.available_width());
                                                    ui.horizontal(|ui| {
                                                        ui.spacing_mut().item_spacing.x = 8.0;
                                                        let ts_resp = ui.add(
                                                            egui::Label::new(
                                                                RichText::new(ts_str)
                                                                    .size(11.5)
                                                                    .monospace()
                                                                    .color(if is_active {
                                                                        ACCENT_RED
                                                                    } else {
                                                                        Color32::from_white_alpha(
                                                                            110,
                                                                        )
                                                                    }),
                                                            )
                                                            .selectable(false)
                                                            .sense(Sense::click()),
                                                        );
                                                        let text_resp = ui.add(
                                                            egui::Label::new(
                                                                RichText::new(text)
                                                                    .size(if is_active {
                                                                        18.5
                                                                    } else {
                                                                        15.5
                                                                    })
                                                                    .strong()
                                                                    .color(if is_active {
                                                                        Color32::WHITE
                                                                    } else {
                                                                        Color32::from_white_alpha(
                                                                            140,
                                                                        )
                                                                    }),
                                                            )
                                                            .wrap()
                                                            .selectable(false)
                                                            .sense(Sense::click()),
                                                        );
                                                        if ts_resp.clicked() || text_resp.clicked()
                                                        {
                                                            row_clicked = true;
                                                        }
                                                    });
                                                });
                                            if frame_resp.response.interact(Sense::click()).clicked()
                                            {
                                                row_clicked = true;
                                            }
                                            if is_active
                                                && self.lyrics_last_scrolled_idx != active_idx
                                            {
                                                self.lyrics_last_scrolled_idx = active_idx;
                                                frame_resp.response.scroll_to_me(Some(Align::Center));
                                            }
                                            if row_clicked {
                                                self.lyrics_last_scrolled_idx = Some(i);
                                                self.audio.seek(Duration::from_secs_f64(
                                                    line.time_secs,
                                                ));
                                            }
                                            ui.add_space(6.0);
                                        }
                                        if !data.source.is_empty() {
                                            ui.add_space(12.0);
                                            ui.label(
                                                RichText::new(format!("Fuente: {}", data.source))
                                                    .small()
                                                    .color(Color32::from_white_alpha(140)),
                                            );
                                        }
                                    });
                            } else if !data.plain.is_empty() {
                                egui::ScrollArea::vertical()
                                    .id_salt("plain_lyrics_scroll")
                                    .show(ui, |ui| {
                                        ui.add_space(6.0);
                                        ui.label(
                                            RichText::new(&data.plain)
                                                .size(16.0)
                                                .color(Color32::from_white_alpha(230)),
                                        );
                                        if data.source.is_empty()
                                            && data.plain.starts_with("No se pudo")
                                        {
                                            ui.add_space(12.0);
                                            if icons::icon_pill_button(
                                                ui,
                                                icons::Icon::Refresh,
                                                "Reintentar",
                                                Color32::from_white_alpha(24),
                                                Color32::WHITE,
                                            )
                                            .clicked()
                                            {
                                                self.load_lyrics(track.clone(), ctx);
                                            }
                                        }
                                    });
                            } else {
                                ui.add_space(24.0);
                                ui.label(
                                    RichText::new("No hay letra disponible para esta canción.")
                                        .color(Color32::from_white_alpha(175)),
                                );
                                ui.add_space(10.0);
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Refresh,
                                    "Reintentar",
                                    Color32::from_white_alpha(24),
                                    Color32::WHITE,
                                )
                                .clicked()
                                {
                                    self.load_lyrics(track.clone(), ctx);
                                }
                            }
                        } else {
                            ui.add_space(24.0);
                            ui.label(
                                RichText::new("No hay letra disponible para esta canción.")
                                    .color(Color32::from_white_alpha(175)),
                            );
                            ui.add_space(10.0);
                            if icons::icon_pill_button(
                                ui,
                                icons::Icon::Refresh,
                                "Reintentar",
                                Color32::from_white_alpha(24),
                                Color32::WHITE,
                            )
                            .clicked()
                            {
                                self.load_lyrics(track.clone(), ctx);
                            }
                        }
                    } else if self.show_related {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Similares y relacionados")
                                    .size(16.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Radio,
                                    "Radio",
                                    Color32::from_white_alpha(24),
                                    Color32::WHITE,
                                )
                                .clicked()
                                {
                                    self.start_radio(track.id.clone(), ctx);
                                }
                            });
                        });
                        ui.add_space(6.0);
                        if self.related_loading {
                            ui.vertical_centered(|ui| {
                                ui.add_space(40.0);
                                ui.spinner();
                            });
                        } else if self.related_tracks.is_empty() {
                            ui.add_space(24.0);
                            ui.label(
                                RichText::new("Sin recomendaciones relacionadas aún.")
                                    .color(Color32::from_white_alpha(175)),
                            );
                            ui.add_space(10.0);
                            if icons::icon_pill_button(
                                ui,
                                icons::Icon::Refresh,
                                "Reintentar",
                                Color32::from_white_alpha(24),
                                Color32::WHITE,
                            )
                            .clicked()
                            {
                                self.load_related(track.id.clone(), ctx);
                            }
                        } else {
                            let tracks = self.related_tracks.clone();
                            egui::ScrollArea::vertical()
                                .id_salt("related_tracks_scroll")
                                .show(ui, |ui| {
                                    for (idx, t) in tracks.iter().enumerate() {
                                        self.render_track_row(ui, ctx, t, &tracks, idx, false);
                                        ui.add_space(4.0);
                                    }
                                });
                        }
                    }
                } else {
                    // Main Cover Now Playing view
                    ui.add_space(4.0);
                    let art_size = (avail_w - 28.0).min(body_h * 0.43).clamp(155.0, 290.0);
                    ui.vertical_centered(|ui| {
                        let (art_rect, art_resp) =
                            ui.allocate_exact_size(Vec2::splat(art_size), Sense::click_and_drag());
                        ui.painter().rect_filled(
                            art_rect,
                            CornerRadius::same(14),
                            Color32::from_white_alpha(22),
                        );
                        let note_rect = egui::Rect::from_center_size(
                            art_rect.center(),
                            Vec2::splat((art_size * 0.34).clamp(36.0, 88.0)),
                        );
                        icons::draw_icon(
                            ui.painter(),
                            note_rect,
                            icons::Icon::MusicNote,
                            Color32::from_white_alpha(110),
                        );
                        if let Some(u) = resolved_cover.as_deref() {
                            let art_uri = tracked_thumb_uri(u, 544);
                            paint_ready_image(
                                ui,
                                art_rect,
                                art_uri,
                                Vec2::splat(art_size),
                                CornerRadius::same(14),
                            );
                        }
                        if art_resp.drag_started() {
                            self.cover_drag_accum = Vec2::ZERO;
                        }
                        if art_resp.dragged() {
                            self.cover_drag_accum += art_resp.drag_delta();
                        }
                        if art_resp.drag_stopped() {
                            let d = self.cover_drag_accum;
                            self.cover_drag_accum = Vec2::ZERO;
                            if d.y > 75.0 && d.y.abs() > d.x.abs() {
                                self.now_playing_open = false;
                            } else if d.x < -55.0
                                && self.queue.lock().unwrap().next_track(true).is_some()
                            {
                                self.play_current_queue_track(ctx);
                            } else if d.x > 55.0 && self.queue.lock().unwrap().prev_track().is_some()
                            {
                                self.play_current_queue_track(ctx);
                            }
                        } else if art_resp.double_clicked() {
                            if let Some(pos) = art_resp.interact_pointer_pos() {
                                let total_dur = snap
                                    .duration_secs
                                    .or(track.duration_secs.map(|d| d as f64))
                                    .unwrap_or(0.0);
                                let delta = if pos.x < art_rect.center().x {
                                    -10.0
                                } else {
                                    10.0
                                };
                                let target = (snap.position_secs + delta)
                                    .clamp(0.0, total_dur.max(snap.position_secs + 10.0));
                                self.audio.seek(Duration::from_secs_f64(target));
                                self.last_media_sync = None;
                                self.set_banner(if delta < 0.0 {
                                    "-10s"
                                } else {
                                    "+10s"
                                });
                            }
                        }
                    });

                    ui.add_space(16.0);
                    // Left-aligned Title > and Artist constrained to avail_w
                    let title_w = (avail_w - 22.0).max(140.0);
                    let mut title_clicked = false;
                    ui.allocate_ui(Vec2::new(avail_w, 0.0), |ui| {
                        ui.horizontal(|ui| {
                            ui.allocate_ui(Vec2::new(title_w, 0.0), |ui| {
                                let job = clamped_layout_job(
                                    &track.title,
                                    21.0,
                                    Color32::WHITE,
                                    title_w,
                                    2,
                                );
                                if ui.add(egui::Label::new(job).sense(Sense::click())).clicked() {
                                    title_clicked = true;
                                }
                            });
                            if ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(" ›")
                                            .size(21.0)
                                            .strong()
                                            .color(Color32::from_white_alpha(210)),
                                    )
                                    .sense(Sense::click()),
                                )
                                .clicked()
                            {
                                title_clicked = true;
                            }
                        });
                    });
                    if title_clicked {
                        if let Some(album_id) = track.album_id.clone() {
                            self.now_playing_open = false;
                            self.open_album(album_id, ctx);
                        } else {
                            self.action_sheet_track = Some(track.clone());
                            let q = self.queue.lock().unwrap();
                            self.action_sheet_context =
                                q.current_index().map(|idx| (q.tracks().to_vec(), idx));
                        }
                    }
                    ui.add_space(2.0);
                    if let Some(artist_id) = track.artist_id.clone() {
                        if ui
                            .add(
                                egui::Label::new(
                                    RichText::new(&track.artist)
                                        .size(15.0)
                                        .color(Color32::from_white_alpha(190)),
                                )
                                .sense(Sense::click()),
                            )
                            .clicked()
                        {
                            self.now_playing_open = false;
                            self.open_artist(artist_id, ctx);
                        }
                    } else {
                        ui.label(
                            RichText::new(&track.artist)
                                .size(15.0)
                                .color(Color32::from_white_alpha(190)),
                        );
                    }

                    let is_offline = self
                        .downloads
                        .iter()
                        .any(|d| d.track.id == track.id && d.status == DownloadStatus::Completed);
                    match &snap.status {
                        PlaybackStatus::Buffering | PlaybackStatus::Loading => {
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new("Cargando audio...")
                                    .size(12.0)
                                    .color(Color32::from_white_alpha(210)),
                            );
                        }
                        PlaybackStatus::Error(err) => {
                            ui.add_space(2.0);
                            ui.label(RichText::new(err).size(12.0).color(ACCENT_RED));
                        }
                        _ => {
                            let badge = match (is_offline, snap.codec_label.as_deref()) {
                                (true, Some(c)) => Some((format!("Offline • {c}"), GREEN_OK)),
                                (true, None) => Some(("Offline".to_string(), GREEN_OK)),
                                (false, Some(c)) => {
                                    Some((c.to_string(), Color32::from_white_alpha(155)))
                                }
                                (false, None) => None,
                            };
                            if let Some((txt, col)) = badge {
                                ui.add_space(2.0);
                                ui.label(RichText::new(txt).size(11.5).color(col));
                            }
                        }
                    }

                    ui.add_space(12.0);
                    // Scrollable Action Pills row matching YTM: ThumbUp|ThumbDown, Letra, Guardar, Compartir, Descargar, Radio
                    let pill_bg = Color32::from_white_alpha(26);
                    ui.allocate_ui(Vec2::new(avail_w, 38.0), |ui| {
                        egui::ScrollArea::horizontal()
                            .id_salt("np_action_pills")
                            .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let liked = self.store.is_liked(&track.id);
                                let disliked = self.store.is_disliked(&track.id);
                                egui::Frame::new()
                                    .fill(pill_bg)
                                    .corner_radius(CornerRadius::same(18))
                                    .inner_margin(egui::Margin::symmetric(8, 5))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 3.0;
                                            let like_icon_resp = icons::icon_button(
                                                ui,
                                                icons::Icon::ThumbUp,
                                                26.0,
                                                18.0,
                                                Color32::TRANSPARENT,
                                                if liked { ACCENT_RED } else { Color32::WHITE },
                                                16.0,
                                            );
                                            let like_label_resp = ui.add(
                                                egui::Label::new(
                                                    RichText::new("Me gusta")
                                                        .size(12.0)
                                                        .color(if liked {
                                                            ACCENT_RED
                                                        } else {
                                                            Color32::WHITE
                                                        }),
                                                )
                                                .selectable(false)
                                                .sense(Sense::click()),
                                            );
                                            if like_icon_resp.clicked()
                                                || like_label_resp.clicked()
                                            {
                                                self.store.toggle_like(track.clone());
                                            }
                                            ui.label(
                                                RichText::new("|")
                                                    .size(12.5)
                                                    .color(Color32::from_white_alpha(70)),
                                            );
                                            if icons::icon_button(
                                                ui,
                                                icons::Icon::ThumbDown,
                                                26.0,
                                                18.0,
                                                Color32::TRANSPARENT,
                                                if disliked { ACCENT_RED } else { Color32::WHITE },
                                                16.0,
                                            )
                                            .clicked()
                                            {
                                                let now_disliked =
                                                    self.store.toggle_dislike(&track.id);
                                                if now_disliked
                                                    && self
                                                        .queue
                                                        .lock()
                                                        .unwrap()
                                                        .next_track(true)
                                                        .is_some()
                                                {
                                                    self.play_current_queue_track(ctx);
                                                }
                                            }
                                        });
                                    });

                                ui.add_space(6.0);
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Lyrics,
                                    "Letra",
                                    pill_bg,
                                    Color32::WHITE,
                                )
                                .clicked()
                                {
                                    self.show_lyrics = true;
                                    self.show_queue = false;
                                    self.show_related = false;
                                    self.lyrics_last_scrolled_idx = None;
                                    if self.lyrics_track_id.as_deref() != Some(&track.id) {
                                        self.load_lyrics(track.clone(), ctx);
                                    }
                                }

                                ui.add_space(6.0);
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::PlaylistAdd,
                                    "Guardar",
                                    pill_bg,
                                    Color32::WHITE,
                                )
                                .clicked()
                                {
                                    self.playlist_picker_track = Some(track.clone());
                                }

                                ui.add_space(6.0);
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Share,
                                    "Compartir",
                                    pill_bg,
                                    Color32::WHITE,
                                )
                                .clicked()
                                {
                                    let url =
                                        format!("https://music.youtube.com/watch?v={}", track.id);
                                    crate::android_media::share_text(
                                        &track.title,
                                        &format!("{} - {}\n{}", track.title, track.artist, url),
                                    );
                                }

                                ui.add_space(6.0);
                                let dl_icon = if is_offline {
                                    icons::Icon::Check
                                } else {
                                    icons::Icon::Download
                                };
                                let dl_label = if is_offline { "Offline" } else { "Descargar" };
                                if icons::icon_pill_button(
                                    ui,
                                    dl_icon,
                                    dl_label,
                                    pill_bg,
                                    if is_offline { GREEN_OK } else { Color32::WHITE },
                                )
                                .clicked()
                                    && !is_offline
                                {
                                    self.enqueue_download(track.clone(), ctx);
                                }

                                ui.add_space(6.0);
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Radio,
                                    "Radio",
                                    pill_bg,
                                    Color32::WHITE,
                                )
                                .clicked()
                                {
                                    self.start_radio(track.id.clone(), ctx);
                                }

                                ui.add_space(6.0);
                                let spd = snap.speed;
                                let spd_text = if (spd - spd.round()).abs() < 0.01 {
                                    format!("{:.0}x", spd.round())
                                } else if ((spd * 10.0) - (spd * 10.0).round()).abs() < 0.01 {
                                    format!("{spd:.1}x")
                                } else {
                                    format!("{spd:.2}x")
                                };
                                let spd_active = (spd - 1.0).abs() > 0.02;
                                let spd_btn = egui::Button::new(
                                    RichText::new(spd_text)
                                        .size(12.5)
                                        .strong()
                                        .color(if spd_active {
                                            Color32::WHITE
                                        } else {
                                            Color32::from_white_alpha(230)
                                        }),
                                )
                                .fill(if spd_active { ACCENT_RED } else { pill_bg })
                                .corner_radius(CornerRadius::same(18))
                                .min_size(Vec2::new(48.0, 32.0));
                                if ui.add(spd_btn).clicked() {
                                    let presets = [0.75_f32, 1.0, 1.25, 1.5, 2.0];
                                    let cur_idx = presets
                                        .iter()
                                        .position(|&p| (p - spd).abs() < 0.05)
                                        .unwrap_or(1);
                                    let next = presets[(cur_idx + 1) % presets.len()];
                                    self.audio.set_speed(next);
                                    self.store.data.config.playback_speed = next;
                                    let _ = self.store.save();
                                    self.set_banner(format!("Velocidad: {next}x"));
                                }
                            });
                        });
                    });

                    ui.add_space(12.0);
                    let total = snap
                        .duration_secs
                        .or(track.duration_secs.map(|d| d as f64))
                        .unwrap_or(1.0)
                        .max(1.0);
                    let pos = self.seeking_secs.unwrap_or(snap.position_secs).min(total);
                    let slider_w = avail_w.max(160.0);
                    let (bar_rect, bar_resp) = ui.allocate_exact_size(
                        Vec2::new(slider_w, 24.0),
                        Sense::click_and_drag(),
                    );
                    let frac_from_x = |x: f32| -> f64 {
                        ((x - bar_rect.left()) / bar_rect.width().max(1.0)).clamp(0.0, 1.0)
                            as f64
                            * total
                    };
                    let mut display_pos = pos;
                    if bar_resp.dragged() {
                        if let Some(pointer) = bar_resp.interact_pointer_pos() {
                            let target = frac_from_x(pointer.x);
                            self.seeking_secs = Some(target);
                            display_pos = target;
                        }
                    }
                    if bar_resp.drag_stopped() {
                        let target = self
                            .seeking_secs
                            .or_else(|| bar_resp.interact_pointer_pos().map(|p| frac_from_x(p.x)))
                            .unwrap_or(display_pos);
                        self.audio.seek(Duration::from_secs_f64(target));
                        self.seeking_secs = None;
                        self.last_media_sync = None;
                        display_pos = target;
                    } else if bar_resp.clicked() {
                        if let Some(pointer) = bar_resp.interact_pointer_pos() {
                            let target = frac_from_x(pointer.x);
                            self.audio.seek(Duration::from_secs_f64(target));
                            self.seeking_secs = None;
                            self.last_media_sync = None;
                            display_pos = target;
                        }
                    }
                    if ui.is_rect_visible(bar_rect) {
                        let frac = (display_pos / total).clamp(0.0, 1.0) as f32;
                        let cy = bar_rect.center().y;
                        let track_rect = egui::Rect::from_min_max(
                            egui::pos2(bar_rect.left(), cy - 1.5),
                            egui::pos2(bar_rect.right(), cy + 1.5),
                        );
                        let painter = ui.painter();
                        painter.rect_filled(track_rect, 1.5, Color32::from_white_alpha(64));
                        let thumb_x = track_rect.left() + track_rect.width() * frac;
                        let active_rect = egui::Rect::from_min_max(
                            track_rect.min,
                            egui::pos2(thumb_x, track_rect.bottom()),
                        );
                        painter.rect_filled(active_rect, 1.5, Color32::WHITE);
                        if self.store.data.config.sponsorblock_enabled
                            && self.sponsor_track_id.as_deref() == Some(&track.id)
                            && total > 1.0
                        {
                            let sb_color = Color32::from_rgba_unmultiplied(255, 193, 7, 210);
                            for seg in &self.sponsor_segments {
                                let s_frac = (seg.start_secs / total).clamp(0.0, 1.0) as f32;
                                let e_frac = (seg.end_secs / total).clamp(s_frac as f64, 1.0) as f32;
                                let seg_rect = egui::Rect::from_min_max(
                                    egui::pos2(
                                        track_rect.left() + track_rect.width() * s_frac,
                                        track_rect.top(),
                                    ),
                                    egui::pos2(
                                        track_rect.left() + track_rect.width() * e_frac,
                                        track_rect.bottom(),
                                    ),
                                );
                                painter.rect_filled(seg_rect, 1.0, sb_color);
                            }
                        }
                        let radius = if bar_resp.dragged() { 8.5 } else { 6.0 };
                        painter.circle_filled(egui::pos2(thumb_x, cy), radius, Color32::WHITE);
                    }
                    let pos = display_pos;

                    ui.allocate_ui(Vec2::new(avail_w, 18.0), |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format_seconds(pos as u64))
                                    .size(12.0)
                                    .color(Color32::from_white_alpha(180)),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format_seconds(total as u64))
                                        .size(12.0)
                                        .color(Color32::from_white_alpha(180)),
                                );
                            });
                        });
                    });

                    ui.add_space(10.0);
                    let (shuffle_on, repeat_mode) = {
                        let q = self.queue.lock().unwrap();
                        (q.shuffle, q.repeat)
                    };
                    ui.columns(5, |cols| {
                        cols[0].vertical_centered(|ui| {
                            let shuf_fg = if shuffle_on {
                                Color32::WHITE
                            } else {
                                Color32::from_white_alpha(150)
                            };
                            if icons::icon_button(
                                ui,
                                icons::Icon::Shuffle,
                                46.0,
                                20.0,
                                Color32::TRANSPARENT,
                                shuf_fg,
                                22.0,
                            )
                            .clicked()
                            {
                                self.queue.lock().unwrap().toggle_shuffle();
                            }
                        });
                        cols[1].vertical_centered(|ui| {
                            if icons::icon_button(
                                ui,
                                icons::Icon::SkipPrev,
                                48.0,
                                22.0,
                                Color32::TRANSPARENT,
                                Color32::WHITE,
                                26.0,
                            )
                            .clicked()
                                && self.queue.lock().unwrap().prev_track().is_some()
                            {
                                self.play_current_queue_track(ctx);
                            }
                        });
                        cols[2].vertical_centered(|ui| {
                            let play_icon = if snap.status == PlaybackStatus::Playing {
                                icons::Icon::Pause
                            } else {
                                icons::Icon::Play
                            };
                            if icons::icon_button(
                                ui,
                                play_icon,
                                64.0,
                                32.0,
                                Color32::WHITE,
                                Color32::BLACK,
                                28.0,
                            )
                            .clicked()
                            {
                                match snap.status {
                                    PlaybackStatus::Playing => self.audio.pause(),
                                    PlaybackStatus::Paused => self.audio.resume(),
                                    _ => self.play_current_queue_track(ctx),
                                }
                            }
                        });
                        cols[3].vertical_centered(|ui| {
                            if icons::icon_button(
                                ui,
                                icons::Icon::SkipNext,
                                48.0,
                                22.0,
                                Color32::TRANSPARENT,
                                Color32::WHITE,
                                26.0,
                            )
                            .clicked()
                                && self.queue.lock().unwrap().next_track(true).is_some()
                            {
                                self.play_current_queue_track(ctx);
                            }
                        });
                        cols[4].vertical_centered(|ui| {
                            let (rep_icon, rep_fg) = match repeat_mode {
                                yr_core::queue::RepeatMode::Off => {
                                    (icons::Icon::Repeat, Color32::from_white_alpha(150))
                                }
                                yr_core::queue::RepeatMode::All => {
                                    (icons::Icon::Repeat, Color32::WHITE)
                                }
                                yr_core::queue::RepeatMode::One => {
                                    (icons::Icon::RepeatOne, Color32::WHITE)
                                }
                            };
                            if icons::icon_button(
                                ui,
                                rep_icon,
                                46.0,
                                20.0,
                                Color32::TRANSPARENT,
                                rep_fg,
                                22.0,
                            )
                            .clicked()
                            {
                                let mut q = self.queue.lock().unwrap();
                                q.repeat = q.repeat.cycle();
                            }
                        });
                    });
                }
            });

            // Bottom drag handle + 3 YTM drawer tabs: SIGUIENTE | LETRA | RELACIONADO
            ui.vertical_centered(|ui| {
                let (handle_rect, handle_resp) =
                    ui.allocate_exact_size(Vec2::new(96.0, 18.0), Sense::click_and_drag());
                let pill_rect = egui::Rect::from_center_size(
                    handle_rect.center(),
                    Vec2::new(36.0, 4.0),
                );
                ui.painter().rect_filled(
                    pill_rect,
                    CornerRadius::same(2),
                    if handle_resp.dragged() {
                        Color32::from_white_alpha(180)
                    } else {
                        Color32::from_white_alpha(110)
                    },
                );
                let drawer_open = self.show_queue || self.show_lyrics || self.show_related;
                if handle_resp.clicked() {
                    if drawer_open {
                        self.show_queue = false;
                        self.show_lyrics = false;
                        self.show_related = false;
                    } else {
                        self.now_playing_open = false;
                    }
                }
                let step = consume_vertical_drag_step(
                    ui,
                    egui::Id::new("yr_player_handle_drag"),
                    &handle_resp,
                    32.0,
                    true,
                );
                if step > 0 {
                    if drawer_open {
                        self.show_queue = false;
                        self.show_lyrics = false;
                        self.show_related = false;
                    } else {
                        self.now_playing_open = false;
                    }
                } else if step < 0 && !drawer_open {
                    self.show_queue = true;
                }
            });
            ui.add_space(6.0);
            ui.allocate_ui(Vec2::new(avail_w, 28.0), |ui| {
                ui.columns(3, |cols| {
                    for (col_idx, (active, label, mode)) in [
                        (self.show_queue, "SIGUIENTE", 0_u8),
                        (self.show_lyrics, "LETRA", 1_u8),
                        (self.show_related, "RELACIONADO", 2_u8),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        cols[col_idx].vertical_centered(|ui| {
                            let resp = ui.add(
                                egui::Label::new(
                                    RichText::new(label)
                                        .size(12.5)
                                        .strong()
                                        .color(if active {
                                            Color32::WHITE
                                        } else {
                                            Color32::from_white_alpha(165)
                                        }),
                                )
                                .sense(Sense::click()),
                            );
                            if active {
                                let underline = egui::Rect::from_min_max(
                                    egui::pos2(resp.rect.left(), resp.rect.bottom() + 4.0),
                                    egui::pos2(resp.rect.right(), resp.rect.bottom() + 6.0),
                                );
                                ui.painter().rect_filled(
                                    underline,
                                    CornerRadius::same(1),
                                    Color32::WHITE,
                                );
                            }
                            if resp.clicked() {
                                if active {
                                    self.show_queue = false;
                                    self.show_lyrics = false;
                                    self.show_related = false;
                                } else {
                                    self.show_queue = mode == 0;
                                    self.show_lyrics = mode == 1;
                                    self.show_related = mode == 2;
                                    if self.show_lyrics {
                                        self.lyrics_last_scrolled_idx = None;
                                        if self.lyrics_track_id.as_deref() != Some(&track.id) {
                                            self.load_lyrics(track.clone(), ctx);
                                        }
                                    }
                                    if self.show_related
                                        && self.related_track_id.as_deref() != Some(&track.id)
                                    {
                                        self.load_related(track.id.clone(), ctx);
                                    }
                                }
                            }
                        });
                    }
                });
            });
        });
    }

    fn render_settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.heading("Ajustes de YR");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Cerrar").clicked() {
                    self.show_settings = false;
                }
            });
        });

        ui.add_space(10.0);
        ui.label("Calidad de streaming:");
        ui.horizontal_wrapped(|ui| {
            for q in [
                AudioQuality::Low,
                AudioQuality::Medium,
                AudioQuality::High,
                AudioQuality::Best,
            ] {
                if ui
                    .selectable_value(&mut self.store.data.config.stream_quality, q, q.label())
                    .clicked()
                {
                    let _ = self.store.save();
                }
            }
        });

        ui.add_space(10.0);
        ui.label("Calidad de descargas:");
        ui.horizontal_wrapped(|ui| {
            for q in [
                AudioQuality::Low,
                AudioQuality::Medium,
                AudioQuality::High,
                AudioQuality::Best,
            ] {
                if ui
                    .selectable_value(&mut self.store.data.config.download_quality, q, q.label())
                    .clicked()
                {
                    let _ = self.store.save();
                }
            }
        });

        ui.add_space(12.0);
        if ui
            .checkbox(
                &mut self.store.data.config.autoplay_radio,
                "Radio automática al reproducir una canción",
            )
            .changed()
        {
            let _ = self.store.save();
        }
        if ui
            .checkbox(
                &mut self.store.data.config.sponsorblock_enabled,
                "SponsorBlock: saltar intros/patrocinios automáticamente",
            )
            .changed()
        {
            let _ = self.store.save();
        }

        ui.add_space(12.0);
        let mb = self.total_downloaded_bytes as f64 / (1024.0 * 1024.0);
        let cache_mb = self.stream_cache_bytes as f64 / (1024.0 * 1024.0);
        ui.label(
            RichText::new(format!("Almacenamiento offline: {mb:.1} MB"))
                .color(TEXT_SECONDARY),
        );
        ui.label(
            RichText::new(format!("Caché de streaming: {cache_mb:.1} MB"))
                .color(TEXT_SECONDARY),
        );
        ui.add_space(6.0);
        if ui.button("Limpiar caché de streaming y portadas").clicked() {
            let cache_dir = self.data_dir.join("stream_cache");
            let _ = std::fs::remove_dir_all(&cache_dir);
            let _ = std::fs::create_dir_all(&cache_dir);
            let _ = std::fs::remove_file(self.data_dir.join("home_cache.json"));
            let _ = std::fs::remove_file(self.data_dir.join("explore_cache.json"));
            self.stream_cache_bytes = 0;
            ctx.forget_all_images();
            self.set_banner("Caché y portadas liberadas");
        }

        ui.add_space(10.0);
        if icons::icon_pill_button(
            ui,
            icons::Icon::MusicNote,
            "Probar audio local (2 pistas)",
            BG_ELEVATED,
            TEXT_PRIMARY,
        )
        .clicked()
        {
            let p1 = self.data_dir.join("yr_demo_1.wav");
            let p2 = self.data_dir.join("yr_demo_2.wav");
            if generate_test_wav(&p1, 440.0, 25).is_ok()
                && generate_test_wav(&p2, 523.25, 25).is_ok()
            {
                let tracks = vec![
                    Track {
                        id: "local_demo_1".into(),
                        title: "Demo Local 440 Hz (Motor Rust)".into(),
                        artist: "YR Audio Engine".into(),
                        artist_id: None,
                        album: Some("Test Local".into()),
                        album_id: None,
                        duration_secs: Some(25),
                        thumbnail_url: None,
                        local_path: Some(p1.to_string_lossy().into_owned()),
                    },
                    Track {
                        id: "local_demo_2".into(),
                        title: "Demo Local 523 Hz (Pista 2)".into(),
                        artist: "YR Audio Engine".into(),
                        artist_id: None,
                        album: Some("Test Local".into()),
                        album_id: None,
                        duration_secs: Some(25),
                        thumbnail_url: None,
                        local_path: Some(p2.to_string_lossy().into_owned()),
                    },
                ];
                self.play_track_in_list(tracks, 0, ctx);
            } else {
                self.set_banner("No se pudo crear audio local de prueba");
            }
        }
    }

    fn render_action_sheet_modal(&mut self, ctx: &egui::Context) {
        let Some(track) = self.action_sheet_track.clone() else {
            return;
        };

        let screen = ctx.screen_rect();
        if render_modal_scrim(ctx, "yr_action_sheet_scrim") {
            self.action_sheet_track = None;
        }

        egui::Area::new(egui::Id::new("yr_action_sheet_panel"))
            .anchor(egui::Align2::CENTER_BOTTOM, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_width(screen.width());
                egui::Frame::new()
                    .fill(Color32::from_rgb(24, 24, 28))
                    .stroke(Stroke::new(0.8_f32, Color32::from_rgb(48, 48, 54)))
                    .corner_radius(CornerRadius {
                        nw: 18,
                        ne: 18,
                        sw: 0,
                        se: 0,
                    })
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.set_width((screen.width() - 28.0).max(180.0));
                        // Header: Thumb | Title + Artist • Duration | ThumbUp | Close
                ui.horizontal(|ui| {
                    render_track_thumb(ui, &track, 46.0);
                    let tw = (ui.available_width() - 90.0).max(120.0);
                    let dur_secs = track.duration_secs.or_else(|| {
                        let cur = self
                            .queue
                            .lock()
                            .unwrap()
                            .current_track()
                            .map(|t| t.id.clone());
                        if cur.as_deref() == Some(track.id.as_str()) {
                            self.audio
                                .snapshot()
                                .duration_secs
                                .map(|d| d.round().max(0.0) as u64)
                        } else {
                            None
                        }
                    });
                    let sheet_sub = match dur_secs.filter(|&s| s > 0) {
                        Some(s) => format!("{} • {}:{:02}", track.artist, s / 60, s % 60),
                        None => track.artist.clone(),
                    };
                    ui.allocate_ui(Vec2::new(tw, 52.0), |ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.set_max_width(tw);
                            render_clamped_label(ui, &track.title, 15.0, Color32::WHITE, tw, 2);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(sheet_sub)
                                        .size(12.5)
                                        .color(TEXT_SECONDARY),
                                )
                                .truncate(),
                            );
                        });
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icons::icon_button(
                            ui,
                            icons::Icon::Close,
                            32.0,
                            16.0,
                            Color32::TRANSPARENT,
                            Color32::WHITE,
                            16.0,
                        )
                        .clicked()
                        {
                            self.action_sheet_track = None;
                        }
                        let liked = self.store.is_liked(&track.id);
                        if icons::icon_button(
                            ui,
                            icons::Icon::ThumbUp,
                            34.0,
                            18.0,
                            Color32::TRANSPARENT,
                            if liked { ACCENT_RED } else { Color32::WHITE },
                            16.0,
                        )
                        .clicked()
                        {
                            self.store.toggle_like(track.clone());
                        }
                    });
                });

                ui.add_space(12.0);
                // 3 prominent top action cards matching reference YTM bottom sheet
                let card_w = (ui.available_width() / 3.0 - 8.0).clamp(80.0, 128.0);
                ui.columns(3, |cols| {
                    let top_actions = [
                        (icons::Icon::QueueNext, "Reproducir a\ncontinuación", 0_u8),
                        (icons::Icon::PlaylistAdd, "Guardar en una\nplaylist", 1_u8),
                        (icons::Icon::Share, "Compartir", 2_u8),
                    ];
                    for (ci, (ic, lbl, act_id)) in top_actions.into_iter().enumerate() {
                        cols[ci].vertical_centered(|ui| {
                            let (r, resp) =
                                ui.allocate_exact_size(Vec2::new(card_w, 64.0), Sense::click());
                            if ui.is_rect_visible(r) {
                                ui.painter().rect_filled(
                                    r,
                                    CornerRadius::same(12),
                                    Color32::from_rgb(36, 36, 40),
                                );
                                icons::draw_icon(
                                    ui.painter(),
                                    egui::Rect::from_center_size(
                                        egui::pos2(r.center().x, r.top() + 26.0),
                                        Vec2::splat(24.0),
                                    ),
                                    ic,
                                    Color32::WHITE,
                                );
                            }
                            if resp.clicked() {
                                match act_id {
                                    0 => {
                                        self.queue.lock().unwrap().enqueue_next(track.clone());
                                        self.set_banner(format!("Siguiente: {}", track.title));
                                    }
                                    1 => self.playlist_picker_track = Some(track.clone()),
                                    _ => {
                                        crate::android_media::share_text(
                                            &track.title,
                                            &format!(
                                                "{} — {} https://music.youtube.com/watch?v={}",
                                                track.title, track.artist, track.id
                                            ),
                                        );
                                    }
                                }
                                self.action_sheet_track = None;
                            }
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(lbl)
                                    .size(11.5)
                                    .color(Color32::from_white_alpha(230)),
                            );
                        });
                    }
                });

                ui.add_space(10.0);
                let is_downloaded = self
                    .downloads
                    .iter()
                    .any(|d| d.track.id == track.id && d.status == DownloadStatus::Completed);
                let is_pinned = self.store.is_pinned_speed_dial(&track.title);
                let liked = self.store.is_liked(&track.id);

                let sheet_row = |ui: &mut egui::Ui, icon: icons::Icon, label: &str| -> bool {
                    let row_w = ui.available_width();
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(row_w, 40.0), Sense::click());
                    if ui.is_rect_visible(r) {
                        if resp.hovered() {
                            ui.painter().rect_filled(
                                r,
                                CornerRadius::same(8),
                                Color32::from_white_alpha(14),
                            );
                        }
                        icons::draw_icon(
                            ui.painter(),
                            egui::Rect::from_center_size(
                                egui::pos2(r.left() + 18.0, r.center().y),
                                Vec2::splat(18.0),
                            ),
                            icon,
                            Color32::WHITE,
                        );
                        ui.painter().text(
                            egui::pos2(r.left() + 48.0, r.center().y),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::proportional(14.5),
                            Color32::WHITE,
                        );
                    }
                    resp.clicked()
                };

                if sheet_row(ui, icons::Icon::Radio, "Comenzar mix") {
                    self.start_radio(track.id.clone(), ctx);
                    self.action_sheet_track = None;
                }
                if sheet_row(ui, icons::Icon::QueueAdd, "Agregar a la fila") {
                    self.queue.lock().unwrap().enqueue(track.clone());
                    self.set_banner(format!("Agregada a la fila: {}", track.title));
                    self.action_sheet_track = None;
                }
                if sheet_row(
                    ui,
                    icons::Icon::Library,
                    if liked {
                        "Quitar de biblioteca (Me gusta)"
                    } else {
                        "Guardar en la biblioteca"
                    },
                ) {
                    self.store.toggle_like(track.clone());
                    self.action_sheet_track = None;
                }
                if !is_downloaded && sheet_row(ui, icons::Icon::Download, "Descargar") {
                    self.enqueue_download(track.clone(), ctx);
                    self.action_sheet_track = None;
                }
                if let Some(aid) = track.album_id.clone() {
                    if sheet_row(ui, icons::Icon::Album, "Ir al álbum") {
                        self.now_playing_open = false;
                        self.open_album(aid, ctx);
                        self.action_sheet_track = None;
                    }
                }
                if let Some(arid) = track.artist_id.clone() {
                    if sheet_row(ui, icons::Icon::Artist, "Ir al artista") {
                        self.now_playing_open = false;
                        self.open_artist(arid, ctx);
                        self.action_sheet_track = None;
                    }
                }
                if sheet_row(
                    ui,
                    icons::Icon::Pin,
                    if is_pinned {
                        "Quitar de Accesos directos"
                    } else {
                        "Fijar en Accesos directos"
                    },
                ) {
                    let card = HomeShelfCard {
                        title: track.title.clone(),
                        subtitle: track.artist.clone(),
                        thumbnail_url: track.thumbnail_url.clone(),
                        kind: ShelfCardKind::Track(track.clone()),
                    };
                    let pinned = self.store.toggle_pin_speed_dial(card);
                    self.set_banner(if pinned {
                        "Fijado en Accesos directos"
                    } else {
                        "Quitado de Accesos directos"
                    });
                    self.action_sheet_track = None;
                }
                let timer_active = self.sleep_timer_until.lock().unwrap().is_some()
                    || self
                        .sleep_after_track
                        .load(std::sync::atomic::Ordering::SeqCst);
                let timer_label = if timer_active {
                    "Temporizador de apagado (activo)"
                } else {
                    "Temporizador de apagado"
                };
                if sheet_row(ui, icons::Icon::Moon, timer_label) {
                    self.show_sleep_timer_sheet = true;
                    self.action_sheet_track = None;
                }

                        #[cfg(target_os = "android")]
                        ui.add_space(46.0);
                    });
            });
    }

    fn render_sleep_timer_modal(&mut self, ctx: &egui::Context) {
        if !self.show_sleep_timer_sheet {
            return;
        }
        let screen = ctx.screen_rect();
        if render_modal_scrim(ctx, "yr_sleep_timer_scrim") {
            self.show_sleep_timer_sheet = false;
        }

        let active_until = *self.sleep_timer_until.lock().unwrap();
        let after_track = self
            .sleep_after_track
            .load(std::sync::atomic::Ordering::SeqCst);

        egui::Area::new(egui::Id::new("yr_sleep_timer_panel"))
            .anchor(egui::Align2::CENTER_BOTTOM, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_width(screen.width());
                egui::Frame::new()
                    .fill(Color32::from_rgb(24, 24, 28))
                    .stroke(Stroke::new(0.8_f32, Color32::from_rgb(48, 48, 54)))
                    .corner_radius(CornerRadius {
                        nw: 18,
                        ne: 18,
                        sw: 0,
                        se: 0,
                    })
                    .inner_margin(egui::Margin::symmetric(16, 14))
                    .show(ui, |ui| {
                        ui.set_width((screen.width() - 32.0).max(180.0));
                        ui.horizontal(|ui| {
                            let (mr, _) =
                                ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                            icons::draw_icon(ui.painter(), mr, icons::Icon::Moon, ACCENT_RED);
                            ui.label(
                                RichText::new("Temporizador de apagado")
                                    .size(16.5)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icons::icon_button(
                                    ui,
                                    icons::Icon::Close,
                                    32.0,
                                    16.0,
                                    Color32::TRANSPARENT,
                                    Color32::WHITE,
                                    16.0,
                                )
                                .clicked()
                                {
                                    self.show_sleep_timer_sheet = false;
                                }
                            });
                        });

                        if let Some(until) = active_until {
                            let rem = until.saturating_duration_since(Instant::now()).as_secs();
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(format!(
                                    "Se pausará en {:02}:{:02}",
                                    rem / 60,
                                    rem % 60
                                ))
                                .size(13.0)
                                .color(GREEN_OK),
                            );
                        } else if after_track {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("Se pausará al terminar esta canción")
                                    .size(13.0)
                                    .color(GREEN_OK),
                            );
                        }

                        ui.add_space(10.0);
                        let row_w = ui.available_width();
                        let timer_row = |ui: &mut egui::Ui,
                                         label: &str,
                                         bg: Color32,
                                         fg: Color32|
                         -> bool {
                            let (r, resp) =
                                ui.allocate_exact_size(Vec2::new(row_w, 42.0), Sense::click());
                            if ui.is_rect_visible(r) {
                                ui.painter().rect_filled(r, CornerRadius::same(10), bg);
                                ui.painter().text(
                                    egui::pos2(r.left() + 14.0, r.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    label,
                                    egui::FontId::proportional(14.5),
                                    fg,
                                );
                            }
                            ui.add_space(6.0);
                            resp.clicked()
                        };

                        let default_bg = Color32::from_white_alpha(12);
                        for (mins, label) in [
                            (15_u64, "15 minutos"),
                            (30_u64, "30 minutos"),
                            (45_u64, "45 minutos"),
                            (60_u64, "60 minutos"),
                        ] {
                            if timer_row(ui, label, default_bg, Color32::WHITE) {
                                *self.sleep_timer_until.lock().unwrap() =
                                    Some(Instant::now() + Duration::from_secs(mins * 60));
                                self.sleep_after_track
                                    .store(false, std::sync::atomic::Ordering::SeqCst);
                                self.set_banner(format!("Temporizador: {label}"));
                                self.show_sleep_timer_sheet = false;
                            }
                        }

                        if timer_row(
                            ui,
                            "Al terminar la canción actual",
                            if after_track {
                                Color32::from_white_alpha(28)
                            } else {
                                default_bg
                            },
                            Color32::WHITE,
                        ) {
                            *self.sleep_timer_until.lock().unwrap() = None;
                            self.sleep_after_track
                                .store(true, std::sync::atomic::Ordering::SeqCst);
                            self.set_banner("Se pausará al terminar esta canción");
                            self.show_sleep_timer_sheet = false;
                        }

                        if (active_until.is_some() || after_track)
                            && timer_row(
                                ui,
                                "Apagar temporizador",
                                Color32::from_rgb(56, 22, 26),
                                ACCENT_RED,
                            )
                        {
                            *self.sleep_timer_until.lock().unwrap() = None;
                            self.sleep_after_track
                                .store(false, std::sync::atomic::Ordering::SeqCst);
                            self.set_banner("Temporizador cancelado");
                            self.show_sleep_timer_sheet = false;
                        }

                        #[cfg(target_os = "android")]
                        ui.add_space(46.0);
                    });
            });
    }

    fn render_playlist_picker_modal(&mut self, ctx: &egui::Context) {
        let Some(track) = self.playlist_picker_track.clone() else {
            return;
        };
        let screen = ctx.screen_rect();
        let mut close_sheet = render_modal_scrim(ctx, "yr_playlist_picker_scrim");

        egui::Area::new(egui::Id::new("yr_playlist_picker_panel"))
            .anchor(egui::Align2::CENTER_BOTTOM, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_width(screen.width());
                egui::Frame::new()
                    .fill(Color32::from_rgb(24, 24, 28))
                    .stroke(Stroke::new(0.8_f32, Color32::from_rgb(48, 48, 54)))
                    .corner_radius(CornerRadius {
                        nw: 18,
                        ne: 18,
                        sw: 0,
                        se: 0,
                    })
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        let content_w = (screen.width() - 28.0).max(180.0);
                        ui.set_width(content_w);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Guardar en playlist")
                                    .size(16.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icons::icon_button(
                                    ui,
                                    icons::Icon::Close,
                                    32.0,
                                    16.0,
                                    Color32::TRANSPARENT,
                                    Color32::WHITE,
                                    16.0,
                                )
                                .clicked()
                                {
                                    close_sheet = true;
                                }
                            });
                        });
                        render_clamped_label(
                            ui,
                            &format!("{} • {}", track.title, track.artist),
                            12.5,
                            TEXT_SECONDARY,
                            content_w,
                            2,
                        );
                        ui.add_space(10.0);

                        let pls = self.store.data.playlists.clone();
                        if pls.is_empty() {
                            ui.label(
                                RichText::new("No tienes playlists aún.")
                                    .size(13.0)
                                    .color(TEXT_SECONDARY),
                            );
                        } else {
                            egui::ScrollArea::vertical()
                                .id_salt("yr_playlist_picker_scroll")
                                .max_height((screen.height() * 0.34).max(140.0))
                                .show(ui, |ui| {
                                    for pl in pls {
                                        let clicked = egui::Frame::new()
                                            .fill(BG_ELEVATED)
                                            .corner_radius(CornerRadius::same(10))
                                            .inner_margin(egui::Margin::symmetric(12, 9))
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                let resp = ui.horizontal(|ui| {
                                                    draw_inline_icon(
                                                        ui,
                                                        icons::Icon::PlaylistAdd,
                                                        18.0,
                                                        Color32::WHITE,
                                                    );
                                                    ui.add_space(8.0);
                                                    ui.label(
                                                        RichText::new(&pl.name)
                                                            .size(14.5)
                                                            .strong()
                                                            .color(Color32::WHITE),
                                                    );
                                                    ui.with_layout(
                                                        Layout::right_to_left(Align::Center),
                                                        |ui| {
                                                            ui.label(
                                                                RichText::new(format!(
                                                                    "{} pistas",
                                                                    pl.tracks.len()
                                                                ))
                                                                .size(12.0)
                                                                .color(TEXT_SECONDARY),
                                                            );
                                                        },
                                                    );
                                                });
                                                ui.interact(
                                                    resp.response.rect,
                                                    ui.make_persistent_id(("pl_pick", &pl.id)),
                                                    Sense::click(),
                                                )
                                                .clicked()
                                            })
                                            .inner;
                                        if clicked {
                                            self.store.add_to_playlist(&pl.id, track.clone());
                                            self.set_banner(format!("Añadida a '{}'", pl.name));
                                            close_sheet = true;
                                        }
                                        ui.add_space(4.0);
                                    }
                                });
                        }

                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let input_w = (content_w - 134.0).max(120.0);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.new_playlist_name)
                                    .hint_text("Nueva playlist...")
                                    .desired_width(input_w),
                            );
                            let can_create = !self.new_playlist_name.trim().is_empty();
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Crear y añadir")
                                            .size(13.0)
                                            .strong()
                                            .color(Color32::WHITE),
                                    )
                                    .fill(if can_create {
                                        ACCENT_RED
                                    } else {
                                        BG_ELEVATED
                                    })
                                    .corner_radius(CornerRadius::same(18))
                                    .min_size(Vec2::new(112.0, 34.0)),
                                )
                                .clicked()
                                && can_create
                            {
                                let name = self.new_playlist_name.trim().to_string();
                                let created = self.store.create_playlist(name);
                                self.store.add_to_playlist(&created.id, track.clone());
                                self.new_playlist_name.clear();
                                self.set_banner(format!("Añadida a '{}'", created.name));
                                close_sheet = true;
                            }
                        });
                        #[cfg(target_os = "android")]
                        ui.add_space(46.0);
                    });
            });

        if close_sheet {
            self.playlist_picker_track = None;
        }
    }

    fn render_google_login_modal(&mut self, ctx: &egui::Context) {
        if !self.show_google_login {
            return;
        }
        let screen = ctx.screen_rect();
        let mut close_modal = render_modal_scrim(ctx, "yr_google_login_scrim");
        let card_w = (screen.width() - 36.0).clamp(270.0, 340.0);

        egui::Area::new(egui::Id::new("yr_google_login_panel"))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(24, 24, 28))
                    .stroke(Stroke::new(0.8_f32, Color32::from_rgb(48, 48, 54)))
                    .corner_radius(CornerRadius::same(18))
                    .inner_margin(egui::Margin::symmetric(16, 14))
                    .show(ui, |ui| {
                        ui.set_width(card_w);
                        ui.horizontal(|ui| {
                            draw_inline_icon(ui, icons::Icon::Account, 20.0, ACCENT_RED);
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("Cuenta de Google")
                                    .size(17.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icons::icon_button(
                                    ui,
                                    icons::Icon::Close,
                                    30.0,
                                    15.0,
                                    Color32::TRANSPARENT,
                                    Color32::WHITE,
                                    15.0,
                                )
                                .clicked()
                                {
                                    close_modal = true;
                                }
                            });
                        });
                        ui.add_space(8.0);

                        if self.home_feed.is_logged_in {
                            ui.label(
                                RichText::new("Sesión de Google conectada")
                                    .strong()
                                    .size(15.5)
                                    .color(GREEN_OK),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(
                                    "Tus listas, historial y recomendaciones personalizadas de YouTube Music están activas.",
                                )
                                .small()
                                .color(TEXT_SECONDARY),
                            );
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Refresh,
                                    "Sincronizar",
                                    BG_ELEVATED,
                                    TEXT_PRIMARY,
                                )
                                .clicked()
                                {
                                    self.refresh_home(ctx);
                                    close_modal = true;
                                }
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new("Cerrar sesión").color(ACCENT_RED),
                                        )
                                        .fill(BG_ELEVATED)
                                        .corner_radius(CornerRadius::same(18))
                                        .min_size(Vec2::new(108.0, 34.0)),
                                    )
                                    .clicked()
                                {
                                    self.logout_google(ctx);
                                    close_modal = true;
                                }
                            });
                        } else {
                            ui.label(
                                RichText::new("Mantén tus recomendaciones de Google")
                                    .strong()
                                    .size(15.0)
                                    .color(TEXT_PRIMARY),
                            );
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(
                                    "Inicia sesión para cargar Tus listas, Escuchar de nuevo y tus mixes de YouTube Music.",
                                )
                                .small()
                                .color(TEXT_SECONDARY),
                            );
                            ui.add_space(10.0);

                            if icons::icon_pill_button(
                                ui,
                                icons::Icon::Account,
                                "Iniciar sesión con Google",
                                ACCENT_RED,
                                Color32::WHITE,
                            )
                            .clicked()
                            {
                                crate::android_media::open_google_login();
                            }

                            ui.add_space(10.0);
                            ui.separator();
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("O conecta con código de dispositivo:")
                                    .small()
                                    .color(TEXT_SECONDARY),
                            );
                            if let Some(auth) = self.google_device_auth.clone() {
                                ui.add_space(4.0);
                                ui.label(
                                    RichText::new(format!("1. Abre: {}", auth.verification_url))
                                        .small()
                                        .color(TEXT_PRIMARY),
                                );
                                ui.label(
                                    RichText::new(&auth.user_code)
                                        .size(22.0)
                                        .strong()
                                        .color(ACCENT_RED),
                                );
                                ui.horizontal(|ui| {
                                    if self.google_login_loading {
                                        ui.spinner();
                                    }
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("Verificar ahora")
                                                    .color(Color32::WHITE),
                                            )
                                            .fill(ACCENT_RED)
                                            .corner_radius(CornerRadius::same(16)),
                                        )
                                        .clicked()
                                    {
                                        self.poll_google_device_auth(ctx);
                                    }
                                });
                            } else if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(if self.google_login_loading {
                                            "Generando código..."
                                        } else {
                                            "Generar código (google.com/device)"
                                        })
                                        .color(TEXT_PRIMARY),
                                    )
                                    .fill(BG_ELEVATED)
                                    .corner_radius(CornerRadius::same(16)),
                                )
                                .clicked()
                                && !self.google_login_loading
                            {
                                self.start_google_device_auth(ctx);
                            }

                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("Cookie manual de music.youtube.com (opcional):")
                                    .small()
                                    .color(TEXT_SECONDARY),
                            );
                            ui.horizontal(|ui| {
                                let cookie_w = (card_w - 92.0).max(150.0);
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.google_cookie_input)
                                        .hint_text("SAPISID=...; ...")
                                        .desired_width(cookie_w),
                                );
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new("Guardar").color(Color32::WHITE),
                                        )
                                        .fill(BG_ELEVATED)
                                        .corner_radius(CornerRadius::same(16))
                                        .min_size(Vec2::new(76.0, 32.0)),
                                    )
                                    .clicked()
                                    && !self.google_login_loading
                                {
                                    let c = self.google_cookie_input.clone();
                                    self.login_with_cookie(c, ctx);
                                }
                            });

                            if let Some(err) = &self.google_auth_error {
                                ui.add_space(6.0);
                                ui.label(RichText::new(err).small().color(ACCENT_RED));
                            }
                        }
                    });
            });

        if close_modal {
            self.show_google_login = false;
        }
    }
}

/// Accumulates vertical drag delta without nesting `egui::Context` locks.
/// Returns `1` when dragged downward past `threshold`, `-1` upward, or `0`.
/// When `single_shot` is true, latches until pointer release so one swipe triggers once.
fn consume_vertical_drag_step(
    ui: &egui::Ui,
    id: egui::Id,
    resp: &egui::Response,
    threshold: f32,
    single_shot: bool,
) -> i32 {
    if resp.dragged() {
        // Read drag_delta before `ui.data_mut` so Context read & write locks never nest.
        let delta_y = resp.drag_delta().y;
        ui.data_mut(|d| {
            let prev = d.get_temp::<f32>(id).unwrap_or(0.0);
            if prev.is_nan() {
                return 0;
            }
            let mut accum = prev + delta_y;
            let step = if accum >= threshold {
                accum = if single_shot { f32::NAN } else { accum - threshold };
                1
            } else if accum <= -threshold {
                accum = if single_shot { f32::NAN } else { accum + threshold };
                -1
            } else {
                0
            };
            d.insert_temp(id, accum);
            step
        })
    } else if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<f32>(id));
        0
    } else {
        0
    }
}

fn draw_inline_icon(ui: &mut egui::Ui, icon: icons::Icon, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    icons::draw_icon(ui.painter(), rect, icon, color);
}

fn render_modal_scrim(ctx: &egui::Context, id_salt: &str) -> bool {
    let screen = ctx.screen_rect();
    let mut dismissed = false;
    egui::Area::new(egui::Id::new(id_salt))
        .fixed_pos(screen.min)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let (_, scrim_resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter()
                .rect_filled(screen, CornerRadius::ZERO, Color32::from_black_alpha(165));
            if scrim_resp.clicked() {
                dismissed = true;
            }
        });
    dismissed
}

fn clamped_layout_job(
    text: &str,
    size: f32,
    color: Color32,
    max_width: f32,
    max_rows: usize,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::simple(
        text.to_string(),
        egui::FontId::proportional(size),
        color,
        max_width,
    );
    job.wrap.max_rows = max_rows;
    job.wrap.overflow_character = Some('…');
    job
}

fn render_clamped_label(
    ui: &mut egui::Ui,
    text: &str,
    size: f32,
    color: Color32,
    max_width: f32,
    max_rows: usize,
) {
    let job = clamped_layout_job(text, size, color, max_width, max_rows);
    ui.add(egui::Label::new(job).selectable(false));
}

fn render_collection_row(
    ui: &mut egui::Ui,
    id_salt: (&str, &str),
    thumb_url: Option<&str>,
    title: &str,
    subtitle: Option<&str>,
) -> bool {
    let mut clicked = false;
    egui::Frame::new()
        .fill(BG_SURFACE)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(10.0)
        .show(ui, |ui| {
            let row_start = ui.cursor().min;
            let row_w = ui.available_width();
            ui.horizontal(|ui| {
                render_thumb(ui, thumb_url, 48.0);
                let text_w = (row_w - 86.0).max(110.0);
                ui.allocate_ui(Vec2::new(text_w, 48.0), |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.set_max_width(text_w);
                        render_clamped_label(ui, title, 14.5, TEXT_PRIMARY, text_w, 2);
                        if let Some(sub) = subtitle {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(sub).small().color(TEXT_SECONDARY),
                                )
                                .truncate()
                                .selectable(false),
                            );
                        }
                    });
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("›").size(20.0).color(TEXT_SECONDARY));
                });
            });
            let hit_rect = egui::Rect::from_min_size(row_start, Vec2::new(row_w, 48.0));
            let resp = ui.interact(hit_rect, ui.id().with(id_salt), Sense::click());
            if resp.clicked() {
                clicked = true;
            }
        });
    clicked
}

/// Paints a remote image only once ready so failed loads preserve the clean vector placeholder.
fn paint_ready_image(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    uri: String,
    size: Vec2,
    corner_radius: CornerRadius,
) {
    let img = Image::new(uri)
        .fit_to_exact_size(size)
        .corner_radius(corner_radius);
    match img.load_for_size(ui.ctx(), size) {
        Ok(egui::load::TexturePoll::Ready { .. }) => {
            img.paint_at(ui, rect);
        }
        Ok(egui::load::TexturePoll::Pending { .. }) => {
            ui.ctx().request_repaint();
        }
        Err(_) => {}
    }
}

fn render_track_thumb(ui: &mut egui::Ui, track: &Track, size: f32) {
    let url = track_thumb_url(track.thumbnail_url.as_deref(), &track.id);
    render_thumb(ui, url.as_deref(), size);
}

fn render_thumb(ui: &mut egui::Ui, url: Option<&str>, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(8), BG_ELEVATED);
    if !ui.is_rect_visible(rect) {
        return;
    }
    let note_rect = egui::Rect::from_center_size(
        rect.center(),
        Vec2::splat((size * 0.44).clamp(14.0, 44.0)),
    );
    icons::draw_icon(
        ui.painter(),
        note_rect,
        icons::Icon::MusicNote,
        Color32::from_white_alpha(95),
    );
    if let Some(u) = url {
        let max_px = if size <= 72.0 {
            160
        } else if size <= 150.0 {
            224
        } else {
            320
        };
        let thumb_uri = tracked_thumb_uri(u, max_px);
        paint_ready_image(
            ui,
            rect,
            thumb_uri,
            Vec2::splat(size),
            CornerRadius::same(8),
        );
    }
}

static IMAGE_LRU: OnceLock<Mutex<ImageLru>> = OnceLock::new();

fn image_lru() -> &'static Mutex<ImageLru> {
    IMAGE_LRU.get_or_init(|| Mutex::new(ImageLru::default()))
}

fn tracked_thumb_uri(url: &str, max_px: u32) -> String {
    let uri = optimize_thumb_url(url, max_px);
    if !uri.is_empty() {
        image_lru().lock().unwrap().touch(&uri);
    }
    uri
}

fn advance_and_prune_image_cache(ctx: &egui::Context) {
    let evicted = image_lru().lock().unwrap().advance_and_collect_evictions();
    for uri in evicted {
        ctx.forget_image(&uri);
    }
}

fn friendly_error_message(prefix: &str, raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("error sending request")
        || low.contains("dns")
        || low.contains("failed to lookup")
        || low.contains("connection refused")
        || low.contains("network is unreachable")
    {
        format!("{prefix}: sin conexión a internet.")
    } else if low.contains("timed out") || low.contains("timeout") {
        format!("{prefix}: tiempo de espera agotado.")
    } else if low.contains("extraction error:") || low.contains("was not found (no content)") {
        format!("{prefix}: no disponible para esta pista.")
    } else if let Some((clean, _)) = raw.split_once(" (http") {
        format!("{prefix}: {}", clean.trim())
    } else {
        format!("{prefix}: {raw}")
    }
}

fn paint_ambient_gradient(painter: &egui::Painter, bg_rect: egui::Rect, ambient: Color32) {
    let top_col = Color32::from_rgb(
        ((ambient.r() as f32 * 0.72).round() as u8).max(20),
        ((ambient.g() as f32 * 0.72).round() as u8).max(18),
        ((ambient.b() as f32 * 0.72).round() as u8).max(20),
    );
    let mid_col = Color32::from_rgb(
        ((ambient.r() as f32 * 0.36).round() as u8).max(14),
        ((ambient.g() as f32 * 0.36).round() as u8).max(13),
        ((ambient.b() as f32 * 0.36).round() as u8).max(15),
    );
    let mut mesh = egui::Mesh::default();
    let mid_y = bg_rect.top() + bg_rect.height() * 0.58;
    mesh.colored_vertex(bg_rect.left_top(), top_col);
    mesh.colored_vertex(bg_rect.right_top(), top_col);
    mesh.colored_vertex(egui::pos2(bg_rect.left(), mid_y), mid_col);
    mesh.colored_vertex(egui::pos2(bg_rect.right(), mid_y), mid_col);
    mesh.colored_vertex(bg_rect.left_bottom(), BG_PRIMARY);
    mesh.colored_vertex(bg_rect.right_bottom(), BG_PRIMARY);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    mesh.add_triangle(2, 3, 4);
    mesh.add_triangle(3, 5, 4);
    painter.add(egui::Shape::mesh(mesh));
}

fn dir_size_bytes(path: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0_u64;
    for entry in entries.flatten() {
        let p = entry.path();
        if let Ok(meta) = entry.metadata() {
            if meta.is_dir() {
                total = total.saturating_add(dir_size_bytes(&p));
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

fn generate_test_wav(path: &std::path::Path, freq_hz: f32, duration_secs: u32) -> std::io::Result<()> {
    use std::io::Write;
    let sample_rate: u32 = 44100;
    let num_samples = sample_rate * duration_secs;
    let data_bytes = num_samples * 2;
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_bytes).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16_u32.to_le_bytes())?;
    f.write_all(&1_u16.to_le_bytes())?;
    f.write_all(&1_u16.to_le_bytes())?;
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&(sample_rate * 2).to_le_bytes())?;
    f.write_all(&2_u16.to_le_bytes())?;
    f.write_all(&16_u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_bytes.to_le_bytes())?;
    let mut buf = Vec::with_capacity(data_bytes as usize);
    for i in 0..num_samples {
        let t = i as f32 / sample_rate as f32;
        let sample = ((2.0 * std::f32::consts::PI * freq_hz * t).sin() * 0.25 * i16::MAX as f32) as i16;
        buf.extend_from_slice(&sample.to_le_bytes());
    }
    f.write_all(&buf)?;
    Ok(())
}
