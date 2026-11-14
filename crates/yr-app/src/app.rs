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
    format_seconds, AlbumDetail, AudioQuality, LibraryStore, PlaybackQueue, PlaybackStatus,
    PlaylistDetail, SearchResults, Track,
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
    Playlists,
}

enum DetailView {
    None,
    Album(AlbumDetail),
    Playlist(PlaylistDetail),
}

enum AppEvent {
    TrendingLoaded(Result<Vec<Track>, String>),
    SearchLoaded(Result<SearchResults, String>),
    AlbumLoaded(Result<AlbumDetail, String>),
    PlaylistLoaded(Result<PlaylistDetail, String>),
    StreamResolved {
        track: Track,
        result: Result<String, String>,
    },
    DownloadsRefreshed(Vec<DownloadTask>),
}

pub struct YrApp {
    runtime: tokio::runtime::Runtime,
    provider: Arc<dyn MusicProvider>,
    downloader: Arc<DownloadManager>,
    audio: AudioEngine,
    store: LibraryStore,
    queue: PlaybackQueue,

    event_tx: mpsc::UnboundedSender<AppEvent>,
    event_rx: mpsc::UnboundedReceiver<AppEvent>,

    active_tab: NavTab,
    search_query: String,
    search_filter: SearchFilter,
    search_results: SearchResults,
    search_loading: bool,
    trending_tracks: Vec<Track>,
    trending_loading: bool,
    detail_view: DetailView,
    detail_loading: bool,

    downloads: Vec<DownloadTask>,
    last_download_poll: Instant,

    now_playing_open: bool,
    show_queue: bool,
    show_settings: bool,
    new_playlist_name: String,

