use crate::theme::{
    apply_ytm_theme, ACCENT_RED, BG_ELEVATED, BG_PRIMARY, BG_SURFACE, GREEN_OK, TEXT_PRIMARY,
    TEXT_SECONDARY,
};
use eframe::CreationContext;
use egui::{Align, Color32, CornerRadius, Image, Layout, RichText, Sense, Stroke, Vec2};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use yr_audio::{AudioEngine, AudioSnapshot};
use yr_core::{
    format_seconds, AlbumDetail, ArtistDetail, AudioQuality, HomeFeed, LibraryStore,
    PlaybackQueue, PlaybackStatus, PlaylistDetail, PlaylistSummary, SearchResults, Track,
};
use yr_download::{DownloadManager, DownloadStatus, DownloadTask};
use yr_provider::{MusicProvider, YtMusicProvider};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavTab {
    Home,
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
    SearchLoaded(Result<SearchResults, String>),
    AlbumLoaded(Result<AlbumDetail, String>),
    PlaylistLoaded(Result<PlaylistDetail, String>),
    ArtistLoaded(Result<ArtistDetail, String>),
    MoodLoaded {
        title: String,
        result: Result<Vec<PlaylistSummary>, String>,
    },
    RadioLoaded(Result<Vec<Track>, String>),
    LyricsLoaded {
        track_id: String,
        result: Result<Option<String>, String>,
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
}

pub struct YrApp {
    runtime: tokio::runtime::Runtime,
    provider: Arc<dyn MusicProvider>,
    downloader: Arc<DownloadManager>,
    audio: AudioEngine,
    store: LibraryStore,
    queue: PlaybackQueue,
    data_dir: PathBuf,

    event_tx: mpsc::UnboundedSender<AppEvent>,
    event_rx: mpsc::UnboundedReceiver<AppEvent>,

    active_tab: NavTab,
    search_query: String,
    search_filter: SearchFilter,
    search_results: SearchResults,
    search_loading: bool,
    search_suggestions: Vec<String>,
    last_search_input_at: Option<Instant>,
    suggestions_last_query: String,

    home_feed: HomeFeed,
    home_loading: bool,
    detail_view: DetailView,
    detail_loading: bool,

    downloads: Vec<DownloadTask>,
    total_downloaded_bytes: u64,
    last_download_poll: Instant,

    now_playing_open: bool,
    show_queue: bool,
    show_lyrics: bool,
    lyrics_track_id: Option<String>,
    lyrics_text: Option<String>,
    lyrics_loading: bool,
    show_settings: bool,
    new_playlist_name: String,

    sleep_timer_until: Option<Instant>,
    sleep_after_track: bool,
    playlist_picker_track: Option<Track>,

    last_finished_counter: u64,
    last_media_sync: Option<(String, bool, Instant)>,
    status_banner: Option<(String, Instant)>,
    seeking_secs: Option<f64>,
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

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let mut app = Self {
            runtime,
            provider,
            downloader,
            audio,
            store,
            queue: PlaybackQueue::new(),
            data_dir,
            event_tx,
            event_rx,
            active_tab: NavTab::Home,
            search_query: String::new(),
            search_filter: SearchFilter::Songs,
            search_results: SearchResults::default(),
            search_loading: false,
            search_suggestions: Vec::new(),
            last_search_input_at: None,
            suggestions_last_query: String::new(),
            home_feed: HomeFeed::default(),
            home_loading: false,
            detail_view: DetailView::None,
            detail_loading: false,
            downloads: Vec::new(),
            total_downloaded_bytes: 0,
            last_download_poll: Instant::now() - Duration::from_secs(10),
            now_playing_open: false,
            show_queue: false,
            show_lyrics: false,
            lyrics_track_id: None,
            lyrics_text: None,
            lyrics_loading: false,
            show_settings: false,
            new_playlist_name: String::new(),
            sleep_timer_until: None,
            sleep_after_track: false,
            playlist_picker_track: None,
            last_finished_counter: 0,
            last_media_sync: None,
            status_banner: None,
            seeking_secs: None,
        };

        let (media_tx, mut media_rx) = mpsc::unbounded_channel();
        crate::android_media::set_action_sender(media_tx);
        let audio_bg = app.audio.clone();
        let ev_tx_bg = app.event_tx.clone();
        let ctx_bg = cc.egui_ctx.clone();
        app.runtime.spawn(async move {
            use crate::android_media::MediaAction;
            while let Some(act) = media_rx.recv().await {
                match act {
                    MediaAction::Play => audio_bg.resume(),
                    MediaAction::Pause => audio_bg.pause(),
                    MediaAction::Toggle => {
                        if audio_bg.snapshot().status == PlaybackStatus::Playing {
                            audio_bg.pause();
                        } else {
                            audio_bg.resume();
                        }
                    }
                    MediaAction::SeekMs(ms) => audio_bg.seek(Duration::from_millis(ms)),
                    MediaAction::Stop => audio_bg.stop(),
                    MediaAction::Next | MediaAction::Prev => {}
                }
                let _ = ev_tx_bg.send(AppEvent::MediaAction(act));
                ctx_bg.request_repaint();
            }
        });

        app.refresh_home(&cc.egui_ctx);
        app.refresh_downloads(&cc.egui_ctx);
        app
    }

    fn set_banner(&mut self, msg: impl Into<String>) {
        self.status_banner = Some((msg.into(), Instant::now()));
    }

    fn refresh_home(&mut self, ctx: &egui::Context) {
        self.home_loading = true;
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider.get_home_feed().await.map_err(|e| e.to_string());
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

    fn load_lyrics(&mut self, track_id: String, ctx: &egui::Context) {
        self.lyrics_loading = true;
        self.lyrics_track_id = Some(track_id.clone());
        self.lyrics_text = None;
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider
                .get_lyrics(&track_id)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::LyricsLoaded {
                track_id,
                result: res,
            });
            ctx.request_repaint();
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
        self.queue.set_queue(tracks, index);
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
        let Some(track) = self.queue.current_track().cloned() else {
            return;
        };
        self.store.record_play(track.clone());
        self.seeking_secs = None;
        self.lyrics_text = None;
        self.lyrics_track_id = None;
        if self.show_lyrics {
            self.load_lyrics(track.id.clone(), ctx);
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
        let ctx = ctx.clone();
        let audio = self.audio.clone();

        self.runtime.spawn(async move {
            if let Some(local_path) = dl.find_local_path(&track.id).await {
                audio.play_file(local_path, track.duration_secs);
                ctx.request_repaint();
                return;
            }
            let res = provider
                .resolve_audio_stream(&track.id, quality)
                .await
                .map(|s| s.url)
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::StreamResolved { track, result: res });
            ctx.request_repaint();
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
                        Ok(feed) => self.home_feed = feed,
                        Err(e) => self.set_banner(format!("Sin conexión: {e}")),
                    }
                }
                AppEvent::SearchLoaded(res) => {
                    self.search_loading = false;
                    match res {
                        Ok(r) => self.search_results = r,
                        Err(e) => self.set_banner(format!("Error al buscar: {e}")),
                    }
                }
                AppEvent::AlbumLoaded(res) => {
                    self.detail_loading = false;
                    match res {
                        Ok(a) => self.detail_view = DetailView::Album(a),
                        Err(e) => self.set_banner(format!("Error al abrir álbum: {e}")),
                    }
                }
                AppEvent::PlaylistLoaded(res) => {
                    self.detail_loading = false;
                    match res {
                        Ok(p) => self.detail_view = DetailView::Playlist(p),
                        Err(e) => self.set_banner(format!("Error al abrir playlist: {e}")),
                    }
                }
                AppEvent::ArtistLoaded(res) => {
                    self.detail_loading = false;
                    match res {
                        Ok(a) => self.detail_view = DetailView::Artist(a),
                        Err(e) => self.set_banner(format!("Error al abrir artista: {e}")),
                    }
                }
                AppEvent::MoodLoaded { title, result } => {
                    self.detail_loading = false;
                    match result {
                        Ok(playlists) => {
                            self.detail_view = DetailView::Mood { title, playlists }
                        }
                        Err(e) => self.set_banner(format!("Error al abrir género: {e}")),
                    }
                }
                AppEvent::RadioLoaded(res) => match res {
                    Ok(tracks) if !tracks.is_empty() => {
                        for t in tracks {
                            self.queue.enqueue(t);
                        }
                        self.set_banner("Radio añadida a la cola");
                    }
                    Ok(_) => self.set_banner("No se encontraron canciones similares"),
                    Err(e) => self.set_banner(format!("Error de radio: {e}")),
                },
                AppEvent::LyricsLoaded { track_id, result } => {
                    if self.lyrics_track_id.as_deref() == Some(&track_id) {
                        self.lyrics_loading = false;
                        self.lyrics_text = match result {
                            Ok(Some(text)) => Some(text),
                            Ok(None) => Some("Letra no disponible para esta canción.".into()),
                            Err(e) => Some(format!("Error al cargar letra: {e}")),
                        };
                    }
                }
                AppEvent::StreamResolved { track, result } => match result {
                    Ok(url) => {
                        if self.queue.current_track().map(|t| t.id.as_str()) == Some(&track.id) {
                            self.audio.play_url(url, track.id, track.duration_secs);
                        }
                    }
                    Err(e) => self.set_banner(format!("No se pudo reproducir: {e}")),
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
                    match act {
                        MediaAction::Next => {
                            if self.queue.next_track(true).is_some() {
                                self.play_current_queue_track(ctx);
                            }
                        }
                        MediaAction::Prev => {
                            if self.queue.prev_track().is_some() {
                                self.play_current_queue_track(ctx);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if self.active_tab == NavTab::Search {
            self.maybe_fetch_suggestions(ctx);
        }

        if let Some(until) = self.sleep_timer_until {
            if Instant::now() >= until {
                self.sleep_timer_until = None;
                self.audio.pause();
                self.set_banner("Temporizador de apagado: reproducción pausada");
            } else {
                ctx.request_repaint_after(Duration::from_secs(1));
            }
        }

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
        if snap.finished_counter > self.last_finished_counter {
            self.last_finished_counter = snap.finished_counter;
            if self.sleep_after_track {
                self.sleep_after_track = false;
                self.audio.pause();
                self.set_banner("Temporizador: detenido al finalizar la canción");
            } else if self.queue.next_track(false).is_some() {
                self.play_current_queue_track(ctx);
            }
        }
        if matches!(
            snap.status,
            PlaybackStatus::Playing | PlaybackStatus::Buffering | PlaybackStatus::Loading
        ) {
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        if let Some(track) = self.queue.current_track() {
            let is_playing = snap.status == PlaybackStatus::Playing;
            let should_sync = match &self.last_media_sync {
                Some((id, prev_playing, at)) => {
                    id != &track.id
                        || *prev_playing != is_playing
                        || (is_playing && at.elapsed() >= Duration::from_secs(2))
                }
                None => true,
            };
            if should_sync {
                let pos_ms = (snap.position_secs * 1000.0).max(0.0) as u64;
                let dur_ms = snap
                    .duration_secs
                    .or(track.duration_secs.map(|d| d as f64))
                    .map(|d| (d * 1000.0).max(0.0) as u64)
                    .unwrap_or(0);
                crate::android_media::update_playback(
                    &track.title,
                    &track.artist,
                    is_playing,
                    pos_ms,
                    dur_ms,
                );
                self.last_media_sync = Some((track.id.clone(), is_playing, Instant::now()));
            }
        }
    }
}

impl eframe::App for YrApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_events(ctx);
        let snap = self.audio.snapshot();

        // Top Header Bar
        egui::TopBottomPanel::top("yr_top_bar")
            .frame(egui::Frame::new().fill(BG_PRIMARY).inner_margin(12.0))
            .show(ctx, |ui| {
                #[cfg(target_os = "android")]
                ui.add_space(20.0);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("▶ YR")
                            .size(22.0)
                            .strong()
                            .color(ACCENT_RED),
                    );
                    ui.label(
                        RichText::new("Music")
                            .size(20.0)
                            .strong()
                            .color(TEXT_PRIMARY),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("⚙").clicked() {
                            self.show_settings = !self.show_settings;
                        }
                    });
                });

                if let Some((msg, at)) = &self.status_banner {
                    if at.elapsed() < Duration::from_secs(4) {
                        ui.label(RichText::new(msg).small().color(ACCENT_RED));
                    }
                }
            });

        // Bottom Navigation + Mini-Player
        egui::TopBottomPanel::bottom("yr_bottom_bar")
            .frame(egui::Frame::new().fill(BG_SURFACE).inner_margin(8.0))
            .show(ctx, |ui| {
                if !self.now_playing_open {
                    if let Some(track) = self.queue.current_track().cloned() {
                        self.render_mini_player(ui, ctx, &track, &snap);
                        ui.add_space(4.0);
                    }
                }

                ui.columns(4, |cols| {
                    self.nav_button(&mut cols[0], NavTab::Home, "Inicio");
                    self.nav_button(&mut cols[1], NavTab::Search, "Buscar");
                    self.nav_button(&mut cols[2], NavTab::Library, "Biblioteca");
                    let dl_count = self
                        .downloads
                        .iter()
                        .filter(|d| d.status == DownloadStatus::Downloading)
                        .count();
                    let dl_label = if dl_count > 0 {
                        format!("Descargas ({dl_count})")
                    } else {
                        "Descargas".to_string()
                    };
                    self.nav_button(&mut cols[3], NavTab::Downloads, &dl_label);
                });

                #[cfg(target_os = "android")]
                ui.add_space(48.0);
            });

        // Main Content Area
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG_PRIMARY).inner_margin(12.0))
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
                    NavTab::Search => self.render_search(ui, ctx),
                    NavTab::Library => self.render_library(ui, ctx),
                    NavTab::Downloads => self.render_downloads(ui, ctx),
                }
            });

        self.render_playlist_picker_modal(ctx);
    }
}

