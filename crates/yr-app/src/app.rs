use crate::color_extract;
use crate::icons;
use crate::theme::{
    apply_ytm_theme, ACCENT_RED, BG_ELEVATED, BG_PRIMARY, BG_SURFACE, GREEN_OK, TEXT_PRIMARY,
    TEXT_SECONDARY,
};
use eframe::CreationContext;
use egui::{Align, Align2, Color32, CornerRadius, Image, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use yr_audio::{AudioEngine, AudioSnapshot};
use yr_core::{
    format_seconds, image_cache::track_thumb_url, optimize_thumb_url, AlbumDetail, ArtistDetail,
    AudioQuality, EqualizerConfig, ExploreFeed, GoogleAccountInfo, GoogleDeviceAuth, HomeFeed, HomeShelf, HomeShelfCard,
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
    History,
    Playlists,
    Songs,
    Albums,
    Artists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsCategory {
    #[default]
    Root,
    Morphe,
    FamilyCenter,
    General,
    Playback,
    DataSaver,
    DownloadsStorage,
    Notifications,
    Privacy,
    Recommendations,
    About,
}

impl SettingsCategory {
    pub fn title(self) -> &'static str {
        match self {
            Self::Root => "Configuración",
            Self::Morphe => "Morphe",
            Self::FamilyCenter => "Centro familiar",
            Self::General => "General",
            Self::Playback => "Reproducción",
            Self::DataSaver => "Ahorro de datos",
            Self::DownloadsStorage => "Descargas y almacenamiento",
            Self::Notifications => "Notificaciones",
            Self::Privacy => "Privacidad y datos",
            Self::Recommendations => "Recomendaciones",
            Self::About => "Acerca de YR Music",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueTuner {
    #[default]
    All,
    Familiar,
    Discovery,
    Popular,
    Deep,
}

impl QueueTuner {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "Todo",
            Self::Familiar => "Familiar",
            Self::Discovery => "Descubrimiento",
            Self::Popular => "Populares",
            Self::Deep => "Profundo",
        }
    }
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
    GoogleLoginDone(
        Result<(), String>,
        Option<String>,
        Option<GoogleAccountInfo>,
    ),
    GoogleCookieReceived(String),
}

/// Disambiguated touch drag & 60fps kinetic fling momentum scroller for egui on mobile.
#[derive(Clone, Debug, Default)]
pub struct SmoothTouchScroller {
    pub offset_y: f32,
    pub vel_y: f32,
    pub max_offset_y: f32,
    pub is_dragging: bool,
    pub drag_slop_exceeded: bool,
}

impl SmoothTouchScroller {
    pub fn update(
        &mut self,
        ui: &egui::Ui,
        viewport_rect: egui::Rect,
        can_pull_refresh: bool,
    ) {
        let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05);
        let pointer = ui.input(|i| i.pointer.clone());
        let down = pointer.any_down();
        let pressed = pointer.any_pressed();
        let released = pointer.any_released();
        let origin_opt = pointer.press_origin();
        let press_in_view = origin_opt.is_some_and(|p| viewport_rect.contains(p));

        if pressed && press_in_view {
            self.vel_y = 0.0;
            self.is_dragging = false;
            self.drag_slop_exceeded = false;
        }

        if down && press_in_view {
            let total_dist = match (origin_opt, pointer.latest_pos()) {
                (Some(o), Some(p)) => (p.y - o.y).abs(),
                _ => 0.0,
            };

            // Touch slop threshold: don't scroll until finger moves > 8.0 pixels,
            // preventing natural finger jitter from swallowing clicks on mobile.
            if total_dist > 8.0 || self.drag_slop_exceeded {
                self.drag_slop_exceeded = true;
                let delta = pointer.delta();
                if delta.y.abs() > 0.1 {
                    let is_vertical = delta.y.abs() >= delta.x.abs() * 0.6;
                    if is_vertical || self.is_dragging {
                        self.is_dragging = true;
                        self.vel_y = 0.0;

                        // If pulling down at top, let pull-to-refresh handle it without scrolling
                        if !(self.offset_y <= 3.0 && delta.y > 0.0 && can_pull_refresh) {
                            self.offset_y = (self.offset_y - delta.y).clamp(0.0, self.max_offset_y);
                            ui.ctx().request_repaint();
                        }
                    }
                }
            }
        }

        if released {
            let was_dragging = self.is_dragging;
            self.is_dragging = false;
            self.drag_slop_exceeded = false;
            if was_dragging {
                let vy = pointer.velocity().y;
                if vy.abs() > 60.0 {
                    // Moving downward (positive vy) means scrolling up (decreasing offset_y)
                    self.vel_y = vy;
                }
            }
        }

        // Kinetic fling momentum (natural deceleration)
        if !down && self.vel_y.abs() > 15.0 {
            let friction = 1500.0 * dt;
            if friction >= self.vel_y.abs() {
                self.vel_y = 0.0;
            } else {
                self.vel_y -= friction * self.vel_y.signum();
                self.offset_y = (self.offset_y - self.vel_y * dt).clamp(0.0, self.max_offset_y);
                ui.ctx().request_repaint();
            }
        } else if !down {
            self.vel_y = 0.0;
        }

        // Support mouse wheel / touchpad scroll
        let wheel = ui.input(|i| i.raw_scroll_delta.y);
        if wheel.abs() > 0.5 {
            self.vel_y = 0.0;
            self.offset_y = (self.offset_y - wheel).clamp(0.0, self.max_offset_y);
            ui.ctx().request_repaint();
        }
    }

    pub fn sync_bounds(&mut self, content_h: f32, view_h: f32) {
        self.max_offset_y = (content_h - view_h).max(0.0);
        self.offset_y = self.offset_y.clamp(0.0, self.max_offset_y);
    }
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
    show_account_modal: bool,
    queue_tuner: QueueTuner,
    settings_category: SettingsCategory,
    is_video_mode: bool,
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
    show_equalizer_modal: bool,
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
    last_autoplay_track_id: Option<String>,
    pull_refresh_px: f32,

    sample_tracks: Vec<Track>,
    sample_idx: usize,
    sample_drag_y: f32,
    sample_prewarmed: HashSet<String>,

    home_scroller: SmoothTouchScroller,
    search_scroller: SmoothTouchScroller,
    library_scroller: SmoothTouchScroller,
    detail_scroller: SmoothTouchScroller,
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

async fn prefetch_next_track_audio(
    dl: &DownloadManager,
    provider: &Arc<dyn MusicProvider>,
    audio: &AudioEngine,
    next_id: &str,
    quality: AudioQuality,
) {
    tokio::time::sleep(Duration::from_millis(650)).await;
    if dl.find_local_path(next_id).await.is_none() {
        if let Ok(stream) = provider.resolve_audio_stream(next_id, quality).await {
            audio.prefetch_url(stream.url, next_id.to_string());
        }
    }
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
            store.data.config.equalizer.clone(),
        );

        let cache_limit_bytes = (store.data.config.max_stream_cache_mb as u64) * 1024 * 1024;
        let stream_cache_bytes = prune_dir_to_max_bytes(&data_dir.join("stream_cache"), cache_limit_bytes);
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
            show_account_modal: false,
            queue_tuner: QueueTuner::All,
            settings_category: SettingsCategory::Root,
            is_video_mode: false,
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
            show_equalizer_modal: false,
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
            last_autoplay_track_id: None,
            pull_refresh_px: 0.0,
            sample_tracks: Vec::new(),
            sample_idx: 0,
            sample_drag_y: 0.0,
            sample_prewarmed: HashSet::new(),
            home_scroller: SmoothTouchScroller::default(),
            search_scroller: SmoothTouchScroller::default(),
            library_scroller: SmoothTouchScroller::default(),
            detail_scroller: SmoothTouchScroller::default(),
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
        if let Some(cookie) = app.store.data.google_cookie.clone() {
            let p = Arc::clone(&app.provider);
            let ev_tx = app.event_tx.clone();
            let egui_ctx = cc.egui_ctx.clone();
            app.home_feed.is_logged_in = true;
            app.home_feed.account = app.store.data.google_account.clone();
            app.runtime.spawn(async move {
                let res = p.login_with_cookie(&cookie).await.map_err(|e| e.to_string());
                let acc = p.get_account_info().await;
                let _ = ev_tx.send(AppEvent::GoogleLoginDone(res, Some(cookie), acc));
                egui_ctx.request_repaint();
            });
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
            if self.store.data.google_account.is_some() {
                cached_home.account = self.store.data.google_account.clone();
                cached_home.is_logged_in = true;
            }
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
            let res = match tokio::time::timeout(
                Duration::from_secs(16),
                provider.get_home_feed_with_params(mood_params.as_deref()),
            )
            .await
            {
                Ok(r) => r.map_err(|e| e.to_string()),
                Err(_) => Err("Tiempo de espera agotado al cargar inicio".to_string()),
            };
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
        let (track_opt, next_track_id, is_near_end) = {
            let q = self.queue.lock().unwrap();
            let curr = q.current_track().cloned();
            let ordered = q.ordered_tracks();
            let curr_pos = curr.as_ref().and_then(|c| {
                ordered.iter().position(|(_, t)| t.id == c.id)
            });
            let next_id = curr_pos
                .and_then(|p| ordered.get(p + 1))
                .map(|(_, t)| t.id.clone());
            let near_end = curr_pos.map(|p| p + 2 >= ordered.len()).unwrap_or(true);
            (curr, next_id, near_end)
        };
        let Some(track) = track_opt else {
            return;
        };
        if self.store.data.config.autoplay_radio && is_near_end {
            if self.last_autoplay_track_id.as_deref() != Some(&track.id) {
                self.last_autoplay_track_id = Some(track.id.clone());
                self.fetch_radio_internal(track.id.clone(), ctx);
            }
        }
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

        let dl = Arc::clone(&self.downloader);
        let provider = Arc::clone(&self.provider);
        let quality = self.store.data.config.stream_quality;
        let tx = self.event_tx.clone();
        let ctx_bg = ctx.clone();
        let audio = self.audio.clone();

        if let Some(local) = &track.local_path {
            let p = PathBuf::from(local);
            if p.exists() {
                self.audio.play_file(p, track.duration_secs);
                if let Some(nid) = next_track_id {
                    self.runtime.spawn(async move {
                        prefetch_next_track_audio(&dl, &provider, &audio, &nid, quality).await;
                    });
                }
                return;
            }
        }

        self.runtime.spawn(async move {
            if let Some(local_path) = dl.find_local_path(&track.id).await {
                audio.play_file(local_path, track.duration_secs);
                ctx_bg.request_repaint();
            } else {
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
            }

            if let Some(nid) = next_track_id {
                prefetch_next_track_audio(&dl, &provider, &audio, &nid, quality).await;
            }
        });
    }

    fn enqueue_tracks(&mut self, tracks: Vec<Track>, banner: String, ctx: &egui::Context) {
        let dl = Arc::clone(&self.downloader);
        let quality = self.store.data.config.download_quality;
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            for track in tracks {
                dl.enqueue(track, quality).await;
            }
            loop {
                let tasks = dl.list_tasks().await;
                let total_bytes = dl.total_downloaded_bytes().await;
                let has_active = tasks.iter().any(|d| {
                    matches!(
                        d.status,
                        DownloadStatus::Queued | DownloadStatus::Downloading
                    )
                });
                let _ = tx.send(AppEvent::DownloadsRefreshed { tasks, total_bytes });
                ctx.request_repaint();
                if !has_active {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        });
        self.set_banner(banner);
    }

    fn enqueue_download(&mut self, track: Track, ctx: &egui::Context) {
        let title = track.title.clone();
        self.enqueue_tracks(vec![track], format!("Descargando: {title}"), ctx);
    }

    fn enqueue_download_all(&mut self, tracks: Vec<Track>, ctx: &egui::Context) {
        let count = tracks.len();
        self.enqueue_tracks(tracks, format!("Descargando {count} canciones..."), ctx);
    }

    fn process_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.event_rx.try_recv() {
            match ev {
                AppEvent::HomeLoaded(res) => {
                    self.home_loading = false;
                    match res {
                        Ok(mut feed) => {
                            if feed.account.is_some() && self.store.data.google_cookie.is_some() {
                                self.store.data.google_account = feed.account.clone();
                                let _ = self.store.save();
                            } else if feed.account.is_none() && self.store.data.google_account.is_some() {
                                feed.account = self.store.data.google_account.clone();
                                feed.is_logged_in = true;
                            }
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
                AppEvent::GoogleLoginDone(res, cookie_opt, acc_opt) => {
                    self.google_login_loading = false;
                    match res {
                        Ok(()) => {
                            self.home_feed.is_logged_in = true;
                            if let Some(a) = acc_opt.clone() {
                                self.home_feed.account = Some(a);
                            }
                            if let Some(c) = cookie_opt {
                                self.store.set_google_session(Some(c), acc_opt);
                            } else if acc_opt.is_some() {
                                self.store.set_google_session(self.store.data.google_cookie.clone(), acc_opt);
                            }
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
        let in_downloads_view = self.active_tab == NavTab::Downloads
            || (self.active_tab == NavTab::Library
                && self.library_filter == LibraryFilter::Downloads);
        let poll_interval = if has_active_dl || in_downloads_view {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(4)
        };
        if self.last_download_poll.elapsed() >= poll_interval {
            self.refresh_downloads(ctx);
        }
        if has_active_dl {
            ctx.request_repaint_after(Duration::from_millis(200));
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
        let fullscreen_overlay = self.now_playing_open || self.show_settings;

        if !fullscreen_overlay {
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
                            self.render_account_button(ui, 28.0);
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
        let panel_frame = if (self.now_playing_open && !self.show_settings) || samples_full_bleed {
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
        self.render_equalizer_modal(ctx);
        self.render_playlist_picker_modal(ctx);
        self.render_account_modal(ctx);
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
        let elapsed = at.elapsed();
        let lifetime = Duration::from_secs(3);
        if elapsed >= lifetime {
            return;
        }
        ctx.request_repaint_after(lifetime.saturating_sub(elapsed) + Duration::from_millis(30));
        let max_w = (ctx.screen_rect().width() - 48.0).clamp(180.0, 420.0);
        let has_mini = self
            .queue
            .lock()
            .map(|q| q.current_track().is_some())
            .unwrap_or(false);
        let (anchor_align, anchor_offset) = if self.now_playing_open && !self.show_settings {
            (egui::Align2::CENTER_TOP, Vec2::new(0.0, 92.0))
        } else if has_mini {
            (egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -204.0))
        } else {
            (egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -138.0))
        };
        egui::Area::new(egui::Id::new("yr_status_toast"))
            .anchor(anchor_align, anchor_offset)
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

    fn apply_queue_tuner(&mut self, tuner: QueueTuner) {
        let liked_ids: HashSet<String> = self
            .store
            .data
            .liked_tracks
            .iter()
            .map(|t| t.id.clone())
            .collect();
        let recent_ids: HashSet<String> = self
            .store
            .data
            .recent_tracks
            .iter()
            .map(|t| t.id.clone())
            .collect();

        let mut q = self.queue.lock().unwrap();
        match tuner {
            QueueTuner::All => {}
            QueueTuner::Familiar => {
                q.reorder_upcoming_by(|t| {
                    if liked_ids.contains(&t.id) {
                        0
                    } else if recent_ids.contains(&t.id) {
                        1
                    } else {
                        2
                    }
                });
            }
            QueueTuner::Discovery => {
                q.reorder_upcoming_by(|t| {
                    if !liked_ids.contains(&t.id) && !recent_ids.contains(&t.id) {
                        0
                    } else {
                        1
                    }
                });
            }
            QueueTuner::Popular => {
                q.reorder_upcoming_by(|t| std::cmp::Reverse(t.duration_secs.unwrap_or(0)));
            }
            QueueTuner::Deep => {
                q.reorder_upcoming_by(|t| t.duration_secs.unwrap_or(0));
            }
        }
        drop(q);
        self.set_banner(format!("Cola afinada: {}", tuner.label()));
    }

    fn refresh_cache_stats(&mut self) {
        self.stream_cache_bytes = dir_size_bytes(&self.data_dir.join("stream_cache"));
    }

    fn is_at_root(&self) -> bool {
        !self.show_settings
            && !self.show_account_modal
            && !self.now_playing_open
            && matches!(self.detail_view, DetailView::None)
            && self.active_tab == NavTab::Home
            && self.action_sheet_track.is_none()
            && self.playlist_picker_track.is_none()
            && !self.show_sleep_timer_sheet
            && !self.show_equalizer_modal
            && !self.show_google_login
            && !self.show_new_playlist_dialog
    }

    fn pop_back_navigation(&mut self) {
        let has_search_results = !self.search_results.songs.is_empty()
            || !self.search_results.albums.is_empty()
            || !self.search_results.artists.is_empty()
            || !self.search_results.playlists.is_empty();

        if self.show_account_modal {
            self.show_account_modal = false;
        } else if self.show_equalizer_modal {
            self.show_equalizer_modal = false;
        } else if self.show_sleep_timer_sheet {
            self.show_sleep_timer_sheet = false;
        } else if self.action_sheet_track.take().is_some() {
            self.action_sheet_context = None;
        } else if self.playlist_picker_track.take().is_some() {
        } else if self.show_google_login {
            self.show_google_login = false;
        } else if self.show_new_playlist_dialog {
            self.show_new_playlist_dialog = false;
        } else if self.show_settings {
            if self.settings_category != SettingsCategory::Root {
                self.settings_category = SettingsCategory::Root;
            } else {
                self.show_settings = false;
            }
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
            let acc = if res.is_ok() {
                provider.get_account_info().await
            } else {
                None
            };
            let _ = tx.send(AppEvent::GoogleLoginDone(res, None, acc));
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
        let cookie_save = trimmed.clone();
        self.runtime.spawn(async move {
            let res = provider
                .login_with_cookie(&trimmed)
                .await
                .map_err(|e| e.to_string());
            let acc = provider.get_account_info().await;
            let _ = tx.send(AppEvent::GoogleLoginDone(res, Some(cookie_save), acc));
            ctx.request_repaint();
        });
    }

    fn logout_google(&mut self, ctx: &egui::Context) {
        let provider = self.provider.clone();
        let ctx_bg = ctx.clone();
        self.home_feed.is_logged_in = false;
        self.home_feed.account = None;
        self.store.clear_google_session();
        self.google_device_auth = None;
        self.google_auth_error = None;
        self.runtime.spawn(async move {
            let _ = provider.logout().await;
            ctx_bg.request_repaint();
        });
        self.set_banner("Sesión de Google cerrada");
        self.refresh_home(ctx);
    }

    fn render_account_button(&mut self, ui: &mut egui::Ui, size: f32) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
        if ui.is_rect_visible(rect) {
            let account_info = self
                .home_feed
                .account
                .as_ref()
                .or(self.store.data.google_account.as_ref());
            let avatar_url = account_info.and_then(|a| a.avatar_url.as_deref());
            let is_logged = self.home_feed.is_logged_in || self.store.data.google_cookie.is_some();
            let radius = CornerRadius::same((size * 0.5) as u8);

            ui.painter().rect_filled(
                rect,
                radius,
                if is_logged {
                    Color32::from_rgb(38, 38, 44)
                } else {
                    BG_ELEVATED
                },
            );

            let mut painted = false;
            if let Some(url) = avatar_url {
                let thumb_uri = tracked_thumb_uri(url, 192);
                let img = Image::new(thumb_uri)
                    .fit_to_exact_size(Vec2::splat(size))
                    .corner_radius(radius);
                match img.load_for_size(ui.ctx(), Vec2::splat(size)) {
                    Ok(egui::load::TexturePoll::Ready { .. }) => {
                        img.paint_at(ui, rect);
                        painted = true;
                    }
                    Ok(egui::load::TexturePoll::Pending { .. }) => {
                        ui.ctx().request_repaint();
                    }
                    Err(_) => {}
                }
            }

            if !painted {
                if is_logged {
                    let initial = account_info
                        .and_then(|a| a.name.chars().next())
                        .unwrap_or('G')
                        .to_uppercase()
                        .to_string();
                    let font_id = egui::FontId::proportional(size * 0.46);
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        initial,
                        font_id,
                        Color32::WHITE,
                    );
                } else {
                    icons::draw_icon(
                        ui.painter(),
                        egui::Rect::from_center_size(rect.center(), Vec2::splat(size * 0.58)),
                        icons::Icon::Account,
                        TEXT_SECONDARY,
                    );
                }
            }
        }
        if resp.clicked() {
            self.show_account_modal = true;
        }
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
                    "Descargar",
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
                    if icons::icon_button(
                        ui,
                        icons::Icon::ArrowBack,
                        36.0,
                        18.0,
                        BG_ELEVATED,
                        TEXT_PRIMARY,
                        18.0,
                    )
                    .clicked()
                    {
                        self.detail_view = DetailView::None;
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Álbum")
                            .size(15.0)
                            .color(TEXT_SECONDARY),
                    );
                });
                ui.add_space(6.0);
                let detail_viewport = ui.available_rect_before_wrap();
                self.detail_scroller.update(ui, detail_viewport, !self.detail_loading);
                let detail_scroll = egui::ScrollArea::vertical()
                    .vertical_scroll_offset(self.detail_scroller.offset_y)
                    .drag_to_scroll(false)
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            let (art_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(156.0), Sense::hover());
                            ui.painter().rect_filled(
                                art_rect,
                                CornerRadius::same(12),
                                BG_ELEVATED,
                            );
                            let note_rect = egui::Rect::from_center_size(
                                art_rect.center(),
                                Vec2::splat(52.0),
                            );
                            icons::draw_icon(
                                ui.painter(),
                                note_rect,
                                icons::Icon::MusicNote,
                                Color32::from_white_alpha(95),
                            );
                            if let Some(u) = album.summary.thumbnail_url.as_deref() {
                                paint_progressive_image(
                                    ui,
                                    art_rect,
                                    u,
                                    544,
                                    Vec2::splat(156.0),
                                    CornerRadius::same(12),
                                );
                            }
                            ui.add_space(10.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&album.summary.title)
                                        .size(19.0)
                                        .strong()
                                        .color(TEXT_PRIMARY),
                                )
                                .wrap()
                                .selectable(false),
                            );
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(&album.summary.artist)
                                    .size(14.0)
                                    .color(TEXT_SECONDARY),
                            );
                            let count = album.tracks.len();
                            let total_secs: u64 =
                                album.tracks.iter().filter_map(|t| t.duration_secs).sum();
                            let count_txt = if count == 1 {
                                "1 canción".to_string()
                            } else {
                                format!("{count} canciones")
                            };
                            let meta = if total_secs > 0 {
                                format!("{count_txt} • {}", format_seconds(total_secs))
                            } else {
                                count_txt
                            };
                            ui.label(RichText::new(meta).size(12.0).color(TEXT_SECONDARY));
                            ui.add_space(10.0);
                            self.render_collection_actions(ui, ctx, &album.tracks, true);
                        });
                        ui.add_space(10.0);
                        for (idx, track) in album.tracks.iter().enumerate() {
                            self.render_track_row(ui, ctx, track, &album.tracks, idx, false);
                        }
                    });
                self.detail_scroller.sync_bounds(detail_scroll.content_size.y, detail_scroll.inner_rect.height());
                if self.handle_pull_to_refresh(
                    ui,
                    self.detail_scroller.offset_y,
                    detail_viewport,
                    self.detail_loading,
                ) {
                    self.open_album(album.summary.id.clone(), ctx);
                    self.set_banner("Actualizando álbum...");
                }
                self.render_pull_refresh_overlay(ctx, detail_viewport, self.detail_loading);
                true
            }
            DetailView::Playlist(pl) => {
                ui.horizontal(|ui| {
                    if icons::icon_button(
                        ui,
                        icons::Icon::ArrowBack,
                        36.0,
                        18.0,
                        BG_ELEVATED,
                        TEXT_PRIMARY,
                        18.0,
                    )
                    .clicked()
                    {
                        self.detail_view = DetailView::None;
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Playlist")
                            .size(15.0)
                            .color(TEXT_SECONDARY),
                    );
                });
                ui.add_space(6.0);
                let detail_viewport = ui.available_rect_before_wrap();
                self.detail_scroller.update(ui, detail_viewport, !self.detail_loading);
                let detail_scroll = egui::ScrollArea::vertical()
                    .vertical_scroll_offset(self.detail_scroller.offset_y)
                    .drag_to_scroll(false)
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            let (art_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(156.0), Sense::hover());
                            ui.painter().rect_filled(
                                art_rect,
                                CornerRadius::same(12),
                                BG_ELEVATED,
                            );
                            let note_rect = egui::Rect::from_center_size(
                                art_rect.center(),
                                Vec2::splat(52.0),
                            );
                            icons::draw_icon(
                                ui.painter(),
                                note_rect,
                                icons::Icon::MusicNote,
                                Color32::from_white_alpha(95),
                            );
                            let cover_url = pl.summary.thumbnail_url.as_deref().or_else(|| {
                                pl.tracks
                                    .first()
                                    .and_then(|t| t.thumbnail_url.as_deref())
                            });
                            if let Some(u) = cover_url {
                                paint_progressive_image(
                                    ui,
                                    art_rect,
                                    u,
                                    544,
                                    Vec2::splat(156.0),
                                    CornerRadius::same(12),
                                );
                            }
                            ui.add_space(10.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&pl.summary.title)
                                        .size(19.0)
                                        .strong()
                                        .color(TEXT_PRIMARY),
                                )
                                .wrap()
                                .selectable(false),
                            );
                            if let Some(author) = pl.summary.author.as_deref().filter(|s| !s.is_empty()) {
                                ui.add_space(2.0);
                                ui.label(
                                    RichText::new(author)
                                        .size(14.0)
                                        .color(TEXT_SECONDARY),
                                );
                            }
                            let count = pl.tracks.len();
                            let total_secs: u64 =
                                pl.tracks.iter().filter_map(|t| t.duration_secs).sum();
                            let count_txt = if count == 1 {
                                "1 canción".to_string()
                            } else {
                                format!("{count} canciones")
                            };
                            let meta = if total_secs > 0 {
                                format!("{count_txt} • {}", format_seconds(total_secs))
                            } else {
                                count_txt
                            };
                            ui.label(RichText::new(meta).size(12.0).color(TEXT_SECONDARY));
                            ui.add_space(10.0);
                            self.render_collection_actions(ui, ctx, &pl.tracks, true);
                        });
                        ui.add_space(10.0);
                        for (idx, track) in pl.tracks.iter().enumerate() {
                            self.render_track_row(ui, ctx, track, &pl.tracks, idx, false);
                        }
                    });
                self.detail_scroller.sync_bounds(detail_scroll.content_size.y, detail_scroll.inner_rect.height());
                if self.handle_pull_to_refresh(
                    ui,
                    self.detail_scroller.offset_y,
                    detail_viewport,
                    self.detail_loading,
                ) {
                    self.open_playlist(pl.summary.id.clone(), ctx);
                    self.set_banner("Actualizando playlist...");
                }
                self.render_pull_refresh_overlay(ctx, detail_viewport, self.detail_loading);
                true
            }
            DetailView::Artist(artist) => {
                ui.horizontal(|ui| {
                    if icons::icon_button(
                        ui,
                        icons::Icon::ArrowBack,
                        36.0,
                        18.0,
                        BG_ELEVATED,
                        TEXT_PRIMARY,
                        18.0,
                    )
                    .clicked()
                    {
                        self.detail_view = DetailView::None;
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Artista")
                            .size(15.0)
                            .color(TEXT_SECONDARY),
                    );
                });
                ui.add_space(6.0);
                let detail_viewport = ui.available_rect_before_wrap();
                self.detail_scroller.update(ui, detail_viewport, !self.detail_loading);
                let detail_scroll = egui::ScrollArea::vertical()
                    .vertical_scroll_offset(self.detail_scroller.offset_y)
                    .drag_to_scroll(false)
                    .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        let (av_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(116.0), Sense::hover());
                        ui.painter()
                            .circle_filled(av_rect.center(), 58.0, BG_ELEVATED);
                        icons::draw_icon(
                            ui.painter(),
                            egui::Rect::from_center_size(av_rect.center(), Vec2::splat(44.0)),
                            icons::Icon::Account,
                            TEXT_SECONDARY,
                        );
                        let avatar_url = artist.summary.thumbnail_url.as_deref().or_else(|| {
                            artist
                                .top_tracks
                                .first()
                                .and_then(|t| t.thumbnail_url.as_deref())
                        });
                        if let Some(u) = avatar_url {
                            paint_progressive_image(
                                ui,
                                av_rect,
                                u,
                                544,
                                Vec2::splat(116.0),
                                CornerRadius::same(58),
                            );
                        }
                        ui.add_space(10.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(&artist.summary.name)
                                    .size(21.0)
                                    .strong()
                                    .color(TEXT_PRIMARY),
                            )
                            .wrap()
                            .selectable(false),
                        );
                        if let Some(subs) = &artist.summary.subscribers {
                            ui.add_space(2.0);
                            ui.label(RichText::new(subs).size(13.0).color(TEXT_SECONDARY));
                        }
                        if !artist.top_tracks.is_empty() {
                            ui.add_space(10.0);
                            self.render_collection_actions(ui, ctx, &artist.top_tracks, false);
                        }
                    });
                    if !artist.top_tracks.is_empty() {
                        ui.add_space(12.0);
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
                self.detail_scroller.sync_bounds(detail_scroll.content_size.y, detail_scroll.inner_rect.height());
                if self.handle_pull_to_refresh(
                    ui,
                    self.detail_scroller.offset_y,
                    detail_viewport,
                    self.detail_loading,
                ) {
                    self.open_artist(artist.summary.id.clone(), ctx);
                    self.set_banner("Actualizando artista...");
                }
                self.render_pull_refresh_overlay(ctx, detail_viewport, self.detail_loading);
                true
            }
            DetailView::Mood { title, playlists } => {
                ui.horizontal(|ui| {
                    if icons::icon_button(
                        ui,
                        icons::Icon::ArrowBack,
                        36.0,
                        18.0,
                        BG_ELEVATED,
                        TEXT_PRIMARY,
                        18.0,
                    )
                    .clicked()
                    {
                        self.detail_view = DetailView::None;
                    }
                    ui.add_space(6.0);
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
        let is_downloaded = self
            .downloads
            .iter()
            .any(|d| d.track.id == track.id && d.status == DownloadStatus::Completed);

        let row_h = 54.0;
        let more_btn_w = 34.0;
        let body_w = (col_width - more_btn_w - 4.0).max(120.0);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;

            // Interactive track body (Thumbnail + Title + Subtitle)
            let (_, body_rect) = ui.allocate_space(Vec2::new(body_w, row_h));
            let row_id = ui.make_persistent_id(("ytm_compact_row", &track.id, index));
            let body_resp = ui.interact(body_rect, row_id, Sense::click());
            if ui.is_rect_visible(body_rect) {
                let bg = if is_current {
                    Color32::from_rgb(32, 28, 30)
                } else if body_resp.hovered() {
                    Color32::from_white_alpha(18)
                } else {
                    Color32::TRANSPARENT
                };
                if bg != Color32::TRANSPARENT {
                    ui.painter().rect_filled(body_rect, CornerRadius::same(8), bg);
                }

                let thumb_sz = 46.0;
                let thumb_rect = Rect::from_min_size(
                    Pos2::new(body_rect.left() + 2.0, body_rect.center().y - thumb_sz * 0.5),
                    Vec2::splat(thumb_sz),
                );
                let radius = CornerRadius::same(4);
                let resolved_thumb = track_thumb_url(track.thumbnail_url.as_deref(), &track.id);
                let mut thumb_painted = false;
                if let Some(url) = resolved_thumb {
                    let thumb_uri = tracked_thumb_uri(&url, 160);
                    let img = Image::new(thumb_uri)
                        .fit_to_exact_size(Vec2::splat(thumb_sz))
                        .corner_radius(radius);
                    if let Ok(egui::load::TexturePoll::Ready { .. }) = img.load_for_size(ui.ctx(), Vec2::splat(thumb_sz)) {
                        img.paint_at(ui, thumb_rect);
                        thumb_painted = true;
                    }
                }
                if !thumb_painted {
                    ui.painter().rect_filled(thumb_rect, radius, Color32::from_rgb(30, 30, 34));
                    let icon_r = Rect::from_center_size(thumb_rect.center(), Vec2::splat(20.0));
                    icons::draw_icon(ui.painter(), icon_r, icons::Icon::Play, Color32::from_white_alpha(160));
                }

                let text_left = thumb_rect.right() + 8.0;
                let text_w = (body_rect.right() - text_left - 4.0).max(60.0);
                let title_color = if is_current { ACCENT_RED } else { TEXT_PRIMARY };

                let title_job = clamped_layout_job(&track.title, 14.0, title_color, text_w, 1);
                let galley_title = ui.painter().layout_job(title_job);

                let sub_txt = if subtitle.is_empty() {
                    track.artist.clone()
                } else {
                    subtitle.to_string()
                };
                let sub_color = if is_downloaded { GREEN_OK } else { TEXT_SECONDARY };
                let sub_job = clamped_layout_job(&sub_txt, 12.0, sub_color, text_w, 1);
                let galley_sub = ui.painter().layout_job(sub_job);

                let gap = 3.0;
                let total_h = galley_title.size().y + gap + galley_sub.size().y;
                let title_y = (body_rect.center().y - total_h * 0.5).max(body_rect.top() + 2.0);
                let sub_y = title_y + galley_title.size().y + gap;
                ui.painter().galley(Pos2::new(text_left, title_y), galley_title, title_color);
                ui.painter().galley(Pos2::new(text_left, sub_y), galley_sub, sub_color);
            }

            if body_resp.clicked() {
                self.play_track_in_list(list.to_vec(), index, ctx);
            }

            // More Options (3 vertical dots)
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
                        ui.push_id((salt, "chunk", chunk_idx), |ui| {
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
        let draw_page = |ui: &mut egui::Ui, page_idx: usize, page: &[HomeShelfCard], app: &mut Self| {
            ui.vertical(|ui| {
                ui.set_width(page_width);
                ui.spacing_mut().item_spacing.y = gap;
                for (row_idx, row) in page.chunks(3).enumerate() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for (col_idx, card) in row.iter().enumerate() {
                            let (_, rect) = ui.allocate_space(Vec2::splat(tile_size));
                            let tile_id = ui.make_persistent_id((
                                salt,
                                "sd_tile",
                                page_idx,
                                row_idx,
                                col_idx,
                                &card.title,
                            ));
                            let resp = ui.interact(rect, tile_id, Sense::click());
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
                draw_page(ui, 0, page, self);
            }
        } else {
            let hscroll = egui::ScrollArea::horizontal()
                .id_salt(salt)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        for (page_idx, page) in pages.iter().enumerate() {
                            draw_page(ui, page_idx, page, self);
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
                        let card_h = card_w + 46.0;
                        let (_, card_rect) = ui.allocate_space(Vec2::new(card_w, card_h));
                        let card_id = ui.make_persistent_id((salt, "carousel_card", cidx, &card.title));
                        let card_resp = ui.interact(card_rect, card_id, Sense::click());
                        if ui.is_rect_visible(card_rect) {
                            let thumb_rect = Rect::from_min_size(card_rect.min, Vec2::splat(card_w));
                            ui.painter().rect_filled(thumb_rect, CornerRadius::same(8), BG_SURFACE);
                            let note_rect = egui::Rect::from_center_size(
                                thumb_rect.center(),
                                Vec2::splat((card_w * 0.44).clamp(18.0, 44.0)),
                            );
                            icons::draw_icon(
                                ui.painter(),
                                note_rect,
                                icons::Icon::MusicNote,
                                Color32::from_white_alpha(95),
                            );
                            if let Some(url) = card.thumbnail_url.as_deref() {
                                let _ = paint_ready_image(
                                    ui,
                                    thumb_rect,
                                    tracked_thumb_uri(url, 224),
                                    Vec2::splat(card_w),
                                    CornerRadius::same(8),
                                );
                            }
                            let title_job = clamped_layout_job(&card.title, 13.5, TEXT_PRIMARY, card_w, 1);
                            let g_title = ui.painter().layout_job(title_job);
                            ui.painter().galley(
                                Pos2::new(card_rect.left(), thumb_rect.bottom() + 4.0),
                                g_title,
                                TEXT_PRIMARY,
                            );
                            if !card.subtitle.is_empty() {
                                let sub_job = clamped_layout_job(&card.subtitle, 11.5, TEXT_SECONDARY, card_w, 1);
                                let g_sub = ui.painter().layout_job(sub_job);
                                ui.painter().galley(
                                    Pos2::new(card_rect.left(), thumb_rect.bottom() + 22.0),
                                    g_sub,
                                    TEXT_SECONDARY,
                                );
                            }
                        }
                        if card_resp.clicked() {
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

    fn handle_pull_to_refresh(
        &mut self,
        ui: &egui::Ui,
        scroll_offset_y: f32,
        viewport_rect: egui::Rect,
        is_loading: bool,
    ) -> bool {
        if scroll_offset_y > 4.0 {
            self.pull_refresh_px = 0.0;
            return false;
        }
        let (down, in_view, delta) = ui.input(|i| {
            (
                i.pointer.any_down(),
                i.pointer.press_origin().is_some_and(|p| viewport_rect.contains(p)),
                i.pointer.delta(),
            )
        });
        if down && in_view {
            if delta.y > 0.0 && delta.y >= delta.x.abs() * 0.5 {
                self.pull_refresh_px = (self.pull_refresh_px + delta.y * 0.75).min(110.0);
                ui.ctx().request_repaint();
            } else if delta.y < -1.0 {
                self.pull_refresh_px = (self.pull_refresh_px + delta.y).max(0.0);
                ui.ctx().request_repaint();
            }
            false
        } else {
            let triggered = self.pull_refresh_px >= 38.0 && !is_loading;
            self.pull_refresh_px = 0.0;
            triggered
        }
    }

    fn render_pull_refresh_overlay(
        &self,
        ctx: &egui::Context,
        viewport_rect: egui::Rect,
        is_loading: bool,
    ) {
        if self.pull_refresh_px <= 6.0 && !is_loading {
            return;
        }
        let top_y = viewport_rect.top() + (self.pull_refresh_px * 0.55).clamp(10.0, 56.0);
        let center = Pos2::new(viewport_rect.center().x, top_y);
        let badge_rect = Rect::from_center_size(center, Vec2::new(184.0, 36.0));
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("yr_pull_refresh_badge"),
        ));
        painter.rect_filled(badge_rect, CornerRadius::same(18), Color32::from_rgb(32, 32, 36));
        painter.rect_stroke(
            badge_rect,
            CornerRadius::same(18),
            Stroke::new(1.0_f32, Color32::from_rgb(60, 60, 68)),
            egui::StrokeKind::Outside,
        );

        let icon_center = Pos2::new(badge_rect.left() + 20.0, center.y);
        let text_center = Pos2::new(badge_rect.left() + 104.0, center.y);

        if is_loading {
            let time = ctx.input(|i| i.time);
            let angle = (time * 6.0) as f32;
            let radius = 6.0;
            for i in 0..4 {
                let a = angle + i as f32 * std::f32::consts::FRAC_PI_2;
                let pt = icon_center + Vec2::new(a.cos() * radius, a.sin() * radius);
                let alpha = (60 + i * 50) as u8;
                painter.circle_filled(pt, 1.8, Color32::from_rgba_unmultiplied(255, 255, 255, alpha));
            }
            painter.text(
                text_center,
                egui::Align2::CENTER_CENTER,
                "Actualizando canciones...",
                egui::FontId::proportional(12.5),
                Color32::WHITE,
            );
        } else if self.pull_refresh_px >= 38.0 {
            painter.circle_filled(icon_center, 5.0, ACCENT_RED);
            painter.text(
                text_center,
                egui::Align2::CENTER_CENTER,
                "Suelta para actualizar",
                egui::FontId::proportional(12.5),
                Color32::WHITE,
            );
        } else {
            let frac = (self.pull_refresh_px / 38.0).clamp(0.0, 1.0);
            painter.circle_filled(icon_center, 4.0 * frac, TEXT_SECONDARY);
            painter.text(
                text_center,
                egui::Align2::CENTER_CENTER,
                "Desliza para actualizar",
                egui::FontId::proportional(12.5),
                TEXT_SECONDARY,
            );
        }
    }

    fn render_home(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let is_loading = self.home_loading;
        let home_viewport = ui.available_rect_before_wrap();
        self.home_scroller.update(ui, home_viewport, !is_loading);
        let scroll_out = egui::ScrollArea::vertical()
            .id_salt("home_vertical_scroll")
            .vertical_scroll_offset(self.home_scroller.offset_y)
            .drag_to_scroll(false)
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
            } else if home_empty && self.home_feed.quick_picks.is_empty() {
                ui.add_space(8.0);
                egui::Frame::new()
                    .fill(BG_SURFACE)
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(egui::Margin::symmetric(14, 14))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("No se pudo cargar el inicio")
                                        .size(14.5)
                                        .strong()
                                        .color(Color32::WHITE),
                                );
                                ui.label(
                                    RichText::new("Revisa tu conexión e inténtalo de nuevo")
                                        .size(12.0)
                                        .color(TEXT_SECONDARY),
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Refresh,
                                    "Reintentar",
                                    ACCENT_RED,
                                    Color32::WHITE,
                                )
                                .clicked()
                                {
                                    self.refresh_home(ctx);
                                }
                            });
                        });
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
        });
        self.home_scroller.sync_bounds(scroll_out.content_size.y, scroll_out.inner_rect.height());
        if self.handle_pull_to_refresh(
            ui,
            self.home_scroller.offset_y,
            home_viewport,
            is_loading,
        ) {
            self.refresh_home(ctx);
            self.load_explore(ctx);
            self.set_banner("Actualizando lista de canciones...");
        }
        self.render_pull_refresh_overlay(ctx, home_viewport, is_loading);
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

        let sample_thumb = track_thumb_url(track.thumbnail_url.as_deref(), &track.id);
        if let Some(url) = sample_thumb.as_deref() {
            self.ensure_cover_color(Some(url), ctx);
        }
        self.prewarm_sample_neighbor(idx);
        if idx + 1 < total_samples {
            self.prewarm_sample_neighbor(idx + 1);
        }

        let ambient = sample_thumb
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
            let body_h = (ui.available_height() - 98.0).max(160.0);
            ui.add_space(8.0);
            ui.allocate_ui(Vec2::new(inner_rect.width(), body_h), |ui| {
                ui.set_min_height(body_h);
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
                            if is_playing {
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
                            }
                            if let Some(url) = sample_thumb.as_deref() {
                                paint_progressive_image(
                                    ui,
                                    visual_art_rect,
                                    url,
                                    720,
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

        let search_viewport = ui.available_rect_before_wrap();
        let search_loading = self.search_loading || self.explore_loading;
        self.search_scroller.update(ui, search_viewport, !search_loading);
        let search_scroll = egui::ScrollArea::vertical()
            .id_salt("search_scroll")
            .vertical_scroll_offset(self.search_scroller.offset_y)
            .drag_to_scroll(false)
            .show(ui, |ui| {
                let mut shown_local_ids: HashSet<String> = HashSet::new();
                let q_trimmed = self.search_query.trim().to_string();
                if q_trimmed.len() >= 2 && self.search_filter == SearchFilter::Songs {
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
                            RichText::new("En tu biblioteca")
                                .size(13.0)
                                .strong()
                                .color(TEXT_SECONDARY),
                        );
                        ui.add_space(4.0);
                        for (idx, t) in local_hits.iter().take(4).enumerate() {
                            shown_local_ids.insert(t.id.clone());
                            self.render_track_row(ui, ctx, t, &local_hits, idx, false);
                            ui.add_space(3.0);
                        }
                    }
                }

                if !has_results && !self.search_loading {
                    if !self.search_query.trim().is_empty() && !self.search_suggestions.is_empty() {
                        ui.add_space(6.0);
                        let suggestions = self.search_suggestions.clone();
                        let history_set: HashSet<String> = self.store.data.search_history.iter().cloned().collect();
                        for (idx, s) in suggestions.iter().take(12).enumerate() {
                            ui.horizontal(|ui| {
                                let row_start = ui.cursor().min;
                                let is_hist = history_set.contains(s);
                                let (icon_r, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
                                icons::draw_icon(
                                    ui.painter(),
                                    egui::Rect::from_center_size(icon_r.center(), Vec2::splat(16.0)),
                                    if is_hist { icons::Icon::History } else { icons::Icon::Search },
                                    TEXT_SECONDARY,
                                );
                                let text_w = (ui.available_width() - 44.0).max(120.0);
                                ui.allocate_ui(Vec2::new(text_w, 38.0), |ui| {
                                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(s)
                                                    .size(15.0)
                                                    .color(TEXT_PRIMARY),
                                            )
                                            .truncate()
                                            .selectable(false),
                                        );
                                    });
                                });
                                let hit = ui.interact(
                                    egui::Rect::from_min_size(row_start, Vec2::new(text_w + 32.0, 38.0)),
                                    ui.id().with(("sug_v_row", idx, s.as_str())),
                                    Sense::click(),
                                );
                                if hit.clicked() {
                                    self.search_query = s.clone();
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
                                        self.search_query = s.clone();
                                        self.last_search_input_at = Some(Instant::now());
                                    }
                                });
                            });
                            ui.add_space(2.0);
                        }
                        return;
                    }

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
                                            let lbl_resp = ui.add(
                                                egui::Label::new(
                                                    RichText::new(q)
                                                        .size(12.0)
                                                        .color(TEXT_PRIMARY),
                                                )
                                                .truncate()
                                                .selectable(false)
                                                .sense(Sense::click()),
                                            );
                                            if r.clicked() || lbl_resp.clicked() {
                                                self.search_query = q.clone();
                                                self.trigger_search(ctx);
                                            }
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
                            if render_artist_card_row(
                                ui,
                                ("artist_search_row", &artist.id),
                                artist.thumbnail_url.as_deref(),
                                &artist.name,
                                artist.subscribers.as_deref(),
                                icons::Icon::Account,
                            ) {
                                self.open_artist(artist.id.clone(), ctx);
                            }
                            ui.add_space(4.0);
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
        self.search_scroller.sync_bounds(search_scroll.content_size.y, search_scroll.inner_rect.height());
        if self.handle_pull_to_refresh(
            ui,
            self.search_scroller.offset_y,
            search_viewport,
            search_loading,
        ) {
            if self.search_query.trim().is_empty() {
                self.load_explore(ctx);
                self.refresh_home(ctx);
                self.set_banner("Actualizando tendencias y categorías...");
            } else {
                self.trigger_search(ctx);
                self.set_banner("Actualizando resultados...");
            }
        }
        self.render_pull_refresh_overlay(ctx, search_viewport, search_loading);
    }

    fn render_album_row(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        album: &yr_core::AlbumSummary,
    ) {
        let sub = match album.year {
            Some(y) if y > 0 => format!("{} • {y}", album.artist),
            _ => album.artist.clone(),
        };
        if render_collection_row(
            ui,
            ("album_row", &album.id),
            album.thumbnail_url.as_deref(),
            &album.title,
            Some(sub.as_str()),
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
        let sub = match (pl.author.as_deref(), pl.track_count) {
            (Some(a), Some(c)) if !a.is_empty() && c > 0 => format!("{a} • {c} canciones"),
            (Some(a), _) if !a.is_empty() => a.to_string(),
            (_, Some(c)) if c > 0 => format!("Playlist • {c} canciones"),
            _ => "Playlist de YouTube Music".to_string(),
        };
        if render_collection_row(
            ui,
            ("playlist_row", &pl.id),
            pl.thumbnail_url.as_deref(),
            &pl.title,
            Some(sub.as_str()),
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
                if icons::icon_button(
                    ui,
                    icons::Icon::Search,
                    32.0,
                    16.0,
                    Color32::TRANSPARENT,
                    TEXT_PRIMARY,
                    16.0,
                )
                .clicked()
                {
                    self.active_tab = NavTab::Search;
                }
                let hist_active = self.library_filter == LibraryFilter::History;
                if icons::icon_button(
                    ui,
                    icons::Icon::History,
                    32.0,
                    16.0,
                    if hist_active { ACCENT_RED } else { Color32::TRANSPARENT },
                    if hist_active { Color32::WHITE } else { TEXT_PRIMARY },
                    16.0,
                )
                .clicked()
                {
                    self.library_filter = if hist_active {
                        LibraryFilter::All
                    } else {
                        LibraryFilter::History
                    };
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
                        (LibraryFilter::History, "Historial"),
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
                .stroke(Stroke::new(0.8_f32, Color32::from_rgb(48, 48, 56)))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let input_w = (ui.available_width() - 134.0).max(110.0);
                        egui::Frame::new()
                            .fill(BG_ELEVATED)
                            .corner_radius(CornerRadius::same(18))
                            .inner_margin(egui::Margin::symmetric(12, 6))
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.new_playlist_name)
                                        .desired_width(input_w)
                                        .frame(false)
                                        .hint_text("Nombre de la playlist..."),
                                );
                            });
                        let can_create = !self.new_playlist_name.trim().is_empty();
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Crear")
                                        .size(13.0)
                                        .strong()
                                        .color(Color32::WHITE),
                                )
                                .fill(if can_create {
                                    ACCENT_RED
                                } else {
                                    BG_ELEVATED
                                })
                                .corner_radius(CornerRadius::same(16))
                                .min_size(Vec2::new(64.0, 32.0)),
                            )
                            .clicked()
                            && can_create
                        {
                            let name = self.new_playlist_name.trim().to_string();
                            self.store.create_playlist(name.clone());
                            self.new_playlist_name.clear();
                            self.show_new_playlist_dialog = false;
                            self.set_banner(format!("Playlist creada: {name}"));
                        }
                        if icons::icon_button(
                            ui,
                            icons::Icon::Close,
                            28.0,
                            14.0,
                            BG_ELEVATED,
                            TEXT_SECONDARY,
                            14.0,
                        )
                        .clicked()
                        {
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

        let lib_loading = self.home_loading;
        let lib_viewport = ui.available_rect_before_wrap();
        self.library_scroller.update(ui, lib_viewport, !lib_loading);
        let lib_scroll = egui::ScrollArea::vertical()
            .id_salt("lib_scroll")
            .vertical_scroll_offset(self.library_scroller.offset_y)
            .drag_to_scroll(false)
            .show(ui, |ui| {
                if self.library_filter == LibraryFilter::History {
                    let recent: Vec<Track> = self
                        .store
                        .data
                        .recent_tracks
                        .iter()
                        .filter(|t| lib_matches(&t.title) || lib_matches(&t.artist))
                        .cloned()
                        .collect();
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Historial ({})", recent.len()))
                                .size(16.0)
                                .strong(),
                        );
                        if !recent.is_empty() {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Trash,
                                    "Limpiar",
                                    BG_ELEVATED,
                                    TEXT_PRIMARY,
                                )
                                .clicked()
                                {
                                    self.store.data.recent_tracks.clear();
                                    let _ = self.store.save();
                                    self.set_banner("Historial de reproducciones borrado");
                                }
                            });
                        }
                    });
                    ui.add_space(6.0);
                    if recent.is_empty() {
                        egui::Frame::new()
                            .fill(BG_SURFACE)
                            .corner_radius(CornerRadius::same(14))
                            .inner_margin(20.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.vertical_centered(|ui| {
                                    let (ir, _) = ui.allocate_exact_size(
                                        Vec2::splat(34.0),
                                        Sense::hover(),
                                    );
                                    icons::draw_icon(
                                        ui.painter(),
                                        ir,
                                        icons::Icon::History,
                                        TEXT_SECONDARY,
                                    );
                                    ui.add_space(6.0);
                                    ui.label(
                                        RichText::new("Sin historial reciente")
                                            .size(15.0)
                                            .strong(),
                                    );
                                    ui.label(
                                        RichText::new(
                                            "Las canciones que reproduzcas aparecerán aquí.",
                                        )
                                        .small()
                                        .color(TEXT_SECONDARY),
                                    );
                                });
                            });
                    } else {
                        for (idx, track) in recent.iter().enumerate() {
                            self.render_track_row(ui, ctx, track, &recent, idx, false);
                        }
                    }
                    return;
                }

                // Pinned "Música que te gustó" gradient card
                if matches!(self.library_filter, LibraryFilter::All | LibraryFilter::Playlists | LibraryFilter::Songs) {
                    let liked_count = self.store.data.liked_tracks.len();
                    egui::Frame::new()
                        .fill(Color32::from_rgb(20, 20, 23))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let card_row_start = ui.cursor().min;
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
                                let hit_rect = egui::Rect::from_min_size(
                                    card_row_start,
                                    Vec2::new(54.0 + 8.0 + text_w, 54.0),
                                );
                                if ui
                                    .interact(
                                        hit_rect,
                                        ui.id().with("liked_hero_card"),
                                        Sense::click(),
                                    )
                                    .clicked()
                                {
                                    if liked_count > 0 {
                                        self.play_track_in_list(
                                            self.store.data.liked_tracks.clone(),
                                            0,
                                            ctx,
                                        );
                                    } else {
                                        self.set_banner(
                                            "Marca canciones con Me gusta para verlas aquí",
                                        );
                                    }
                                }
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

                if matches!(self.library_filter, LibraryFilter::All | LibraryFilter::Songs) {
                    let completed: Vec<Track> = self
                        .downloads
                        .iter()
                        .filter(|d| {
                            d.status == DownloadStatus::Completed
                                && (lib_matches(&d.track.title) || lib_matches(&d.track.artist))
                        })
                        .map(|d| d.track.clone())
                        .collect();
                    if !completed.is_empty() {
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("Descargas offline ({})", completed.len()))
                                    .size(14.5)
                                    .strong()
                                    .color(GREEN_OK),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
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
                        ui.add_space(4.0);
                        for (idx, t) in completed.iter().enumerate() {
                            self.render_track_row(ui, ctx, t, &completed, idx, true);
                        }
                    }

                    if self.library_filter == LibraryFilter::Songs {
                        let mut seen_ids: HashSet<String> = self
                            .store
                            .data
                            .liked_tracks
                            .iter()
                            .map(|t| t.id.clone())
                            .chain(completed.iter().map(|t| t.id.clone()))
                            .collect();
                        let recent_extra: Vec<Track> = self
                            .store
                            .data
                            .recent_tracks
                            .iter()
                            .filter(|t| {
                                (lib_matches(&t.title) || lib_matches(&t.artist))
                                    && seen_ids.insert(t.id.clone())
                            })
                            .cloned()
                            .collect();
                        if !recent_extra.is_empty() {
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new("Escuchadas recientemente")
                                    .size(16.0)
                                    .strong(),
                            );
                            ui.add_space(4.0);
                            for (idx, t) in recent_extra.iter().enumerate() {
                                self.render_track_row(ui, ctx, t, &recent_extra, idx, false);
                            }
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
                    ui.add_space(4.0);
                    let mut seen_artists = std::collections::HashSet::new();
                    let mut artists = Vec::new();
                    for t in self
                        .store
                        .data
                        .liked_tracks
                        .iter()
                        .chain(self.store.data.recent_tracks.iter())
                        .chain(self.downloads.iter().map(|d| &d.track))
                        .chain(self.home_feed.quick_picks.iter())
                    {
                        if let Some(aid) = t.artist_id.as_ref() {
                            if lib_matches(&t.artist) && seen_artists.insert(aid.clone()) {
                                let thumb = track_thumb_url(t.thumbnail_url.as_deref(), &t.id);
                                artists.push((aid.clone(), t.artist.clone(), thumb));
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
                        if render_artist_card_row(
                            ui,
                            ("lib_artist", &aid),
                            thumb.as_deref(),
                            &name,
                            Some("Artista • Biblioteca"),
                            icons::Icon::Artist,
                        ) {
                            self.open_artist(aid, ctx);
                        }
                        ui.add_space(4.0);
                    }
                }
                ui.add_space(72.0);
            });
        self.library_scroller.sync_bounds(lib_scroll.content_size.y, lib_scroll.inner_rect.height());
        if self.handle_pull_to_refresh(
            ui,
            self.library_scroller.offset_y,
            lib_viewport,
            lib_loading,
        ) {
            self.refresh_downloads(ctx);
            self.refresh_home(ctx);
            self.set_banner("Actualizando biblioteca...");
        }
        self.render_pull_refresh_overlay(ctx, lib_viewport, lib_loading);

        // Floating "+ Nueva" pill button in bottom right on playlist-capable tabs
        if matches!(
            self.library_filter,
            LibraryFilter::All | LibraryFilter::Playlists
        ) {
            let screen = ctx.screen_rect();
            let fab_x = screen.right() - 120.0;
            let has_track = self
                .queue
                .lock()
                .map(|q| q.current_track().is_some())
                .unwrap_or(false);
            #[cfg(target_os = "android")]
            let clearance = if has_track { 234.0 } else { 158.0 };
            #[cfg(not(target_os = "android"))]
            let clearance = if has_track { 168.0 } else { 96.0 };
            let fab_y = screen.bottom() - clearance;
            egui::Area::new(egui::Id::new("yr_lib_fab"))
                .fixed_pos(egui::pos2(fab_x, fab_y))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let (rect, resp) =
                        ui.allocate_exact_size(Vec2::new(104.0, 40.0), Sense::click());
                    if ui.is_rect_visible(rect) {
                        let bg = if resp.hovered() {
                            Color32::from_rgb(235, 235, 240)
                        } else {
                            Color32::WHITE
                        };
                        ui.painter().rect_filled(rect, CornerRadius::same(20), bg);
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            "+ Nueva",
                            egui::FontId::proportional(14.0),
                            Color32::BLACK,
                        );
                    }
                    if resp.clicked() {
                        self.show_new_playlist_dialog = !self.show_new_playlist_dialog;
                    }
                });
        }
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

        if !completed.is_empty() {
            ui.horizontal(|ui| {
                if icons::icon_pill_button(
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
            });
        }

        let dl_viewport = ui.available_rect_before_wrap();
        let dl_scroll = egui::ScrollArea::vertical()
            .drag_to_scroll(true)
            .show(ui, |ui| {
            if self.downloads.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(36.0);
                    let (circle_r, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
                    ui.painter()
                        .circle_filled(circle_r.center(), 28.0, BG_ELEVATED);
                    icons::draw_icon(
                        ui.painter(),
                        egui::Rect::from_center_size(circle_r.center(), Vec2::splat(26.0)),
                        icons::Icon::Download,
                        TEXT_SECONDARY,
                    );
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Sin descargas offline")
                            .size(16.0)
                            .strong()
                            .color(TEXT_PRIMARY),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Toca Descargar en cualquier canción o álbum para escuchar sin conexión.",
                        )
                        .size(12.5)
                        .color(TEXT_SECONDARY),
                    );
                    ui.add_space(14.0);
                    if icons::icon_pill_button(
                        ui,
                        icons::Icon::Home,
                        "Explorar música",
                        ACCENT_RED,
                        Color32::WHITE,
                    )
                    .clicked()
                    {
                        self.active_tab = NavTab::Home;
                    }
                });
                return;
            }

            #[derive(Copy, Clone)]
            enum DlAction {
                Cancel,
                Pause,
                Resume,
                Delete,
            }

            let items = self.downloads.clone();
            for task in items {
                let mut pending_op: Option<DlAction> = None;
                let mut play_completed = false;
                egui::Frame::new()
                    .fill(BG_SURFACE)
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let row_start = ui.cursor().min;
                            render_track_thumb(ui, &task.track, 48.0);
                            let text_w = (ui.available_width() - 88.0).max(110.0);
                            ui.allocate_ui(Vec2::new(text_w, 54.0), |ui| {
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.set_max_width(text_w);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&task.track.title).strong(),
                                        )
                                        .truncate()
                                        .selectable(false),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&task.track.artist)
                                                .small()
                                                .color(TEXT_SECONDARY),
                                        )
                                        .truncate()
                                        .selectable(false),
                                    );
                                    match &task.status {
                                        DownloadStatus::Queued => {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new("En cola...")
                                                        .small()
                                                        .color(TEXT_SECONDARY),
                                                )
                                                .truncate()
                                                .selectable(false),
                                            );
                                        }
                                        DownloadStatus::Downloading => {
                                            let frac = task.progress().clamp(0.0, 1.0);
                                            let (bar_rect, _) = ui.allocate_exact_size(
                                                Vec2::new(text_w, 4.0),
                                                Sense::hover(),
                                            );
                                            ui.painter().rect_filled(
                                                bar_rect,
                                                CornerRadius::same(2),
                                                Color32::from_white_alpha(28),
                                            );
                                            if frac > 0.0 {
                                                let fill_rect = egui::Rect::from_min_size(
                                                    bar_rect.min,
                                                    Vec2::new(
                                                        bar_rect.width() * frac,
                                                        bar_rect.height(),
                                                    ),
                                                );
                                                ui.painter().rect_filled(
                                                    fill_rect,
                                                    CornerRadius::same(2),
                                                    ACCENT_RED,
                                                );
                                            }
                                            let pct = (frac * 100.0) as u32;
                                            let mb_done =
                                                task.downloaded_bytes as f64 / (1024.0 * 1024.0);
                                            let speed_kb = task.speed_bps as f64 / 1024.0;
                                            let eta = task
                                                .eta_secs()
                                                .map(|s| format!(" • {s}s"))
                                                .unwrap_or_default();
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(format!(
                                                        "{pct}% • {mb_done:.1} MB • {speed_kb:.0} KB/s{eta}"
                                                    ))
                                                    .small()
                                                    .color(TEXT_SECONDARY),
                                                )
                                                .truncate()
                                                .selectable(false),
                                            );
                                        }
                                        DownloadStatus::Paused => {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new("Pausada")
                                                        .small()
                                                        .color(TEXT_SECONDARY),
                                                )
                                                .truncate()
                                                .selectable(false),
                                            );
                                        }
                                        DownloadStatus::Completed => {
                                            let mb =
                                                task.downloaded_bytes as f64 / (1024.0 * 1024.0);
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(format!("Offline • {mb:.1} MB"))
                                                        .small()
                                                        .color(GREEN_OK),
                                                )
                                                .truncate()
                                                .selectable(false),
                                            );
                                        }
                                        DownloadStatus::Failed(err) => {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(format!("Error: {err}"))
                                                        .small()
                                                        .color(ACCENT_RED),
                                                )
                                                .truncate()
                                                .selectable(false),
                                            );
                                        }
                                        DownloadStatus::Cancelled => {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new("Cancelada")
                                                        .small()
                                                        .color(TEXT_SECONDARY),
                                                )
                                                .truncate()
                                                .selectable(false),
                                            );
                                        }
                                    }
                                });
                            });

                            if task.status == DownloadStatus::Completed {
                                let hit_rect = egui::Rect::from_min_size(
                                    row_start,
                                    Vec2::new(48.0 + 8.0 + text_w, 54.0),
                                );
                                if ui
                                    .interact(
                                        hit_rect,
                                        ui.id().with(("dl_row", &task.track.id)),
                                        Sense::click(),
                                    )
                                    .clicked()
                                {
                                    play_completed = true;
                                }
                            }

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
                                            pending_op = Some(DlAction::Cancel);
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
                                            pending_op = Some(DlAction::Pause);
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
                                            pending_op = Some(DlAction::Cancel);
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
                                            pending_op = Some(DlAction::Resume);
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
                                            pending_op = Some(DlAction::Delete);
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
                                            play_completed = true;
                                        }
                                    }
                                    DownloadStatus::Cancelled => {}
                                }
                            });
                        });
                    });

                if play_completed {
                    if let Some(pos) = completed.iter().position(|t| t.id == task.track.id) {
                        self.play_track_in_list(completed.clone(), pos, ctx);
                    }
                }
                if let Some(op) = pending_op {
                    let dl = Arc::clone(&self.downloader);
                    let id = task.track.id.clone();
                    let tx = self.event_tx.clone();
                    let ctx = ctx.clone();
                    self.runtime.spawn(async move {
                        match op {
                            DlAction::Cancel => dl.cancel(&id).await,
                            DlAction::Pause => dl.pause(&id).await,
                            DlAction::Resume => dl.resume(&id).await,
                            DlAction::Delete => dl.delete_downloaded(&id).await,
                        }
                        let tasks = dl.list_tasks().await;
                        let total_bytes = dl.total_downloaded_bytes().await;
                        let _ = tx.send(AppEvent::DownloadsRefreshed { tasks, total_bytes });
                        ctx.request_repaint();
                    });
                }
            }
        });
        if self.handle_pull_to_refresh(ui, dl_scroll.state.offset.y, dl_viewport, false) {
            self.refresh_downloads(ctx);
            self.set_banner("Actualizando descargas...");
        }
        self.render_pull_refresh_overlay(ctx, dl_viewport, false);
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
            let _top_bar_min_y = ui.cursor().top();
            let active_until = *self.sleep_timer_until.lock().unwrap();
            let after_track = self
                .sleep_after_track
                .load(std::sync::atomic::Ordering::SeqCst);
            let timer_text = if let Some(until) = active_until {
                let rem = until.saturating_duration_since(Instant::now()).as_secs();
                Some(format!("{}:{:02}", rem / 60, rem % 60))
            } else if after_track {
                Some("Fin".to_string())
            } else {
                None
            };

            // Top bar: v chevron | [ Canción | Video ] switch | Cast & More ⋮
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
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

                let total_w = inner_rect.width();
                let pill_w = 166.0_f32;
                let left_w = 42.0_f32;
                let pad_left = ((total_w - pill_w) * 0.5 - left_w).max(4.0);
                ui.add_space(pad_left);

                // Center: YouTube Music Audio/Video mode segmented pill
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(18, 18, 22, 195))
                    .stroke(Stroke::new(0.8_f32, Color32::from_white_alpha(34)))
                    .corner_radius(CornerRadius::same(16))
                    .inner_margin(egui::Margin::symmetric(2, 2))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            let is_video = self.is_video_mode;
                            let (ar, a_resp) = ui.allocate_exact_size(Vec2::new(84.0, 28.0), Sense::click());
                            if ui.is_rect_visible(ar) {
                                if !is_video {
                                    ui.painter().rect_filled(ar, CornerRadius::same(14), Color32::from_rgb(62, 62, 74));
                                }
                                let ic_r = Rect::from_center_size(Pos2::new(ar.left() + 14.0, ar.center().y), Vec2::splat(15.0));
                                icons::draw_icon(
                                    ui.painter(),
                                    ic_r,
                                    icons::Icon::Headphones,
                                    if !is_video { Color32::WHITE } else { Color32::from_white_alpha(150) },
                                );
                                ui.painter().text(
                                    Pos2::new(ar.left() + 26.0, ar.center().y),
                                    Align2::LEFT_CENTER,
                                    "Canción",
                                    egui::FontId::proportional(12.0),
                                    if !is_video { Color32::WHITE } else { Color32::from_white_alpha(150) },
                                );
                            }
                            if a_resp.clicked() {
                                self.is_video_mode = false;
                                self.set_banner("Modo Canción activado");
                            }
                            let (vr, v_resp) = ui.allocate_exact_size(Vec2::new(76.0, 28.0), Sense::click());
                            if ui.is_rect_visible(vr) {
                                if is_video {
                                    ui.painter().rect_filled(vr, CornerRadius::same(14), ACCENT_RED);
                                }
                                let ic_r = Rect::from_center_size(Pos2::new(vr.left() + 16.0, vr.center().y), Vec2::splat(13.0));
                                icons::draw_icon(
                                    ui.painter(),
                                    ic_r,
                                    icons::Icon::Play,
                                    if is_video { Color32::WHITE } else { Color32::from_white_alpha(150) },
                                );
                                ui.painter().text(
                                    Pos2::new(vr.left() + 28.0, vr.center().y),
                                    Align2::LEFT_CENTER,
                                    "Video",
                                    egui::FontId::proportional(12.0),
                                    if is_video { Color32::WHITE } else { Color32::from_white_alpha(150) },
                                );
                            }
                            if v_resp.clicked() {
                                self.is_video_mode = true;
                                self.audio.pause();
                                crate::android_media::open_video(&track.id, &track.title, &track.artist);
                                self.set_banner("Reproduciendo video musical...");
                            }
                        });
                    });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
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
                    if icons::icon_button(
                        ui,
                        icons::Icon::Cast,
                        34.0,
                        17.0,
                        Color32::TRANSPARENT,
                        Color32::from_white_alpha(200),
                        17.0,
                    )
                    .clicked()
                    {
                        self.set_banner("Transmitiendo a dispositivo...");
                    }
                });
            });

            let avail_w = inner_rect.width();
            let bottom_tabs_h = 52.0;
            let body_h = (ui.available_height() - bottom_tabs_h).max(200.0);

            ui.allocate_ui(Vec2::new(avail_w, body_h), |ui| {
                ui.set_min_height(body_h);
                ui.set_max_width(avail_w);
                if self.show_queue || self.show_lyrics || self.show_related {
                    // Compact header row in drawers so current track stays visible + controllable.
                    ui.horizontal(|ui| {
                        render_track_thumb(ui, &track, 42.0);
                        let tw = (ui.available_width() - 136.0).max(80.0);
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
                            if icons::icon_button(
                                ui,
                                icons::Icon::SkipNext,
                                34.0,
                                17.0,
                                Color32::from_white_alpha(20),
                                Color32::WHITE,
                                17.0,
                            )
                            .clicked()
                                && self.queue.lock().unwrap().next_track(true).is_some()
                            {
                                self.play_current_queue_track(ctx);
                            }
                            ui.add_space(6.0);
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
                    ui.add_space(6.0);
                    let total_dur = snap
                        .duration_secs
                        .or(track.duration_secs.map(|d| d as f64))
                        .unwrap_or(1.0)
                        .max(1.0);
                    let frac = (snap.position_secs / total_dur).clamp(0.0, 1.0) as f32;
                    let (pb_rect, _) =
                        ui.allocate_exact_size(Vec2::new(avail_w, 2.5), Sense::hover());
                    ui.painter().rect_filled(
                        pb_rect,
                        CornerRadius::same(1),
                        Color32::from_white_alpha(28),
                    );
                    if frac > 0.0 {
                        let fill_w = (pb_rect.width() * frac).max(2.0);
                        let fill_rect = egui::Rect::from_min_size(
                            pb_rect.min,
                            Vec2::new(fill_w, pb_rect.height()),
                        );
                        ui.painter()
                            .rect_filled(fill_rect, CornerRadius::same(1), ACCENT_RED);
                    }
                    ui.add_space(8.0);

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
                            let hdr_w = (ui.available_width() - 196.0).max(100.0);
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
                        // Dynamic Queue Tuner Mood Pills (YouTube Music Parity)
                        ui.add_space(4.0);
                        egui::ScrollArea::horizontal()
                            .id_salt("queue_tuner_chips")
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    for tuner in [
                                        QueueTuner::All,
                                        QueueTuner::Familiar,
                                        QueueTuner::Discovery,
                                        QueueTuner::Popular,
                                        QueueTuner::Deep,
                                    ] {
                                        let is_sel = self.queue_tuner == tuner;
                                        let btn = egui::Button::new(
                                            RichText::new(tuner.label()).size(12.0).color(if is_sel {
                                                Color32::WHITE
                                            } else {
                                                TEXT_PRIMARY
                                            }),
                                        )
                                        .fill(if is_sel {
                                            ACCENT_RED
                                        } else {
                                            Color32::from_white_alpha(24)
                                        })
                                        .corner_radius(CornerRadius::same(14));
                                        if ui.add(btn).clicked() {
                                            self.queue_tuner = tuner;
                                            self.apply_queue_tuner(tuner);
                                        }
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
                            let hdr_w = (ui.available_width() - 96.0).max(120.0);
                            ui.allocate_ui(Vec2::new(hdr_w, 30.0), |ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new("Similares y relacionados")
                                                .size(16.0)
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
                    if self.is_video_mode {
                        // Widescreen Video Surface (16:9 YouTube Music style)
                    ui.add_space(6.0);
                    let video_w = (avail_w - 24.0).min(380.0);
                    let video_h = (video_w * 9.0 / 16.0).clamp(140.0, 210.0);
                    let video_size = Vec2::new(video_w, video_h);
                    ui.vertical_centered(|ui| {
                        let (video_rect, video_resp) = ui.allocate_exact_size(video_size, Sense::click());
                        ui.painter().rect_filled(video_rect, CornerRadius::same(12), Color32::from_rgb(10, 10, 12));
                        ui.painter().rect_stroke(video_rect, CornerRadius::same(12), Stroke::new(1.0_f32, Color32::from_white_alpha(32)), egui::StrokeKind::Outside);

                        // High resolution video frame thumbnail
                        if let Some(u) = resolved_cover.as_deref() {
                            paint_progressive_image(ui, video_rect, u, 720, video_size, CornerRadius::same(12));
                        }

                        // Play circle overlay
                        let overlay_c = video_rect.center();
                        ui.painter().circle_filled(overlay_c, 24.0, Color32::from_rgba_unmultiplied(0, 0, 0, 170));
                        ui.painter().circle_stroke(overlay_c, 24.0, Stroke::new(1.5_f32, Color32::WHITE));
                        icons::draw_icon(
                            ui.painter(),
                            Rect::from_center_size(overlay_c + Vec2::new(2.0, 0.0), Vec2::splat(18.0)),
                            icons::Icon::Play,
                            Color32::WHITE,
                        );

                        // Video Badge top right
                        let badge_rect = Rect::from_min_size(
                            Pos2::new(video_rect.right() - 86.0, video_rect.top() + 8.0),
                            Vec2::new(76.0, 20.0),
                        );
                        ui.painter().rect_filled(badge_rect, CornerRadius::same(4), Color32::from_rgba_unmultiplied(0, 0, 0, 190));
                        ui.painter().text(
                            badge_rect.center(),
                            Align2::CENTER_CENTER,
                            "1080p HD",
                            egui::FontId::proportional(11.0),
                            Color32::WHITE,
                        );

                        if video_resp.clicked() {
                            self.audio.pause();
                            crate::android_media::open_video(&track.id, &track.title, &track.artist);
                            self.set_banner("Reproduciendo video musical...");
                        }

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let pad = (video_w - 230.0).max(0.0) * 0.5;
                            if pad > 0.0 {
                                ui.add_space(pad);
                            }
                            if icons::icon_pill_button(
                                ui,
                                icons::Icon::Video,
                                "Reproducir Video",
                                ACCENT_RED,
                                Color32::WHITE,
                            )
                            .clicked()
                            {
                                self.audio.pause();
                                crate::android_media::open_video(&track.id, &track.title, &track.artist);
                                self.set_banner("Reproduciendo video musical...");
                            }
                            ui.add_space(6.0);
                            if icons::icon_pill_button(
                                ui,
                                icons::Icon::Headphones,
                                "Solo Audio",
                                Color32::from_white_alpha(26),
                                Color32::WHITE,
                            )
                            .clicked()
                            {
                                self.is_video_mode = false;
                            }
                        });
                    });
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
                            paint_progressive_image(
                                ui,
                                art_rect,
                                u,
                                544,
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
                    }

                    ui.add_space(if self.is_video_mode { 10.0 } else { 16.0 });
                    // Left-aligned Title > and Artist constrained to avail_w
                    let title_w = (avail_w - 24.0).max(140.0);
                    let mut title_clicked = false;
                    ui.allocate_ui(Vec2::new(avail_w, 0.0), |ui| {
                        ui.set_max_width(avail_w);
                        ui.horizontal(|ui| {
                            ui.allocate_ui(Vec2::new(title_w, 0.0), |ui| {
                                ui.vertical(|ui| {
                                    ui.set_max_width(title_w);
                                    let job = clamped_layout_job(
                                        &track.title,
                                        21.0,
                                        Color32::WHITE,
                                        title_w,
                                        2,
                                    );
                                    if ui
                                        .add(
                                            egui::Label::new(job)
                                                .wrap()
                                                .selectable(false)
                                                .sense(Sense::click()),
                                        )
                                        .clicked()
                                    {
                                        title_clicked = true;
                                    }
                                });
                            });
                            if ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(" ›")
                                            .size(21.0)
                                            .strong()
                                            .color(Color32::from_white_alpha(210)),
                                    )
                                    .selectable(false)
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
                                .truncate()
                                .selectable(false)
                                .sense(Sense::click()),
                            )
                            .clicked()
                        {
                            self.now_playing_open = false;
                            self.open_artist(artist_id, ctx);
                        }
                    } else {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&track.artist)
                                    .size(15.0)
                                    .color(Color32::from_white_alpha(190)),
                            )
                            .truncate()
                            .selectable(false),
                        );
                    }

                    let is_offline = self
                        .downloads
                        .iter()
                        .any(|d| d.track.id == track.id && d.status == DownloadStatus::Completed);
                    let status_badge: Option<(String, Color32)> = match &snap.status {
                        PlaybackStatus::Buffering | PlaybackStatus::Loading => {
                            Some(("Cargando audio...".to_string(), Color32::from_white_alpha(210)))
                        }
                        PlaybackStatus::Error(err) => Some((err.clone(), ACCENT_RED)),
                        _ => match (is_offline, snap.codec_label.as_deref()) {
                            (true, Some(c)) => Some((format!("Offline • {c}"), GREEN_OK)),
                            (true, None) => Some(("Offline".to_string(), GREEN_OK)),
                            (false, Some(c)) => {
                                Some((c.to_string(), Color32::from_white_alpha(155)))
                            }
                            (false, None) => None,
                        },
                    };
                    if status_badge.is_some() || timer_text.is_some() {
                        ui.add_space(3.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            if let Some((txt, col)) = status_badge {
                                ui.label(RichText::new(txt).size(11.5).color(col));
                            }
                            if let Some(ref t_txt) = timer_text {
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Moon,
                                    &format!("Apagado: {t_txt}"),
                                    Color32::from_white_alpha(32),
                                    Color32::WHITE,
                                )
                                .clicked()
                                {
                                    self.show_sleep_timer_sheet = true;
                                }
                            }
                        });
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

                                ui.add_space(6.0);
                                let sleep_label = timer_text
                                    .as_deref()
                                    .map(|t| format!("Apagado {t}"))
                                    .unwrap_or_else(|| "Temporizador".to_string());
                                if icons::icon_pill_button(
                                    ui,
                                    icons::Icon::Moon,
                                    &sleep_label,
                                    if timer_text.is_some() { ACCENT_RED } else { pill_bg },
                                    Color32::WHITE,
                                )
                                .clicked()
                                {
                                    self.show_sleep_timer_sheet = true;
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

            ui.add_space(4.0);
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
        #[cfg(target_os = "android")]
        ui.add_space(28.0);
        let is_root = self.settings_category == SettingsCategory::Root;
        let title = self.settings_category.title();

        ui.horizontal(|ui| {
            if icons::icon_button(
                ui,
                icons::Icon::ArrowBack,
                36.0,
                18.0,
                Color32::TRANSPARENT,
                Color32::WHITE,
                18.0,
            )
            .clicked()
            {
                if is_root {
                    self.show_settings = false;
                } else {
                    self.settings_category = SettingsCategory::Root;
                }
            }
            ui.add_space(6.0);
            let title_max_w = if is_root {
                ui.available_width()
            } else {
                (ui.available_width() - 42.0).max(100.0)
            };
            ui.allocate_ui(Vec2::new(title_max_w, 28.0), |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(title).size(18.0).strong().color(Color32::WHITE),
                    )
                    .truncate(),
                );
            });
            if !is_root {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icons::icon_button(
                        ui,
                        icons::Icon::Close,
                        34.0,
                        17.0,
                        Color32::TRANSPARENT,
                        Color32::WHITE,
                        17.0,
                    )
                    .clicked()
                    {
                        self.show_settings = false;
                        self.settings_category = SettingsCategory::Root;
                    }
                });
            }
        });

        ui.add_space(10.0);

        if is_root {
            // Categorized settings menu list (matches YouTube Music settings screen: media_1794881819990_14eee889.jpg)
            let cat_row = |ui: &mut egui::Ui, cat_title: &str| -> egui::Response {
                let w = ui.available_width();
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 50.0), Sense::click());
                if ui.is_rect_visible(rect) {
                    let bg = if resp.hovered() {
                        Color32::from_white_alpha(16)
                    } else {
                        Color32::TRANSPARENT
                    };
                    if bg != Color32::TRANSPARENT {
                        ui.painter().rect_filled(rect, CornerRadius::same(6), bg);
                    }
                    ui.painter().text(
                        Pos2::new(rect.left() + 14.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        cat_title,
                        egui::FontId::proportional(15.5),
                        Color32::WHITE,
                    );
                }
                resp
            };

            if cat_row(ui, "Morphe").clicked() {
                self.settings_category = SettingsCategory::Morphe;
            }
            if cat_row(ui, "Centro familiar").clicked() {
                self.settings_category = SettingsCategory::FamilyCenter;
            }
            if cat_row(ui, "General").clicked() {
                self.settings_category = SettingsCategory::General;
            }
            if cat_row(ui, "Reproducción").clicked() {
                self.settings_category = SettingsCategory::Playback;
            }
            if cat_row(ui, "Ahorro de datos").clicked() {
                self.settings_category = SettingsCategory::DataSaver;
            }
            if cat_row(ui, "Descargas y almacenamiento").clicked() {
                self.settings_category = SettingsCategory::DownloadsStorage;
            }
            if cat_row(ui, "Notificaciones").clicked() {
                self.settings_category = SettingsCategory::Notifications;
            }
            if cat_row(ui, "Privacidad y datos").clicked() {
                self.settings_category = SettingsCategory::Privacy;
            }
            if cat_row(ui, "Recomendaciones").clicked() {
                self.settings_category = SettingsCategory::Recommendations;
            }
            if cat_row(ui, "Acerca de YR Music").clicked() {
                self.settings_category = SettingsCategory::About;
            }
        } else {
            // Category detail page
            match self.settings_category {
                SettingsCategory::Root => {}
                SettingsCategory::Morphe => {
                    ui.heading("Morphe & Laboratorio");
                    ui.add_space(4.0);
                    ui.label(RichText::new("Experimentos de audio en Rust y funciones avanzadas de próxima generación.").color(TEXT_SECONDARY));
                    ui.add_space(10.0);
                    ui.label("Efectos espaciales:");
                    ui.label(RichText::new("Procesamiento binaural de baja latencia con motor nativo Symphonia.").size(12.0).color(TEXT_SECONDARY));
                    ui.add_space(10.0);
                    ui.label("Generación de carátulas:");
                    ui.label(RichText::new("Paleta cromática adaptativa en tiempo real extraída de la portada del álbum.").size(12.0).color(TEXT_SECONDARY));
                }
                SettingsCategory::FamilyCenter => {
                    ui.label(RichText::new("Administra la experiencia musical de los miembros de tu familia.").color(TEXT_SECONDARY));
                    ui.add_space(10.0);
                    ui.label("Filtro de contenido explícito:");
                    ui.label(RichText::new("Oculta canciones con letras o temáticas explícitas en búsquedas y mixes automáticos.").size(12.0).color(TEXT_SECONDARY));
                    ui.add_space(10.0);
                    ui.label("Cuentas compartidas:");
                    ui.label(RichText::new("Permite alternar perfiles familiares de Google rápidamente sin perder descargas locales.").size(12.0).color(TEXT_SECONDARY));
                }
                SettingsCategory::General => {
                    ui.label("Idioma de la aplicación:");
                    ui.label(RichText::new("Español (Latinoamérica)").color(TEXT_SECONDARY));
                    ui.add_space(10.0);
                    ui.label("Tema visual:");
                    ui.label(RichText::new("Negro puro OLED (Ahorro de energía en pantallas AMOLED)").color(TEXT_SECONDARY));
                    ui.add_space(10.0);
                    ui.label("Volumen interno de reproducción:");
                    let mut vol_pct = (self.store.data.config.volume.clamp(0.0, 1.0) * 100.0).round() as u32;
                    let slider_resp = ui.add(egui::Slider::new(&mut vol_pct, 0..=100).suffix("%"));
                    if slider_resp.changed() {
                        let v = (vol_pct as f32 / 100.0).clamp(0.0, 1.0);
                        self.store.data.config.volume = v;
                        self.audio.set_volume(v);
                    }
                    if slider_resp.drag_stopped() || slider_resp.lost_focus() {
                        let _ = self.store.save();
                    }
                }
                SettingsCategory::Playback => {
                    ui.label("Calidad de streaming de audio:");
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                        for q in [AudioQuality::Low, AudioQuality::Medium, AudioQuality::High, AudioQuality::Best] {
                            let selected = self.store.data.config.stream_quality == q;
                            if settings_choice_pill(ui, selected, q.label()) {
                                self.store.data.config.stream_quality = q;
                                let _ = self.store.save();
                            }
                        }
                    });
                    ui.add_space(12.0);
                    let eq_active = self.store.data.config.equalizer.enabled;
                    let eq_preset = self.store.data.config.equalizer.preset_name.clone();
                    egui::Frame::new()
                        .fill(BG_SURFACE)
                        .stroke(Stroke::new(0.8_f32, Color32::from_rgb(52, 52, 60)))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                                icons::draw_icon(
                                    ui.painter(),
                                    r,
                                    icons::Icon::Equalizer,
                                    if eq_active { ACCENT_RED } else { Color32::WHITE },
                                );
                                ui.vertical(|ui| {
                                    ui.label(RichText::new("Ecualizador & Bass Boost").strong());
                                    ui.label(
                                        RichText::new(if eq_active {
                                            format!("Activo • Preset: {}", eq_preset)
                                        } else {
                                            "Desactivado (bypass)".to_string()
                                        })
                                        .size(12.0)
                                        .color(TEXT_SECONDARY),
                                    );
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if icons::icon_pill_button(
                                        ui,
                                        icons::Icon::Equalizer,
                                        "Ajustar",
                                        if eq_active { ACCENT_RED } else { BG_ELEVATED },
                                        Color32::WHITE,
                                    )
                                    .clicked()
                                    {
                                        self.show_equalizer_modal = true;
                                    }
                                });
                            });
                        });
                    ui.add_space(12.0);
                    let mut norm = self.store.data.config.equalizer.loudness_normalization;
                    if ui.checkbox(&mut norm, "Normalización de volumen (Loudness Normalization)").changed() {
                        self.store.data.config.equalizer.loudness_normalization = norm;
                        self.audio.set_equalizer(self.store.data.config.equalizer.clone());
                        let _ = self.store.save();
                    }
                    ui.label(RichText::new("Ajusta suavemente el nivel de volumen para mantener un nivel uniforme en todas las pistas.").size(12.0).color(TEXT_SECONDARY));
                    ui.add_space(8.0);
                    if ui.checkbox(&mut self.store.data.config.autoplay_radio, "Radio automática continua al terminar la cola").changed() {
                        let _ = self.store.save();
                    }
                    if ui.checkbox(&mut self.store.data.config.sponsorblock_enabled, "SponsorBlock: omitir intros/patrocinios en audio").changed() {
                        let _ = self.store.save();
                    }
                }
                SettingsCategory::DataSaver => {
                    if ui.checkbox(&mut self.store.data.config.mobile_data_saver, "Ahorro de datos móviles").changed() {
                        let _ = self.store.save();
                    }
                    ui.label(RichText::new("Reduce el streaming a 64 kbps al reproducir con datos móviles para ahorrar consumo y batería.").size(12.0).color(TEXT_SECONDARY));
                    ui.add_space(10.0);
                    if ui.checkbox(&mut self.store.data.config.wifi_only_streaming, "Transmitir solo mediante Wi-Fi").changed() {
                        let _ = self.store.save();
                    }
                    ui.label(RichText::new("Bloquea la descarga de streams en conexiones de datos móviles.").size(12.0).color(TEXT_SECONDARY));
                    ui.add_space(10.0);
                    if ui.checkbox(&mut self.store.data.config.battery_saver, "Modo ahorro de batería").changed() {
                        let _ = self.store.save();
                    }
                    ui.label(RichText::new("Desactiva animaciones de carátula y suspende la precarga de la siguiente canción cuando la batería esté baja.").size(12.0).color(TEXT_SECONDARY));
                    ui.add_space(10.0);
                    if ui.checkbox(&mut self.store.data.config.prefer_video_mode, "Desactivar videos en datos móviles").changed() {
                        let _ = self.store.save();
                    }
                    ui.label(RichText::new("Fuerza reproducción de solo audio en el reproductor para máxima autonomía.").size(12.0).color(TEXT_SECONDARY));
                }
                SettingsCategory::DownloadsStorage => {
                    if ui.checkbox(&mut self.store.data.config.smart_downloads, "Descargas inteligentes (Auto-descarga)").changed() {
                        let _ = self.store.save();
                    }
                    ui.label(RichText::new("Descarga automáticamente en segundo plano tus canciones favoritas y mixes recientes para escuchar offline.").size(12.0).color(TEXT_SECONDARY));
                    ui.add_space(10.0);
                    if ui.checkbox(&mut self.store.data.config.wifi_only_downloads, "Descargar solo mediante Wi-Fi").changed() {
                        let _ = self.store.save();
                    }
                    ui.add_space(10.0);
                    ui.label("Calidad de descargas offline:");
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                        for q in [AudioQuality::Low, AudioQuality::Medium, AudioQuality::High, AudioQuality::Best] {
                            let selected = self.store.data.config.download_quality == q;
                            if settings_choice_pill(ui, selected, q.label()) {
                                self.store.data.config.download_quality = q;
                                let _ = self.store.save();
                            }
                        }
                    });
                    ui.add_space(14.0);
                    let mb = self.total_downloaded_bytes as f64 / (1024.0 * 1024.0);
                    let cache_mb = self.stream_cache_bytes as f64 / (1024.0 * 1024.0);
                    ui.label(RichText::new(format!("Almacenamiento offline usado: {mb:.1} MB")).strong());
                    ui.label(RichText::new(format!("Caché de streaming en disco: {cache_mb:.1} MB")).color(TEXT_SECONDARY));
                    ui.add_space(10.0);
                    ui.label("Límite de caché de streaming:");
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                        for limit_mb in [128_u32, 256, 512, 1024, 2048] {
                            let label = match limit_mb {
                                1024 => "1 GB".to_string(),
                                2048 => "2 GB".to_string(),
                                _ => format!("{limit_mb} MB"),
                            };
                            let selected = self.store.data.config.max_stream_cache_mb == limit_mb;
                            if settings_choice_pill(ui, selected, &label) {
                                self.store.data.config.max_stream_cache_mb = limit_mb;
                                let _ = self.store.save();
                                self.stream_cache_bytes = prune_dir_to_max_bytes(
                                    &self.data_dir.join("stream_cache"),
                                    (limit_mb as u64) * 1024 * 1024,
                                );
                            }
                        }
                    });
                    ui.add_space(12.0);
                    if settings_action_button(ui, "Limpiar caché de streaming y portadas") {
                        let cache_dir = self.data_dir.join("stream_cache");
                        let _ = std::fs::remove_dir_all(&cache_dir);
                        let _ = std::fs::create_dir_all(&cache_dir);
                        let _ = std::fs::remove_file(self.data_dir.join("home_cache.json"));
                        let _ = std::fs::remove_file(self.data_dir.join("explore_cache.json"));
                        self.stream_cache_bytes = 0;
                        ctx.forget_all_images();
                        self.set_banner("Caché y portadas liberadas");
                    }
                }
                SettingsCategory::Notifications => {
                    if ui.checkbox(&mut self.store.data.config.notifications_enabled, "Notificación multimedia en segundo plano").changed() {
                        let _ = self.store.save();
                    }
                    ui.label(RichText::new("Muestra la carátula, título y controles de pausa/saltar en la barra de notificaciones del sistema.").size(12.0).color(TEXT_SECONDARY));
                }
                SettingsCategory::Privacy => {
                    if ui.checkbox(&mut self.store.data.config.incognito_mode, "Modo incógnito (Pausar historial)").changed() {
                        let _ = self.store.save();
                    }
                    ui.label(RichText::new("Las canciones que escuches no se guardarán en tu historial ni alterarán tus recomendaciones.").size(12.0).color(TEXT_SECONDARY));
                    ui.add_space(12.0);
                    if settings_action_button(ui, "Borrar historial de reproducciones") {
                        self.store.data.recent_tracks.clear();
                        let _ = self.store.save();
                        self.set_banner("Historial de reproducciones borrado");
                    }
                    ui.add_space(8.0);
                    if settings_action_button(ui, "Borrar historial de búsquedas") {
                        self.store.data.search_history.clear();
                        self.store.data.search_history_items.clear();
                        let _ = self.store.save();
                        self.set_banner("Historial de búsquedas borrado");
                    }
                }
                SettingsCategory::Recommendations => {
                    if ui.checkbox(&mut self.store.data.config.personalized_recommendations, "Recomendaciones personalizadas con cuenta Google").changed() {
                        let _ = self.store.save();
                    }
                    ui.label(RichText::new("Ajusta los mixes de inicio según tu historial de reproducción, canciones favoritas y artistas seguidos.").size(12.0).color(TEXT_SECONDARY));
                }
                SettingsCategory::About => {
                    ui.heading("YR Music v0.7.4");
                    ui.label("Cliente nativo de música 100% en Rust para Android y escritorio.");
                    ui.add_space(6.0);
                    ui.label(RichText::new("Motor de audio Symphonia PCM de ultra-baja latencia con ecualizador paramétrico de 5 bandas en tiempo real, prefetch predictivo en disco y compatibilidad completa con YouTube Music.").size(12.0).color(TEXT_SECONDARY));
                    ui.add_space(8.0);
                    ui.label(RichText::new("Licencia MIT • Código abierto").size(12.0).color(TEXT_SECONDARY));
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
            .anchor(egui::Align2::LEFT_BOTTOM, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_width(screen.width());
                ui.set_max_width(screen.width());
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
                        let inner_w = (screen.width() - 28.0).max(180.0);
                        ui.set_width(inner_w);
                        ui.set_max_width(inner_w);
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
                if sheet_row(ui, icons::Icon::Video, "Ver video musical") {
                    self.audio.pause();
                    crate::android_media::open_video(&track.id, &track.title, &track.artist);
                    self.set_banner("Reproduciendo video musical...");
                    self.action_sheet_track = None;
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

                let eq_active = self.store.data.config.equalizer.enabled;
                let eq_label = if eq_active {
                    format!("Ecualizador ({})", self.store.data.config.equalizer.preset_name)
                } else {
                    "Ecualizador (Desactivado)".to_string()
                };
                if sheet_row(ui, icons::Icon::Equalizer, &eq_label) {
                    self.show_equalizer_modal = true;
                    self.action_sheet_track = None;
                }

                let queue_not_empty = !self.queue.lock().unwrap().is_empty();
                if queue_not_empty && sheet_row(ui, icons::Icon::Close, "Descartar fila") {
                    self.queue.lock().unwrap().clear();
                    self.audio.stop();
                    self.set_banner("Fila descartada");
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
            .anchor(egui::Align2::LEFT_BOTTOM, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_width(screen.width());
                ui.set_max_width(screen.width());
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
                        let inner_w = (screen.width() - 32.0).max(180.0);
                        ui.set_width(inner_w);
                        ui.set_max_width(inner_w);
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
                                         fg: Color32,
                                         border: Stroke|
                         -> bool {
                            let (r, resp) =
                                ui.allocate_exact_size(Vec2::new(row_w, 42.0), Sense::click());
                            if ui.is_rect_visible(r) {
                                ui.painter().rect(
                                    r,
                                    CornerRadius::same(10),
                                    bg,
                                    border,
                                    egui::StrokeKind::Inside,
                                );
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

                        let default_bg = BG_ELEVATED;
                        let default_border = Stroke::new(0.8_f32, Color32::from_rgb(56, 56, 66));
                        for (mins, label) in [
                            (15_u64, "15 minutos"),
                            (30_u64, "30 minutos"),
                            (45_u64, "45 minutos"),
                            (60_u64, "60 minutos"),
                        ] {
                            if timer_row(ui, label, default_bg, Color32::WHITE, default_border) {
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
                                Color32::from_rgb(56, 22, 26)
                            } else {
                                default_bg
                            },
                            if after_track {
                                ACCENT_RED
                            } else {
                                Color32::WHITE
                            },
                            if after_track {
                                Stroke::new(1.0_f32, ACCENT_RED)
                            } else {
                                default_border
                            },
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
                                Stroke::new(1.0_f32, ACCENT_RED),
                            )
                        {
                            *self.sleep_timer_until.lock().unwrap() = None;
                            self.sleep_after_track
                                .store(false, std::sync::atomic::Ordering::SeqCst);
                            self.set_banner("Temporizador cancelado");
                            self.show_sleep_timer_sheet = false;
                        }

                        #[cfg(target_os = "android")]
                        ui.add_space(58.0);
                    });
            });
    }

    fn render_equalizer_modal(&mut self, ctx: &egui::Context) {
        if !self.show_equalizer_modal {
            return;
        }
        let screen = ctx.screen_rect();
        if render_modal_scrim(ctx, "yr_equalizer_scrim") {
            self.show_equalizer_modal = false;
        }

        let mut changed = false;
        let panel_w = screen.width();
        let inner_w = (panel_w - 28.0).max(220.0);

        egui::Area::new(egui::Id::new("yr_equalizer_panel"))
            .anchor(egui::Align2::LEFT_BOTTOM, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_width(panel_w);
                ui.set_max_width(panel_w);
                egui::Frame::new()
                    .fill(Color32::from_rgb(22, 22, 26))
                    .stroke(Stroke::new(0.8_f32, Color32::from_rgb(48, 48, 54)))
                    .corner_radius(CornerRadius {
                        nw: 20,
                        ne: 20,
                        sw: 0,
                        se: 0,
                    })
                    .inner_margin(egui::Margin::symmetric(14, 14))
                    .show(ui, |ui| {
                        ui.set_width(inner_w);
                        ui.set_max_width(inner_w);

                        // Header
                        ui.horizontal(|ui| {
                            let (mr, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                            icons::draw_icon(ui.painter(), mr, icons::Icon::Equalizer, ACCENT_RED);
                            ui.label(
                                RichText::new("Ecualizador & Bass Boost")
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
                                    self.show_equalizer_modal = false;
                                }
                            });
                        });

                        ui.add_space(8.0);

                        // Master enable toggle + reset button
                        ui.horizontal(|ui| {
                            let mut eq_enabled = self.store.data.config.equalizer.enabled;
                            if ui.checkbox(&mut eq_enabled, "Activar ecualizador").changed() {
                                self.store.data.config.equalizer.enabled = eq_enabled;
                                changed = true;
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("Reiniciar a Plano").clicked() {
                                    self.store.data.config.equalizer.apply_preset("Plano");
                                    changed = true;
                                }
                            });
                        });

                        ui.add_space(4.0);
                        let mut norm = self.store.data.config.equalizer.loudness_normalization;
                        if ui.checkbox(&mut norm, "Normalización de volumen").changed() {
                            self.store.data.config.equalizer.loudness_normalization = norm;
                            changed = true;
                        }

                        ui.add_space(8.0);
                        ui.label(RichText::new("Preajustes de sonido:").size(12.5).color(TEXT_SECONDARY));
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                            let current_preset = self.store.data.config.equalizer.preset_name.clone();
                            for &(name, _, _) in EqualizerConfig::presets() {
                                let selected = current_preset == name;
                                if settings_choice_pill(ui, selected, name) {
                                    self.store.data.config.equalizer.apply_preset(name);
                                    if !self.store.data.config.equalizer.enabled {
                                        self.store.data.config.equalizer.enabled = true;
                                    }
                                    changed = true;
                                }
                            }
                        });

                        ui.add_space(10.0);

                        // Bass Boost Slider
                        let mut bass = self.store.data.config.equalizer.bass_boost_db;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Bass Boost:").size(13.0).color(Color32::WHITE));
                            let slider_w = (inner_w - 110.0).clamp(90.0, 180.0);
                            let slider_resp = ui.add_sized(
                                Vec2::new(slider_w, 20.0),
                                egui::Slider::new(&mut bass, 0.0..=10.0)
                                    .custom_formatter(|v, _| format!("+{:.1} dB", v))
                                    .step_by(0.5),
                            );
                            if slider_resp.changed() {
                                self.store.data.config.equalizer.bass_boost_db = bass;
                                self.store.data.config.equalizer.preset_name = "Personalizado".to_string();
                                changed = true;
                            }
                        });

                        ui.add_space(8.0);

                        // 5 Bands faders
                        ui.label(RichText::new("Bandas de frecuencia (-12 dB a +12 dB):").size(12.5).color(TEXT_SECONDARY));
                        ui.add_space(4.0);

                        let band_labels = EqualizerConfig::BAND_LABELS;
                        ui.columns(5, |cols| {
                            for i in 0..5 {
                                cols[i].vertical_centered(|ui| {
                                    let mut gain = self.store.data.config.equalizer.bands_db[i];
                                    let val_str = if gain >= 0.0 {
                                        format!("+{:.1}", gain)
                                    } else {
                                        format!("{:.1}", gain)
                                    };
                                    ui.label(
                                        RichText::new(val_str)
                                            .size(11.0)
                                            .monospace()
                                            .color(Color32::from_white_alpha(200)),
                                    );

                                    let v_slider = egui::Slider::new(&mut gain, -12.0..=12.0)
                                        .vertical()
                                        .show_value(false)
                                        .step_by(0.5);
                                    let resp = ui.add_sized(Vec2::new(36.0, 90.0), v_slider);
                                    if resp.changed() {
                                        self.store.data.config.equalizer.bands_db[i] = gain;
                                        self.store.data.config.equalizer.preset_name = "Personalizado".to_string();
                                        changed = true;
                                    }

                                    ui.label(
                                        RichText::new(band_labels[i])
                                            .size(10.5)
                                            .strong()
                                            .color(Color32::WHITE),
                                    );
                                });
                            }
                        });

                        #[cfg(target_os = "android")]
                        ui.add_space(58.0);
                    });
            });

        if changed {
            self.audio.set_equalizer(self.store.data.config.equalizer.clone());
            let _ = self.store.save();
        }
    }

    fn render_playlist_picker_modal(&mut self, ctx: &egui::Context) {
        let Some(track) = self.playlist_picker_track.clone() else {
            return;
        };
        let screen = ctx.screen_rect();
        let mut close_sheet = render_modal_scrim(ctx, "yr_playlist_picker_scrim");

        egui::Area::new(egui::Id::new("yr_playlist_picker_panel"))
            .anchor(egui::Align2::LEFT_BOTTOM, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_width(screen.width());
                ui.set_max_width(screen.width());
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
                        ui.set_max_width(content_w);
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
                            let input_w = (content_w - 156.0).max(110.0);
                            egui::Frame::new()
                                .fill(BG_ELEVATED)
                                .corner_radius(CornerRadius::same(18))
                                .inner_margin(egui::Margin::symmetric(12, 6))
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.new_playlist_name)
                                            .hint_text("Nueva playlist...")
                                            .frame(false)
                                            .desired_width(input_w),
                                    );
                                });
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
                        ui.add_space(62.0);
                    });
            });

        if close_sheet {
            self.playlist_picker_track = None;
        }
    }

    fn render_account_modal(&mut self, ctx: &egui::Context) {
        if !self.show_account_modal {
            return;
        }

        let screen = ctx.screen_rect();
        if render_modal_scrim(ctx, "yr_account_modal_scrim") {
            self.show_account_modal = false;
        }

        let (avatar_url, user_name, user_handle, is_logged) = {
            let account_info = self
                .home_feed
                .account
                .as_ref()
                .or(self.store.data.google_account.as_ref());
            let is_logged = self.home_feed.is_logged_in || self.store.data.google_cookie.is_some();
            let av = account_info.and_then(|a| a.avatar_url.clone());
            let name = account_info
                .map(|a| a.name.clone())
                .unwrap_or_else(|| {
                    if is_logged {
                        "Usuario de Google".to_string()
                    } else {
                        "Iniciar sesión con Google".to_string()
                    }
                });
            let handle = account_info
                .and_then(|a| a.handle.clone())
                .unwrap_or_else(|| {
                    if is_logged {
                        "Cuenta sincronizada".to_string()
                    } else {
                        "Toca para sincronizar tus listas y foto".to_string()
                    }
                });
            (av, name, handle, is_logged)
        };

        egui::Area::new(egui::Id::new("yr_account_modal_panel"))
            .anchor(egui::Align2::LEFT_TOP, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.painter().rect_filled(screen, CornerRadius::ZERO, Color32::BLACK);
                ui.set_width(screen.width());
                ui.set_max_width(screen.width());
                ui.set_height(screen.height());
                ui.set_max_height(screen.height());
                egui::Frame::new()
                    .fill(Color32::from_rgb(0, 0, 0))
                    .stroke(Stroke::NONE)
                    .corner_radius(CornerRadius::ZERO)
                    .inner_margin(egui::Margin::symmetric(16, 12))
                    .show(ui, |ui| {
                        let inner_w = (screen.width() - 32.0).max(200.0);
                        ui.set_width(inner_w);
                        ui.set_max_width(inner_w);

                        #[cfg(target_os = "android")]
                        ui.add_space(32.0);

                        // Top bar: ✕ close | "Cuenta"
                        ui.horizontal(|ui| {
                            if icons::icon_button(
                                ui,
                                icons::Icon::Close,
                                32.0,
                                18.0,
                                Color32::TRANSPARENT,
                                Color32::WHITE,
                                16.0,
                            )
                            .clicked()
                            {
                                self.show_account_modal = false;
                            }
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new("Cuenta")
                                    .size(20.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                        });

                        ui.add_space(16.0);

                        // Profile Header Card (matching media_1794881820029_c543419e.jpg)
                        let (card_rect, card_resp) = ui.allocate_exact_size(Vec2::new(inner_w, 76.0), Sense::click());
                        if ui.is_rect_visible(card_rect) {
                            if card_resp.hovered() {
                                ui.painter().rect_filled(card_rect, CornerRadius::same(10), Color32::from_white_alpha(15));
                            }
                            let avatar_sz = 46.0;
                            let av_rect = Rect::from_min_size(
                                Pos2::new(card_rect.left(), card_rect.top() + 4.0),
                                Vec2::splat(avatar_sz),
                            );
                            let radius = CornerRadius::same((avatar_sz * 0.5) as u8);
                            ui.painter().rect_filled(av_rect, radius, Color32::from_rgb(38, 38, 44));

                            let mut painted = false;
                            if let Some(url) = &avatar_url {
                                let thumb_uri = tracked_thumb_uri(url, 192);
                                let img = Image::new(thumb_uri)
                                    .fit_to_exact_size(Vec2::splat(avatar_sz))
                                    .corner_radius(radius);
                                match img.load_for_size(ui.ctx(), Vec2::splat(avatar_sz)) {
                                    Ok(egui::load::TexturePoll::Ready { .. }) => {
                                        img.paint_at(ui, av_rect);
                                        painted = true;
                                    }
                                    _ => {}
                                }
                            }
                            if !painted {
                                let initial = user_name.chars().next().unwrap_or('A').to_uppercase().to_string();
                                ui.painter().text(
                                    av_rect.center(),
                                    Align2::CENTER_CENTER,
                                    initial,
                                    egui::FontId::proportional(20.0),
                                    Color32::WHITE,
                                );
                            }

                            let text_left = av_rect.right() + 12.0;
                            let chev_sz = 16.0;
                            let chev_rect = Rect::from_center_size(
                                Pos2::new(card_rect.right() - 8.0, card_rect.top() + 24.0),
                                Vec2::splat(chev_sz),
                            );
                            icons::draw_icon(ui.painter(), chev_rect, icons::Icon::ChevronRight, TEXT_SECONDARY);

                            ui.painter().text(
                                Pos2::new(text_left, card_rect.top() + 14.0),
                                Align2::LEFT_CENTER,
                                &user_name,
                                egui::FontId::proportional(16.5),
                                Color32::WHITE,
                            );

                            ui.painter().text(
                                Pos2::new(text_left, card_rect.top() + 34.0),
                                Align2::LEFT_CENTER,
                                &user_handle,
                                egui::FontId::proportional(13.5),
                                TEXT_SECONDARY,
                            );

                            let action_label = if is_logged {
                                "Administra tu Cuenta de Google"
                            } else {
                                "Acceder con tu Cuenta de Google"
                            };
                            let action_color = if is_logged {
                                Color32::from_rgb(108, 168, 255)
                            } else {
                                ACCENT_RED
                            };
                            ui.painter().text(
                                Pos2::new(text_left, card_rect.top() + 62.0),
                                Align2::LEFT_CENTER,
                                action_label,
                                egui::FontId::proportional(13.0),
                                action_color,
                            );
                        }

                        if card_resp.clicked() {
                            self.show_account_modal = false;
                            #[cfg(target_os = "android")]
                            crate::android_media::open_google_login();
                            #[cfg(not(target_os = "android"))]
                            {
                                self.show_google_login = true;
                            }
                        }

                        ui.add_space(12.0);
                        let (_, div_rect) = ui.allocate_space(Vec2::new(inner_w, 1.0));
                        ui.painter().hline(
                            screen.x_range(),
                            div_rect.center().y,
                            Stroke::new(1.0_f32, Color32::from_white_alpha(28)),
                        );
                        ui.add_space(6.0);

                        // Menu Rows
                        let menu_row = |ui: &mut egui::Ui, icon: icons::Icon, title: &str| -> egui::Response {
                            let (rect, resp) = ui.allocate_exact_size(Vec2::new(inner_w, 46.0), Sense::click());
                            if ui.is_rect_visible(rect) {
                                let bg = if resp.hovered() {
                                    Color32::from_white_alpha(18)
                                } else {
                                    Color32::TRANSPARENT
                                };
                                if bg != Color32::TRANSPARENT {
                                    ui.painter().rect_filled(rect, CornerRadius::same(8), bg);
                                }
                                let icon_rect = Rect::from_center_size(
                                    Pos2::new(rect.left() + 18.0, rect.center().y),
                                    Vec2::splat(18.0),
                                );
                                icons::draw_icon(ui.painter(), icon_rect, icon, Color32::WHITE);
                                ui.painter().text(
                                    Pos2::new(rect.left() + 46.0, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    title,
                                    egui::FontId::proportional(14.5),
                                    Color32::WHITE,
                                );
                            }
                            resp
                        };

                        if menu_row(ui, icons::Icon::Channel, "Tu canal").clicked() {
                            self.show_account_modal = false;
                            self.active_tab = NavTab::Library;
                            self.set_banner("Abriendo tu canal y biblioteca");
                        }
                        if menu_row(ui, icons::Icon::Download, "Descargas").clicked() {
                            self.show_account_modal = false;
                            self.active_tab = NavTab::Library;
                            self.library_filter = LibraryFilter::Downloads;
                        }
                        if menu_row(ui, icons::Icon::History, "Historial").clicked() {
                            self.show_account_modal = false;
                            self.active_tab = NavTab::Library;
                            self.library_filter = LibraryFilter::History;
                        }
                        if menu_row(ui, icons::Icon::Recap, "Tu Recap").clicked() {
                            self.show_account_modal = false;
                            let count = self.store.data.recent_tracks.len();
                            let liked = self.store.data.liked_tracks.len();
                            self.set_banner(format!("Tu Recap: {} canciones escuchadas • {} favoritas", count, liked));
                        }
                        if menu_row(ui, icons::Icon::Badge, "Insignias").clicked() {
                            self.show_account_modal = false;
                            self.set_banner("Insignia: Oyente Audiófilo Hi-Fi Rust");
                        }
                        if menu_row(ui, icons::Icon::SwitchAccount, "Cambiar de cuenta").clicked() {
                            self.show_account_modal = false;
                            #[cfg(target_os = "android")]
                            crate::android_media::open_google_login();
                            #[cfg(not(target_os = "android"))]
                            {
                                self.show_google_login = true;
                            }
                        }
                        if is_logged && menu_row(ui, icons::Icon::Close, "Cerrar sesión de Google").clicked() {
                            self.show_account_modal = false;
                            self.logout_google(ctx);
                            self.set_banner("Sesión de Google cerrada");
                        }
                        if menu_row(ui, icons::Icon::Settings, "Configuración").clicked() {
                            self.show_account_modal = false;
                            self.show_settings = true;
                            self.settings_category = SettingsCategory::Root;
                        }
                        if menu_row(ui, icons::Icon::Help, "Ayuda y comentarios").clicked() {
                            self.show_account_modal = false;
                            self.set_banner("YR Music v0.7.4 • Pure Rust");
                        }

                        #[cfg(target_os = "android")]
                        let footer_reserve = 78.0;
                        #[cfg(not(target_os = "android"))]
                        let footer_reserve = 36.0;
                        let rem_h = (screen.bottom() - ui.cursor().top() - footer_reserve).max(14.0);
                        ui.add_space(rem_h);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            let pad = (inner_w - 230.0).max(0.0) * 0.5;
                            if pad > 0.0 {
                                ui.add_space(pad);
                            }
                            ui.label(
                                RichText::new("Polít. de Privacidad • Condic. del Servicio")
                                    .size(11.0)
                                    .color(TEXT_SECONDARY),
                            );
                        });
                        #[cfg(target_os = "android")]
                        ui.add_space(46.0);
                        #[cfg(not(target_os = "android"))]
                        ui.add_space(8.0);
                    });
            });
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
    ui.add(egui::Label::new(job).wrap().selectable(false));
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