    last_finished_counter: u64,
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
            event_tx,
            event_rx,
            active_tab: NavTab::Home,
            search_query: String::new(),
            search_filter: SearchFilter::Songs,
            search_results: SearchResults::default(),
            search_loading: false,
            trending_tracks: Vec::new(),
            trending_loading: false,
            detail_view: DetailView::None,
            detail_loading: false,
            downloads: Vec::new(),
            last_download_poll: Instant::now() - Duration::from_secs(10),
            now_playing_open: false,
            show_queue: false,
            show_settings: false,
            new_playlist_name: String::new(),
            last_finished_counter: 0,
            status_banner: None,
            seeking_secs: None,
        };

        app.refresh_trending(&cc.egui_ctx);
        app.refresh_downloads(&cc.egui_ctx);
        app
    }

    fn set_banner(&mut self, msg: impl Into<String>) {
        self.status_banner = Some((msg.into(), Instant::now()));
    }

    fn refresh_trending(&mut self, ctx: &egui::Context) {
        self.trending_loading = true;
        let provider = Arc::clone(&self.provider);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let res = provider
                .get_trending_music()
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::TrendingLoaded(res));
            ctx.request_repaint();
        });
    }

    fn trigger_search(&mut self, ctx: &egui::Context) {
        let q = self.search_query.trim().to_string();
        if q.is_empty() {
            return;
        }
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

    fn refresh_downloads(&mut self, ctx: &egui::Context) {
        self.last_download_poll = Instant::now();
        let dl = Arc::clone(&self.downloader);
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let items = dl.list_tasks().await;
            let _ = tx.send(AppEvent::DownloadsRefreshed(items));
            ctx.request_repaint();
        });
    }

    fn play_track_in_list(&mut self, tracks: Vec<Track>, index: usize, ctx: &egui::Context) {
        self.queue.set_queue(tracks, index);
        self.play_current_queue_track(ctx);
    }

    fn play_current_queue_track(&mut self, ctx: &egui::Context) {
        let Some(track) = self.queue.current_track().cloned() else {
            return;
        };
        self.store.record_play(track.clone());
        self.seeking_secs = None;

        // Check local offline file first for zero-latency playback.
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
            let items = dl.list_tasks().await;
            let _ = tx.send(AppEvent::DownloadsRefreshed(items));
            ctx.request_repaint();
        });
        self.set_banner(format!("Descargando: {title}"));
    }

    fn process_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.event_rx.try_recv() {
            match ev {
                AppEvent::TrendingLoaded(res) => {
                    self.trending_loading = false;
                    match res {
                        Ok(tracks) => self.trending_tracks = tracks,
                        Err(e) => self.set_banner(format!("Sin conexión o error: {e}")),
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
                AppEvent::StreamResolved { track, result } => match result {
                    Ok(url) => {
                        if self.queue.current_track().map(|t| t.id.as_str()) == Some(&track.id) {
                            self.audio.play_url(url, track.id, track.duration_secs);
                        }
                    }
                    Err(e) => self.set_banner(format!("No se pudo reproducir: {e}")),
                },
                AppEvent::DownloadsRefreshed(items) => {
                    self.downloads = items;
                }
            }
        }

        // Poll downloads periodically while on Downloads tab or active tasks exist.
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

        // Auto-advance when current track finishes.
        let snap = self.audio.snapshot();
        if snap.finished_counter > self.last_finished_counter {
            self.last_finished_counter = snap.finished_counter;
            if self.queue.next_track(false).is_some() {
                self.play_current_queue_track(ctx);
            }
        }
        if matches!(
            snap.status,
            PlaybackStatus::Playing | PlaybackStatus::Buffering | PlaybackStatus::Loading
        ) {
            ctx.request_repaint_after(Duration::from_millis(250));
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
                // Extra top spacing on Android status bar
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
                    self.render_settings(ui);
                    return;
                }

                if self.now_playing_open {
                    self.render_full_player(ui, ctx, &snap);
                    return;
                }

                match self.active_tab {
                    NavTab::Home => self.render_home(ui, ctx),
                    NavTab::Search => self.render_search(ui, ctx),
                    NavTab::Library => self.render_library(ui, ctx),
                    NavTab::Downloads => self.render_downloads(ui, ctx),
                }
            });
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
        }
    }

    fn render_home(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(RichText::new("Para ti").color(TEXT_PRIMARY));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("↻ Actualizar").clicked() {
                        self.refresh_trending(ctx);
                    }
                });
            });

            if self.trending_loading {
                ui.spinner();
            }

            let offline_tracks: Vec<Track> = self
                .downloads
                .iter()
                .filter(|d| d.status == DownloadStatus::Completed)
                .map(|d| d.track.clone())
                .collect();

            if !offline_tracks.is_empty() {
                ui.add_space(6.0);
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
                ui.add_space(10.0);
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

            if !self.trending_tracks.is_empty() {
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Tendencias en YT Music")
                        .size(16.0)
                        .strong()
                        .color(TEXT_PRIMARY),
                );
                let trending = self.trending_tracks.clone();
                for (idx, track) in trending.iter().take(20).enumerate() {
                    self.render_track_row(ui, ctx, track, &trending, idx, false);
                }
            } else if !self.trending_loading {
                ui.add_space(16.0);
                ui.label(
                    RichText::new("Busca canciones o explora tus descargas offline.")
                        .color(TEXT_SECONDARY),
                );
            }
        });
    }

    fn render_search(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.search_query)
                    .hint_text("Buscar canciones, álbumes, playlists...")
                    .desired_width(ui.available_width() - 88.0),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Buscar").clicked() || enter {
                self.trigger_search(ctx);
            }
        });

        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.search_filter, SearchFilter::Songs, "Canciones");
            ui.selectable_value(&mut self.search_filter, SearchFilter::Albums, "Álbumes");
            ui.selectable_value(
                &mut self.search_filter,
                SearchFilter::Playlists,
                "Playlists",
            );
        });

        if self.search_loading || self.detail_loading {
            ui.spinner();
        }

        // Album / Playlist detail drill-down
        let mut close_detail = false;
        match &self.detail_view {
            DetailView::Album(album) => {
                let album = album.clone();
                ui.horizontal(|ui| {
                    if ui.button("← Volver").clicked() {
                        close_detail = true;
                    }
                    ui.heading(&album.summary.title);
                });
                ui.label(RichText::new(&album.summary.artist).color(TEXT_SECONDARY));
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (idx, track) in album.tracks.iter().enumerate() {
                        self.render_track_row(ui, ctx, track, &album.tracks, idx, false);
                    }
                });
                if close_detail {
                    self.detail_view = DetailView::None;
                }
                return;
            }
            DetailView::Playlist(pl) => {
                let pl = pl.clone();
                ui.horizontal(|ui| {
                    if ui.button("← Volver").clicked() {
                        close_detail = true;
                    }
                    ui.heading(&pl.summary.title);
                });
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (idx, track) in pl.tracks.iter().enumerate() {
                        self.render_track_row(ui, ctx, track, &pl.tracks, idx, false);
                    }
                });
                if close_detail {
                    self.detail_view = DetailView::None;
                }
                return;
            }
            DetailView::None => {}
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
                for album in albums {
                    egui::Frame::new()
                        .fill(BG_SURFACE)
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                render_thumb(ui, album.thumbnail_url.as_deref(), 48.0);
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(&album.title).strong());
                                    ui.label(
                                        RichText::new(&album.artist).small().color(TEXT_SECONDARY),
                                    );
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.button("Abrir").clicked() {
                                        self.open_album(album.id.clone(), ctx);
                                    }
                                });
                            });
                        });
                }
            }
            SearchFilter::Playlists => {
                let playlists = self.search_results.playlists.clone();
                for pl in playlists {
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
                                        ui.label(
                                            RichText::new(author).small().color(TEXT_SECONDARY),
                                        );
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
            }
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
                    egui::CollapsingHeader::new(format!("{} ({})", pl.name, pl.tracks.len()))
                        .show(ui, |ui| {
                            for (idx, track) in pl.tracks.iter().enumerate() {
                                self.render_track_row(ui, ctx, track, &pl.tracks, idx, false);
                            }
                        });
                }
            }
        });
    }

    fn render_downloads(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.heading("Descargas");
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
                                            egui::ProgressBar::new(task.progress())
                                                .text(format!(
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
                Stroke::new(1.0, ACCENT_RED)
            } else {
                Stroke::NONE
            })
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    render_thumb(ui, track.thumbnail_url.as_deref(), 46.0);

                    let info_resp = ui
                        .vertical(|ui| {
                            ui.label(
                                RichText::new(&track.title)
                                    .strong()
                                    .color(if is_current { ACCENT_RED } else { TEXT_PRIMARY }),
                            );
                            let sub = if offline_badge {
                                format!("{} • Offline", track.artist)
                            } else {
                                format!("{} • {}", track.artist, track.formatted_duration())
                            };
                            ui.label(RichText::new(sub).small().color(TEXT_SECONDARY));
                        })
                        .response
                        .interact(Sense::click());

                    if info_resp.clicked() {
                        self.play_track_in_list(list.to_vec(), index, ctx);
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let is_downloaded = self.downloads.iter().any(|d| {
                            d.track.id == track.id && d.status == DownloadStatus::Completed
                        });
                        if is_downloaded {
                            ui.label(RichText::new("✓").color(GREEN_OK));
                        } else if ui.button("⬇").clicked() {
                            self.enqueue_download(track.clone(), ctx);
                        }

                        let liked = self.store.is_liked(&track.id);
                        let heart = if liked { "♥" } else { "♡" };
                        if ui
                            .button(
                                RichText::new(heart)
                                    .color(if liked { ACCENT_RED } else { TEXT_SECONDARY }),
                            )
                            .clicked()
                        {
                            self.store.toggle_like(track.clone());
                        }

                        if ui.button("▶").clicked() {
                            self.play_track_in_list(list.to_vec(), index, ctx);
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
        egui::Frame::new()
            .fill(BG_ELEVATED)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    render_thumb(ui, track.thumbnail_url.as_deref(), 40.0);
                    let resp = ui
                        .vertical(|ui| {
                            ui.label(RichText::new(&track.title).strong());
                            ui.label(RichText::new(&track.artist).small().color(TEXT_SECONDARY));
                        })
                        .response
                        .interact(Sense::click());
                    if resp.clicked() {
                        self.now_playing_open = true;
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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

        ui.horizontal(|ui| {
            if ui.button("⌄ Minimizar").clicked() {
                self.now_playing_open = false;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Cola").clicked() {
                    self.show_queue = !self.show_queue;
                }
            });
        });

        if self.show_queue {
            ui.heading("Cola de reproducción");
            let ordered: Vec<(usize, Track)> = self
                .queue
                .ordered_tracks()
                .into_iter()
                .map(|(i, t)| (i, t.clone()))
                .collect();
            let current_idx = self.queue.current_index();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (idx, t) in ordered {
                    let is_curr = current_idx == Some(idx);
                    if ui
                        .selectable_label(is_curr, format!("{} — {}", t.title, t.artist))
                        .clicked()
                    {
                        self.queue.jump_to(idx);
                        self.play_current_queue_track(ctx);
                    }
                }
            });
            return;
        }

        ui.vertical_centered(|ui| {
            ui.add_space(12.0);
            render_thumb(ui, track.thumbnail_url.as_deref(), 220.0);
            ui.add_space(16.0);
            ui.label(RichText::new(&track.title).size(22.0).strong());
            ui.label(RichText::new(&track.artist).size(15.0).color(TEXT_SECONDARY));

            match &snap.status {
                PlaybackStatus::Buffering | PlaybackStatus::Loading => {
                    ui.label(RichText::new("Cargando audio...").small().color(ACCENT_RED));
                }
                PlaybackStatus::Error(err) => {
                    ui.label(RichText::new(err).small().color(ACCENT_RED));
                }
                _ => {}
            }

            ui.add_space(14.0);

            let total = snap
                .duration_secs
                .or(track.duration_secs.map(|d| d as f64))
                .unwrap_or(1.0)
                .max(1.0);
            let mut pos = self.seeking_secs.unwrap_or(snap.position_secs).min(total);
            let slider = egui::Slider::new(&mut pos, 0.0..=total).show_value(false);
            let resp = ui.add(slider);
            if resp.dragged() {
                self.seeking_secs = Some(pos);
            }
            if resp.drag_stopped() {
                self.audio.seek(Duration::from_secs_f64(pos));
                self.seeking_secs = None;
            }

            ui.horizontal(|ui| {
                ui.label(format_seconds(pos as u64));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(format_seconds(total as u64));
                });
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("🔀").color(if self.queue.shuffle {
                        ACCENT_RED
                    } else {
                        TEXT_SECONDARY
                    }))
                    .clicked()
                {
                    self.queue.toggle_shuffle();
                }

                if ui.button("⏮").clicked() && self.queue.prev_track().is_some() {
                    self.play_current_queue_track(ctx);
                }

                let play_lbl = if snap.status == PlaybackStatus::Playing {
                    "⏸ Pausar"
                } else {
                    "▶ Reproducir"
                };
                if ui
                    .add(egui::Button::new(RichText::new(play_lbl).size(18.0)).fill(ACCENT_RED))
                    .clicked()
                {
                    match snap.status {
                        PlaybackStatus::Playing => self.audio.pause(),
                        PlaybackStatus::Paused => self.audio.resume(),
                        _ => self.play_current_queue_track(ctx),
                    }
                }

                if ui.button("⏭").clicked() && self.queue.next_track(true).is_some() {
                    self.play_current_queue_track(ctx);
                }

                if ui.button(self.queue.repeat.label()).clicked() {
                    self.queue.repeat = self.queue.repeat.cycle();
                }
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let liked = self.store.is_liked(&track.id);
                if ui
                    .button(if liked { "♥ Me gusta" } else { "♡ Me gusta" })
                    .clicked()
                {
                    self.store.toggle_like(track.clone());
                }
                if ui.button("⬇ Descargar").clicked() {
                    self.enqueue_download(track.clone(), ctx);
                }
            });

            ui.add_space(10.0);
            let mut vol = self.store.data.config.volume;
            if ui
                .add(egui::Slider::new(&mut vol, 0.0..=1.0).text("Volumen"))
                .changed()
            {
                self.store.data.config.volume = vol;
                self.audio.set_volume(vol);
                let _ = self.store.save();
            }
        });
    }

    fn render_settings(&mut self, ui: &mut egui::Ui) {
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
        ui.horizontal(|ui| {
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
        ui.horizontal(|ui| {
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