impl YrApp {
    fn nav_button(&mut self, ui: &mut egui::Ui, tab: NavTab, label: &str) {
        let selected = self.active_tab == tab && !self.now_playing_open && !self.show_settings;
        let text = RichText::new(label)
            .size(13.0)
            .color(if selected { ACCENT_RED } else { TEXT_SECONDARY });
        let btn = egui::Button::new(text)
            .fill(if selected { BG_ELEVATED } else { Color32::TRANSPARENT })
            .min_size(Vec2::new(ui.available_width(), 38.0));
        if ui.add(btn).clicked() {
            self.active_tab = tab;
            self.now_playing_open = false;
            self.show_settings = false;
            self.detail_view = DetailView::None;
        }
    }

    fn render_detail_if_active(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        if self.detail_loading {
            ui.horizontal(|ui| {
                if ui.button("← Volver").clicked() {
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
                    if ui.button("← Volver").clicked() {
                        self.detail_view = DetailView::None;
                    }
                    ui.heading(&album.summary.title);
                });
                ui.horizontal(|ui| {
                    render_thumb(ui, album.summary.thumbnail_url.as_deref(), 64.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&album.summary.artist).color(TEXT_SECONDARY));
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    egui::Button::new(RichText::new("▶ Reproducir").color(Color32::WHITE))
                                        .fill(ACCENT_RED),
                                )
                                .clicked()
                                && !album.tracks.is_empty()
                            {
                                self.play_track_in_list(album.tracks.clone(), 0, ctx);
                            }
                            if ui.button("⬇ Descargar todo").clicked() && !album.tracks.is_empty() {
                                self.enqueue_download_all(album.tracks.clone(), ctx);
                            }
                        });
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
                    if ui.button("← Volver").clicked() {
                        self.detail_view = DetailView::None;
                    }
                    ui.heading(&pl.summary.title);
                });
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("▶ Reproducir").color(Color32::WHITE))
                                .fill(ACCENT_RED),
                        )
                        .clicked()
                        && !pl.tracks.is_empty()
                    {
                        self.play_track_in_list(pl.tracks.clone(), 0, ctx);
                    }
                    if ui.button("⬇ Descargar todo").clicked() && !pl.tracks.is_empty() {
                        self.enqueue_download_all(pl.tracks.clone(), ctx);
                    }
                });
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
                    if ui.button("← Volver").clicked() {
                        self.detail_view = DetailView::None;
                    }
                    ui.heading(&artist.summary.name);
                });
                if let Some(subs) = &artist.summary.subscribers {
                    ui.label(RichText::new(subs).small().color(TEXT_SECONDARY));
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
                    if ui.button("← Volver").clicked() {
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

    fn render_home(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Mood / Genre Chips row (YT Music style)
            if !self.home_feed.moods.is_empty() {
                let moods = self.home_feed.moods.clone();
                egui::ScrollArea::horizontal()
                    .id_salt("home_moods")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for mood in moods {
                                let btn = egui::Button::new(
                                    RichText::new(&mood.title).size(13.0).color(TEXT_PRIMARY),
                                )
                                .fill(BG_ELEVATED)
                                .corner_radius(CornerRadius::same(16));
                                if ui.add(btn).clicked() {
                                    self.open_mood(mood.id, mood.title, ctx);
                                }
                            }
                        });
                    });
                ui.add_space(8.0);
            }

            if !self.store.data.recent_tracks.is_empty() {
                ui.label(
                    RichText::new("Volver a escuchar")
                        .size(17.0)
                        .strong()
                        .color(TEXT_PRIMARY),
                );
                let recents = self.store.data.recent_tracks.clone();
                egui::ScrollArea::horizontal()
                    .id_salt("home_recent_cards")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (idx, track) in recents.iter().take(12).enumerate() {
                                egui::Frame::new()
                                    .fill(BG_SURFACE)
                                    .corner_radius(CornerRadius::same(10))
                                    .inner_margin(8.0)
                                    .show(ui, |ui| {
                                        ui.set_width(116.0);
                                        ui.vertical(|ui| {
                                            render_thumb(
                                                ui,
                                                track.thumbnail_url.as_deref(),
                                                100.0,
                                            );
                                            ui.add_space(4.0);
                                            if ui
                                                .add(
                                                    egui::Label::new(
                                                        RichText::new(&track.title)
                                                            .strong()
                                                            .size(12.5),
                                                    )
                                                    .truncate()
                                                    .sense(Sense::click()),
                                                )
                                                .clicked()
                                            {
                                                self.play_track_in_list(recents.clone(), idx, ctx);
                                            }
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&track.artist)
                                                        .size(11.0)
                                                        .color(TEXT_SECONDARY),
                                                )
                                                .truncate(),
                                            );
                                        });
                                    });
                            }
                        });
                    });
                ui.add_space(10.0);
            }

            ui.horizontal(|ui| {
                ui.heading(RichText::new("Selección rápida").color(TEXT_PRIMARY));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("↻ Actualizar").clicked() {
                        self.refresh_home(ctx);
                    }
                });
            });

            if self.home_loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Cargando música de YT Music...").color(TEXT_SECONDARY));
                });
            }

            if !self.home_feed.quick_picks.is_empty() {
                let picks = self.home_feed.quick_picks.clone();
                for (idx, track) in picks.iter().take(15).enumerate() {
                    self.render_track_row(ui, ctx, track, &picks, idx, false);
                }
            }

            if !self.home_feed.new_albums.is_empty() {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Nuevos lanzamientos")
                        .size(17.0)
                        .strong()
                        .color(TEXT_PRIMARY),
                );
                let albums = self.home_feed.new_albums.clone();
                egui::ScrollArea::horizontal()
                    .id_salt("home_new_albums")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for album in albums.iter().take(12) {
                                egui::Frame::new()
                                    .fill(BG_SURFACE)
                                    .corner_radius(CornerRadius::same(10))
                                    .inner_margin(8.0)
                                    .show(ui, |ui| {
                                        ui.set_width(126.0);
                                        ui.vertical(|ui| {
                                            render_thumb(
                                                ui,
                                                album.thumbnail_url.as_deref(),
                                                110.0,
                                            );
                                            ui.add_space(4.0);
                                            if ui
                                                .add(
                                                    egui::Label::new(
                                                        RichText::new(&album.title)
                                                            .strong()
                                                            .size(13.0),
                                                    )
                                                    .truncate()
                                                    .sense(Sense::click()),
                                                )
                                                .clicked()
                                            {
                                                self.open_album(album.id.clone(), ctx);
                                            }
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&album.artist)
                                                        .small()
                                                        .color(TEXT_SECONDARY),
                                                )
                                                .truncate(),
                                            );
                                        });
                                    });
                            }
                        });
                    });
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
    }

    fn render_search(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.search_query)
                    .hint_text("Buscar canciones, álbumes, artistas...")
                    .desired_width(ui.available_width() - 88.0),
            );
            if resp.changed() {
                self.last_search_input_at = Some(Instant::now());
            }
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Buscar").clicked() || enter {
                self.trigger_search(ctx);
            }
        });

        if !self.search_suggestions.is_empty() && !self.search_loading {
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

        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.search_filter, SearchFilter::Songs, "Canciones");
            ui.selectable_value(&mut self.search_filter, SearchFilter::Albums, "Álbumes");
            ui.selectable_value(&mut self.search_filter, SearchFilter::Artists, "Artistas");
            ui.selectable_value(
                &mut self.search_filter,
                SearchFilter::Playlists,
                "Playlists",
            );
        });

        if self.search_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Buscando en YT Music...").color(TEXT_SECONDARY));
            });
        }

        // Quick search chips & search history when results are empty
        if self.search_results.songs.is_empty() && !self.search_loading {
            if !self.store.data.search_history.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Búsquedas recientes")
                        .small()
                        .color(TEXT_SECONDARY),
                );
                let history = self.store.data.search_history.clone();
                ui.horizontal_wrapped(|ui| {
                    for item in history.iter().take(10) {
                        ui.horizontal(|ui| {
                            if ui.button(item).clicked() {
                                self.search_query = item.clone();
                                self.trigger_search(ctx);
                            }
                            if ui.small_button("✕").clicked() {
                                self.store.remove_search_history(item);
                            }
                        });
                    }
                });
            }

            ui.add_space(6.0);
            ui.label(RichText::new("Búsquedas populares").small().color(TEXT_SECONDARY));
            ui.horizontal_wrapped(|ui| {
                for preset in ["Bad Bunny", "Daft Punk", "Feid", "Lo-Fi Beats", "Rock", "Pop"] {
                    if ui.button(preset).clicked() {
                        self.search_query = preset.to_string();
                        self.trigger_search(ctx);
                    }
                }
            });
        }

        egui::ScrollArea::vertical().show(ui, |ui| match self.search_filter {
            SearchFilter::Songs => {
                let songs = self.search_results.songs.clone();
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
                                            RichText::new(subs).small().color(TEXT_SECONDARY),
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
        });
    }

    fn render_album_row(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        album: &yr_core::AlbumSummary,
    ) {
        egui::Frame::new()
            .fill(BG_SURFACE)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    render_thumb(ui, album.thumbnail_url.as_deref(), 48.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&album.title).strong());
                        ui.label(RichText::new(&album.artist).small().color(TEXT_SECONDARY));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Abrir").clicked() {
                            self.open_album(album.id.clone(), ctx);
                        }
                    });
                });
            });
    }

    fn render_playlist_row(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        pl: &PlaylistSummary,
    ) {
        egui::Frame::new()
            .fill(BG_SURFACE)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    render_thumb(ui, pl.thumbnail_url.as_deref(), 48.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&pl.title).strong());
                        if let Some(author) = &pl.author {
                            ui.label(RichText::new(author).small().color(TEXT_SECONDARY));
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Abrir").clicked() {
                            self.open_playlist(pl.id.clone(), ctx);
                        }
                    });
                });
            });
    }

    fn render_library(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Biblioteca");

            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_playlist_name)
                        .hint_text("Nueva playlist..."),
                );
                if ui.button("+ Crear").clicked() && !self.new_playlist_name.trim().is_empty() {
                    let name = self.new_playlist_name.trim().to_string();
                    self.store.create_playlist(name);
                    self.new_playlist_name.clear();
                }
            });

            ui.add_space(10.0);
            ui.label(
                RichText::new(format!("Me gusta ({})", self.store.data.liked_tracks.len()))
                    .size(16.0)
                    .strong(),
            );
            let liked = self.store.data.liked_tracks.clone();
            for (idx, track) in liked.iter().enumerate() {
                self.render_track_row(ui, ctx, track, &liked, idx, false);
            }

            if !self.store.data.playlists.is_empty() {
                ui.add_space(12.0);
                ui.label(RichText::new("Tus playlists").size(16.0).strong());
                let pls = self.store.data.playlists.clone();
                for pl in pls {
                    ui.horizontal(|ui| {
                        egui::CollapsingHeader::new(format!("{} ({})", pl.name, pl.tracks.len()))
                            .id_salt(&pl.id)
                            .show(ui, |ui| {
                                for (idx, track) in pl.tracks.iter().enumerate() {
                                    self.render_track_row(ui, ctx, track, &pl.tracks, idx, false);
                                }
                            });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("🗑").clicked() {
                                self.store.delete_playlist(&pl.id);
                                self.set_banner(format!("Playlist '{}' eliminada", pl.name));
                            }
                        });
                    });
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
                if ui.button("↻").clicked() {
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

        if !completed.is_empty() && ui.button("▶ Reproducir todo offline").clicked() {
            self.play_track_in_list(completed.clone(), 0, ctx);
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            if self.downloads.is_empty() {
                ui.add_space(16.0);
                ui.label(
                    RichText::new("Aún no tienes descargas. Toca ⬇ en cualquier canción.")
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
                                        if ui.button("✕").clicked() {
                                            let dl = Arc::clone(&self.downloader);
                                            let id = task.track.id.clone();
                                            self.runtime.spawn(async move { dl.cancel(&id).await });
                                        }
                                        if ui.button("⏸").clicked() {
                                            let dl = Arc::clone(&self.downloader);
                                            let id = task.track.id.clone();
                                            self.runtime.spawn(async move { dl.pause(&id).await });
                                        }
                                    }
                                    DownloadStatus::Paused | DownloadStatus::Failed(_) => {
                                        if ui.button("✕").clicked() {
                                            let dl = Arc::clone(&self.downloader);
                                            let id = task.track.id.clone();
                                            self.runtime.spawn(async move { dl.cancel(&id).await });
                                        }
                                        if ui.button("▶").clicked() {
                                            let dl = Arc::clone(&self.downloader);
                                            let id = task.track.id.clone();
                                            self.runtime.spawn(async move { dl.resume(&id).await });
                                        }
                                    }
                                    DownloadStatus::Completed => {
                                        if ui.button("🗑").clicked() {
                                            let dl = Arc::clone(&self.downloader);
                                            let id = task.track.id.clone();
                                            self.runtime.spawn(async move {
                                                dl.delete_downloaded(&id).await;
                                            });
                                        }
                                        if ui.button("▶").clicked() {
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
                    render_thumb(ui, track.thumbnail_url.as_deref(), 48.0);

                    let text_width = (ui.available_width() - 44.0).max(120.0);
                    let display_title = if track.title.chars().count() > 78 {
                        let clipped: String = track.title.chars().take(75).collect();
                        format!("{clipped}...")
                    } else {
                        track.title.clone()
                    };
                    let is_downloaded = self.downloads.iter().any(|d| {
                        d.track.id == track.id && d.status == DownloadStatus::Completed
                    });

                    let info_resp = ui
                        .allocate_ui(Vec2::new(text_width, 50.0), |ui| {
                            ui.vertical(|ui| {
                                ui.set_max_width(text_width);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(display_title).strong().color(if is_current {
                                            ACCENT_RED
                                        } else {
                                            TEXT_PRIMARY
                                        }),
                                    )
                                    .wrap(),
                                );
                                let sub = if offline_badge || is_downloaded {
                                    format!("✓ {} • {}", track.artist, track.formatted_duration())
                                } else {
                                    format!("{} • {}", track.artist, track.formatted_duration())
                                };
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(sub).small().color(if is_downloaded {
                                            GREEN_OK
                                        } else {
                                            TEXT_SECONDARY
                                        }),
                                    )
                                    .truncate(),
                                );
                            });
                        })
                        .response
                        .interact(Sense::click());

                    if info_resp.clicked() {
                        self.play_track_in_list(list.to_vec(), index, ctx);
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.menu_button(RichText::new("...").size(16.0).strong(), |ui| {
                            if ui.button("▶ Reproducir").clicked() {
                                self.play_track_in_list(list.to_vec(), index, ctx);
                                ui.close_menu();
                            }
                            if ui.button("⏭ Reproducir siguiente").clicked() {
                                self.queue.enqueue_next(track.clone());
                                self.set_banner(format!("Siguiente: {}", track.title));
                                ui.close_menu();
                            }
                            let liked = self.store.is_liked(&track.id);
                            if ui
                                .button(if liked {
                                    "♥ Quitar de Me gusta"
                                } else {
                                    "♡ Me gusta"
                                })
                                .clicked()
                            {
                                self.store.toggle_like(track.clone());
                                ui.close_menu();
                            }
                            if ui.button("➕ Añadir a playlist").clicked() {
                                self.playlist_picker_track = Some(track.clone());
                                ui.close_menu();
                            }
                            if is_downloaded {
                                ui.label(RichText::new("✓ Descargada offline").color(GREEN_OK));
                            } else if ui.button("⬇ Descargar").clicked() {
                                self.enqueue_download(track.clone(), ctx);
                                ui.close_menu();
                            }
                            if ui.button("📻 Iniciar radio").clicked() {
                                self.start_radio(track.id.clone(), ctx);
                                ui.close_menu();
                            }
                        });
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
            .fill(BG_ELEVATED)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let thumb_resp = ui
                        .allocate_ui(Vec2::splat(40.0), |ui| {
                            render_thumb(ui, track.thumbnail_url.as_deref(), 40.0);
                        })
                        .response
                        .interact(Sense::click());
                    if thumb_resp.clicked() {
                        self.now_playing_open = true;
                    }

                    let text_width = (ui.available_width() - 132.0).max(80.0);
                    let resp = ui
                        .allocate_ui(Vec2::new(text_width, 40.0), |ui| {
                            ui.vertical(|ui| {
                                ui.set_max_width(text_width);
                                ui.add(
                                    egui::Label::new(RichText::new(&track.title).strong())
                                        .truncate(),
                                );
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&track.artist)
                                            .small()
                                            .color(TEXT_SECONDARY),
                                    )
                                    .truncate(),
                                );
                            });
                        })
                        .response
                        .interact(Sense::click());
                    if resp.clicked() {
                        self.now_playing_open = true;
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("⌃").clicked() {
                            self.now_playing_open = true;
                        }
                        if ui.button("⏭").clicked() && self.queue.next_track(true).is_some() {
                            self.play_current_queue_track(ctx);
                        }
                        let play_icon = match snap.status {
                            PlaybackStatus::Playing => "⏸",
                            _ => "▶",
                        };
                        if ui.button(play_icon).clicked() {
                            match snap.status {
                                PlaybackStatus::Playing => self.audio.pause(),
                                PlaybackStatus::Paused => self.audio.resume(),
                                _ => self.play_current_queue_track(ctx),
                            }
                        }
                    });
                });
            });

        let bar_rect = egui::Rect::from_min_size(
            frame_resp.response.rect.left_top(),
            Vec2::new(frame_resp.response.rect.width() * progress, 2.0),
        );
        ui.painter()
            .rect_filled(bar_rect, CornerRadius::same(1), ACCENT_RED);
    }

    fn render_full_player(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        snap: &AudioSnapshot,
    ) {
        let Some(track) = self.queue.current_track().cloned() else {
            self.now_playing_open = false;
            return;
        };

        // Top Bar: Minimize | Portada / Letra / Cola | Sleep Timer
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new("v").size(16.0).strong())
                        .fill(BG_ELEVATED)
                        .corner_radius(CornerRadius::same(16)),
                )
                .clicked()
            {
                self.now_playing_open = false;
            }

            ui.add_space(6.0);
            let show_cover = !self.show_queue && !self.show_lyrics;
            for (active, label, mode) in [
                (show_cover, "Portada", 0_u8),
                (self.show_lyrics, "Letra", 1_u8),
                (self.show_queue, "Cola", 2_u8),
            ] {
                let btn = egui::Button::new(
                    RichText::new(label)
                        .size(13.0)
                        .color(if active { BG_PRIMARY } else { TEXT_PRIMARY }),
                )
                .fill(if active { TEXT_PRIMARY } else { BG_ELEVATED })
                .corner_radius(CornerRadius::same(16));
                if ui.add(btn).clicked() {
                    self.show_lyrics = mode == 1;
                    self.show_queue = mode == 2;
                    if self.show_lyrics && self.lyrics_track_id.as_deref() != Some(&track.id) {
                        self.load_lyrics(track.id.clone(), ctx);
                    }
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let timer_label = if let Some(until) = self.sleep_timer_until {
                    let rem = until
                        .saturating_duration_since(Instant::now())
                        .as_secs();
                    format!("🌙 {}:{:02}", rem / 60, rem % 60)
                } else if self.sleep_after_track {
                    "🌙 Fin".to_string()
                } else {
                    "🌙".to_string()
                };
                ui.menu_button(timer_label, |ui| {
                    for mins in [15_u64, 30, 45, 60] {
                        if ui.button(format!("{mins} minutos")).clicked() {
                            self.sleep_timer_until =
                                Some(Instant::now() + Duration::from_secs(mins * 60));
                            self.sleep_after_track = false;
                            self.set_banner(format!("Temporizador: {mins} min"));
                            ui.close_menu();
                        }
                    }
                    if ui.button("Al finalizar la canción").clicked() {
                        self.sleep_timer_until = None;
                        self.sleep_after_track = true;
                        self.set_banner("Se detendrá al finalizar la canción");
                        ui.close_menu();
                    }
                    if (self.sleep_timer_until.is_some() || self.sleep_after_track)
                        && ui.button("Desactivar temporizador").clicked()
                    {
                        self.sleep_timer_until = None;
                        self.sleep_after_track = false;
                        ui.close_menu();
                    }
                });
            });
        });

        ui.add_space(6.0);

        if self.show_queue {
            ui.horizontal(|ui| {
                ui.heading(format!("Cola ({})", self.queue.len()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("📻 Añadir similares").clicked() {
                        self.start_radio(track.id.clone(), ctx);
                    }
                });
            });
            let ordered: Vec<(usize, Track)> = self
                .queue
                .ordered_tracks()
                .into_iter()
                .map(|(i, t)| (i, t.clone()))
                .collect();
            let current_idx = self.queue.current_index();
            let total_ordered = ordered.len();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (order_pos, (idx, t)) in ordered.into_iter().enumerate() {
                    let is_curr = current_idx == Some(idx);
                    egui::Frame::new()
                        .fill(if is_curr { BG_ELEVATED } else { BG_SURFACE })
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(6.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                render_thumb(ui, t.thumbnail_url.as_deref(), 40.0);
                                let tw = (ui.available_width() - 112.0).max(80.0);
                                let resp = ui
                                    .allocate_ui(Vec2::new(tw, 40.0), |ui| {
                                        ui.vertical(|ui| {
                                            ui.set_max_width(tw);
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&t.title).strong().color(
                                                        if is_curr { ACCENT_RED } else { TEXT_PRIMARY },
                                                    ),
                                                )
                                                .truncate(),
                                            );
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&t.artist)
                                                        .small()
                                                        .color(TEXT_SECONDARY),
                                                )
                                                .truncate(),
                                            );
                                        });
                                    })
                                    .response
                                    .interact(Sense::click());
                                if resp.clicked() {
                                    self.queue.jump_to(idx);
                                    self.play_current_queue_track(ctx);
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if !is_curr && ui.small_button("✕").clicked() {
                                        self.queue.remove_at(idx);
                                    }
                                    if order_pos + 1 < total_ordered
                                        && ui.small_button("↓").clicked()
                                    {
                                        self.queue.move_in_order(order_pos, 1);
                                    }
                                    if order_pos > 0 && ui.small_button("↑").clicked() {
                                        self.queue.move_in_order(order_pos, -1);
                                    }
                                });
                            });
                        });
                }
            });
            return;
        }

        if self.show_lyrics {
            ui.horizontal(|ui| {
                ui.heading("Letra");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("↻").clicked() {
                        self.load_lyrics(track.id.clone(), ctx);
                    }
                });
            });
            if self.lyrics_loading {
                ui.spinner();
            } else if let Some(text) = &self.lyrics_text {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new(text).size(15.5).color(TEXT_PRIMARY));
                });
            }
            return;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(10.0);
                let art_size = (ui.available_width() - 32.0).clamp(180.0, 270.0);
                let art_resp = egui::Frame::new()
                    .fill(BG_SURFACE)
                    .corner_radius(CornerRadius::same(16))
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        render_thumb(ui, track.thumbnail_url.as_deref(), art_size);
                    })
                    .response
                    .interact(Sense::drag());
                if art_resp.drag_stopped() {
                    let delta = ui.input(|i| {
                        i.pointer
                            .press_origin()
                            .zip(i.pointer.interact_pos())
                            .map(|(a, b)| b - a)
                    });
                    if let Some(d) = delta {
                        if d.y > 80.0 && d.y.abs() > d.x.abs() {
                            self.now_playing_open = false;
                        } else if d.x < -60.0 && self.queue.next_track(true).is_some() {
                            self.play_current_queue_track(ctx);
                        } else if d.x > 60.0 && self.queue.prev_track().is_some() {
                            self.play_current_queue_track(ctx);
                        }
                    }
                }

                ui.add_space(14.0);
                ui.label(RichText::new(&track.title).size(21.0).strong().color(TEXT_PRIMARY));
                ui.add_space(2.0);
                if let Some(artist_id) = track.artist_id.clone() {
                    if ui
                        .add(
                            egui::Label::new(
                                RichText::new(&track.artist).size(15.0).color(TEXT_SECONDARY),
                            )
                            .sense(Sense::click()),
                        )
                        .clicked()
                    {
                        self.now_playing_open = false;
                        self.open_artist(artist_id, ctx);
                    }
                } else {
                    ui.label(RichText::new(&track.artist).size(15.0).color(TEXT_SECONDARY));
                }

                ui.add_space(4.0);
                let is_offline = self
                    .downloads
                    .iter()
                    .any(|d| d.track.id == track.id && d.status == DownloadStatus::Completed);
                let source_badge = if is_offline {
                    "Offline • Sin anuncios"
                } else {
                    match self.store.data.config.stream_quality {
                        AudioQuality::Best => "Streaming • Máxima calidad",
                        AudioQuality::High => "Streaming • Alta calidad",
                        AudioQuality::Medium => "Streaming • Calidad media",
                        AudioQuality::Low => "Streaming • Ahorro de datos",
                    }
                };
                ui.label(
                    RichText::new(source_badge)
                        .size(11.5)
                        .color(if is_offline { GREEN_OK } else { TEXT_SECONDARY }),
                );

                match &snap.status {
                    PlaybackStatus::Buffering | PlaybackStatus::Loading => {
                        ui.label(RichText::new("Cargando audio...").small().color(ACCENT_RED));
                    }
                    PlaybackStatus::Error(err) => {
                        ui.label(RichText::new(err).small().color(ACCENT_RED));
                    }
                    _ => {}
                }

                ui.add_space(12.0);
                let total = snap
                    .duration_secs
                    .or(track.duration_secs.map(|d| d as f64))
                    .unwrap_or(1.0)
                    .max(1.0);
                let mut pos = self.seeking_secs.unwrap_or(snap.position_secs).min(total);
                let slider_w = (ui.available_width() - 16.0).max(160.0);
                ui.spacing_mut().slider_width = slider_w;
                let resp = ui.add(egui::Slider::new(&mut pos, 0.0..=total).show_value(false));
                if resp.dragged() {
                    self.seeking_secs = Some(pos);
                }
                if resp.drag_stopped() {
                    self.audio.seek(Duration::from_secs_f64(pos));
                    self.seeking_secs = None;
                }

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format_seconds(pos as u64))
                            .small()
                            .color(TEXT_SECONDARY),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format_seconds(total as u64))
                                .small()
                                .color(TEXT_SECONDARY),
                        );
                    });
                });

                ui.add_space(14.0);
                ui.columns(5, |cols| {
                    cols[0].vertical_centered(|ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("🔀").size(18.0).color(
                                    if self.queue.shuffle {
                                        ACCENT_RED
                                    } else {
                                        TEXT_SECONDARY
                                    },
                                ))
                                .min_size(Vec2::new(46.0, 44.0)),
                            )
                            .clicked()
                        {
                            self.queue.toggle_shuffle();
                        }
                    });
                    cols[1].vertical_centered(|ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("⏮").size(20.0))
                                    .min_size(Vec2::new(48.0, 44.0)),
                            )
                            .clicked()
                            && self.queue.prev_track().is_some()
                        {
                            self.play_current_queue_track(ctx);
                        }
                    });
                    cols[2].vertical_centered(|ui| {
                        let play_lbl = if snap.status == PlaybackStatus::Playing {
                            "⏸"
                        } else {
                            "▶"
                        };
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(play_lbl).size(24.0).color(TEXT_PRIMARY),
                                )
                                .fill(ACCENT_RED)
                                .corner_radius(CornerRadius::same(26))
                                .min_size(Vec2::new(64.0, 52.0)),
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
                        if ui
                            .add(
                                egui::Button::new(RichText::new("⏭").size(20.0))
                                    .min_size(Vec2::new(48.0, 44.0)),
                            )
                            .clicked()
                            && self.queue.next_track(true).is_some()
                        {
                            self.play_current_queue_track(ctx);
                        }
                    });
                    cols[4].vertical_centered(|ui| {
                        let rep_lbl = match self.queue.repeat {
                            yr_core::queue::RepeatMode::Off => "Off",
                            yr_core::queue::RepeatMode::All => "Todo",
                            yr_core::queue::RepeatMode::One => "1x",
                        };
                        if ui
                            .add(
                                egui::Button::new(RichText::new(rep_lbl).size(14.0))
                                    .min_size(Vec2::new(50.0, 44.0)),
                            )
                            .clicked()
                        {
                            self.queue.repeat = self.queue.repeat.cycle();
                        }
                    });
                });

                ui.add_space(14.0);
                ui.horizontal_wrapped(|ui| {
                    let liked = self.store.is_liked(&track.id);
                    if ui
                        .button(if liked { "♥ Me gusta" } else { "♡ Me gusta" })
                        .clicked()
                    {
                        self.store.toggle_like(track.clone());
                    }
                    if ui.button("➕ Playlist").clicked() {
                        self.playlist_picker_track = Some(track.clone());
                    }
                    if ui.button("⬇ Descargar").clicked() {
                        self.enqueue_download(track.clone(), ctx);
                    }
                    if ui.button("📻 Radio").clicked() {
                        self.start_radio(track.id.clone(), ctx);
                    }
                });

                ui.add_space(10.0);
                let mut vol = self.store.data.config.volume;
                ui.spacing_mut().slider_width = (ui.available_width() - 130.0).max(100.0);
                if ui
                    .add(egui::Slider::new(&mut vol, 0.0..=1.0).text("Vol"))
                    .changed()
                {
                    self.store.data.config.volume = vol;
                    self.audio.set_volume(vol);
                    let _ = self.store.save();
                }
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

        ui.add_space(12.0);
        let mb = self.total_downloaded_bytes as f64 / (1024.0 * 1024.0);
        ui.label(
            RichText::new(format!("Almacenamiento offline: {mb:.1} MB"))
                .color(TEXT_SECONDARY),
        );
        ui.add_space(6.0);
        if ui.button("Limpiar caché de streaming").clicked() {
            let cache_dir = self.data_dir.join("stream_cache");
            let _ = std::fs::remove_dir_all(&cache_dir);
            let _ = std::fs::create_dir_all(&cache_dir);
            self.set_banner("Caché de streaming eliminada");
        }

        ui.add_space(10.0);
        if ui.button("🔊 Probar audio local (2 pistas)").clicked() {
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

    fn render_playlist_picker_modal(&mut self, ctx: &egui::Context) {
        let Some(track) = self.playlist_picker_track.clone() else {
            return;
        };
        let mut open = true;
        egui::Window::new("Añadir a playlist")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(260.0);
                ui.label(RichText::new(&track.title).strong());
                ui.add_space(6.0);
                let pls = self.store.data.playlists.clone();
                if pls.is_empty() {
                    ui.label(
                        RichText::new("No tienes playlists aún.")
                            .small()
                            .color(TEXT_SECONDARY),
                    );
                } else {
                    for pl in pls {
                        if ui
                            .button(format!("{} ({})", pl.name, pl.tracks.len()))
                            .clicked()
                        {
                            self.store.add_to_playlist(&pl.id, track.clone());
                            self.set_banner(format!("Añadida a '{}'", pl.name));
                            self.playlist_picker_track = None;
                        }
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_playlist_name)
                            .hint_text("Nueva playlist...")
                            .desired_width(150.0),
                    );
                    if ui.button("Crear y añadir").clicked()
                        && !self.new_playlist_name.trim().is_empty()
                    {
                        let name = self.new_playlist_name.trim().to_string();
                        let created = self.store.create_playlist(name);
                        self.store.add_to_playlist(&created.id, track.clone());
                        self.new_playlist_name.clear();
                        self.set_banner(format!("Añadida a '{}'", created.name));
                        self.playlist_picker_track = None;
                    }
                });
            });
        if !open {
            self.playlist_picker_track = None;
        }
    }
}

fn render_thumb(ui: &mut egui::Ui, url: Option<&str>, size: f32) {
    if let Some(u) = url {
        ui.add(
            Image::new(u)
                .fit_to_exact_size(Vec2::splat(size))
                .corner_radius(CornerRadius::same(8)),
        );
    } else {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), BG_ELEVATED);
    }
}

fn generate_test_wav(path: &std::path::Path, freq_hz: f32, duration_secs: u32) -> std::io::Result<()> {
    use std::io::Write;
    let sample_rate: u32 = 22050;
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