fn render_artist_card_row(
    ui: &mut egui::Ui,
    id_salt: (&str, &str),
    avatar_url: Option<&str>,
    name: &str,
    subtitle: Option<&str>,
    fallback_icon: icons::Icon,
) -> bool {
    let mut clicked = false;
    egui::Frame::new()
        .fill(BG_SURFACE)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(8.0)
        .show(ui, |ui| {
            let row_start = ui.cursor().min;
            let row_w = ui.available_width();
            ui.horizontal(|ui| {
                let (av_rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
                ui.painter()
                    .circle_filled(av_rect.center(), 24.0, BG_ELEVATED);
                icons::draw_icon(
                    ui.painter(),
                    egui::Rect::from_center_size(av_rect.center(), Vec2::splat(22.0)),
                    fallback_icon,
                    TEXT_SECONDARY,
                );
                if let Some(u) = avatar_url {
                    paint_ready_image(
                        ui,
                        av_rect,
                        tracked_thumb_uri(u, 160),
                        Vec2::splat(48.0),
                        CornerRadius::same(24),
                    );
                }
                let text_w = (row_w - 86.0).max(110.0);
                ui.allocate_ui(Vec2::new(text_w, 48.0), |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.set_max_width(text_w);
                        render_clamped_label(ui, name, 15.0, TEXT_PRIMARY, text_w, 1);
                        if let Some(sub) = subtitle {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(sub).size(12.5).color(TEXT_SECONDARY),
                                )
                                .truncate()
                                .selectable(false),
                            );
                        }
                    });
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (ch_rect, _) =
                        ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    icons::draw_icon(ui.painter(), ch_rect, icons::Icon::ChevronRight, TEXT_SECONDARY);
                });
            });
            let hit_rect = egui::Rect::from_min_size(row_start, Vec2::new(row_w, 48.0));
            if ui
                .interact(hit_rect, ui.id().with(id_salt), Sense::click())
                .clicked()
            {
                clicked = true;
            }
        });
    clicked
}

fn settings_choice_pill(ui: &mut egui::Ui, selected: bool, label: &str) -> bool {
    let btn = egui::Button::new(
        RichText::new(label)
            .size(13.0)
            .strong()
            .color(Color32::WHITE),
    )
    .fill(if selected { ACCENT_RED } else { BG_ELEVATED })
    .stroke(if selected {
        Stroke::NONE
    } else {
        Stroke::new(0.8_f32, Color32::from_rgb(56, 56, 66))
    })
    .corner_radius(CornerRadius::same(16))
    .min_size(Vec2::new(32.0, 32.0));
    ui.add(btn).clicked()
}

fn settings_action_button(ui: &mut egui::Ui, label: &str) -> bool {
    let btn = egui::Button::new(RichText::new(label).size(13.5).color(Color32::WHITE))
        .fill(BG_ELEVATED)
        .stroke(Stroke::new(0.8_f32, Color32::from_rgb(56, 56, 66)))
        .corner_radius(CornerRadius::same(16))
        .min_size(Vec2::new(140.0, 34.0));
    ui.add(btn).clicked()
}

/// Paints a remote image only once ready so failed loads preserve the clean vector placeholder.
fn paint_ready_image(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    uri: String,
    size: Vec2,
    corner_radius: CornerRadius,
) -> bool {
    let img = Image::new(uri)
        .fit_to_exact_size(size)
        .corner_radius(corner_radius);
    match img.load_for_size(ui.ctx(), size) {
        Ok(egui::load::TexturePoll::Ready { .. }) => {
            img.paint_at(ui, rect);
            true
        }
        Ok(egui::load::TexturePoll::Pending { .. }) => {
            ui.ctx().request_repaint();
            false
        }
        Err(_) => false,
    }
}

/// Paints a high-res remote image if ready, otherwise falling back to an already-cached 160px thumbnail.
fn paint_progressive_image(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    url: &str,
    preferred_max_px: u32,
    size: Vec2,
    corner_radius: CornerRadius,
) {
    let hi_uri = tracked_thumb_uri(url, preferred_max_px);
    if !paint_ready_image(ui, rect, hi_uri, size, corner_radius) {
        let fallback_uri = tracked_thumb_uri(url, 160);
        paint_ready_image(ui, rect, fallback_uri, size, corner_radius);
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

/// Prunes oldest regular files in `dir` until total disk usage is at or below `max_bytes`.
fn prune_dir_to_max_bytes(dir: &std::path::Path, max_bytes: u64) -> u64 {
    fn collect_files(
        path: &std::path::Path,
        out: &mut Vec<(std::time::SystemTime, u64, PathBuf)>,
    ) -> u64 {
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };
        let mut sum = 0_u64;
        for entry in entries.flatten() {
            let p = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                sum = sum.saturating_add(collect_files(&p, out));
            } else if meta.is_file() {
                let len = meta.len();
                let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                out.push((mtime, len, p));
                sum = sum.saturating_add(len);
            }
        }
        sum
    }

    let mut files = Vec::new();
    let mut total = collect_files(dir, &mut files);
    if total <= max_bytes {
        return total;
    }
    files.sort_by_key(|(mtime, _, _)| *mtime);
    for (_, len, path) in files {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
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
