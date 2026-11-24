pub mod innertube;

use async_trait::async_trait;
use innertube::GoogleSession;
use rustypipe::client::{OauthDeviceCode, RustyPipe};
use rustypipe::model::{AlbumItem, ArtistItem, MusicPlaylistItem, TrackItem};
use rustypipe::param::Country;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::Mutex;
use yr_core::{
    AlbumDetail, AlbumSummary, ArtistDetail, ArtistSummary, AudioQuality, ExploreFeed,
    GoogleAccountInfo, GoogleDeviceAuth, HomeFeed, HomeShelf, HomeShelfCard, HomeShelfLayout, LyricsData, MoodGenre,
    PlaylistDetail, PlaylistSummary, SearchResults, ShelfCardKind, SponsorSegment, StreamInfo,
    Track, VideoStreamInfo,
};

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("Error de red o API: {0}")]
    Api(String),
    #[error("No se encontró ningún stream de audio compatible para {0}")]
    NoAudioStream(String),
}

#[async_trait]
pub trait MusicProvider: Send + Sync {
    async fn get_home_feed(&self) -> Result<HomeFeed, ProviderError>;
    async fn get_home_feed_with_params(
        &self,
        mood_params: Option<&str>,
    ) -> Result<HomeFeed, ProviderError>;
    async fn search(&self, query: &str) -> Result<SearchResults, ProviderError>;
    async fn search_suggestions(&self, query: &str) -> Result<Vec<String>, ProviderError>;
    async fn get_album(&self, album_id: &str) -> Result<AlbumDetail, ProviderError>;
    async fn get_playlist(&self, playlist_id: &str) -> Result<PlaylistDetail, ProviderError>;
    async fn get_artist(&self, artist_id: &str) -> Result<ArtistDetail, ProviderError>;
    async fn get_mood_playlists(&self, genre_id: &str) -> Result<Vec<PlaylistSummary>, ProviderError>;
    async fn get_explore_feed(&self) -> Result<ExploreFeed, ProviderError>;
    async fn get_radio(&self, track_id: &str) -> Result<Vec<Track>, ProviderError>;
    async fn get_related_tracks(&self, track_id: &str) -> Result<Vec<Track>, ProviderError>;
    async fn get_lyrics(&self, track_id: &str) -> Result<Option<String>, ProviderError>;
    async fn get_synced_lyrics(&self, track: &Track) -> Result<Option<LyricsData>, ProviderError>;
    async fn get_sponsor_segments(&self, track_id: &str) -> Result<Vec<SponsorSegment>, ProviderError>;
    async fn resolve_audio_stream(
        &self,
        track_id: &str,
        quality: AudioQuality,
    ) -> Result<StreamInfo, ProviderError>;
    async fn resolve_video_stream(
        &self,
        _track_id: &str,
        _max_height: u32,
    ) -> Result<VideoStreamInfo, ProviderError> {
        Err(ProviderError::Api("Video stream not supported".into()))
    }
    async fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError>;
    async fn is_logged_in(&self) -> bool;
    async fn start_google_device_login(&self) -> Result<GoogleDeviceAuth, ProviderError>;
    async fn poll_google_device_login(&self) -> Result<bool, ProviderError>;
    async fn login_with_cookie(&self, cookie: &str) -> Result<(), ProviderError>;
    async fn logout(&self) -> Result<(), ProviderError>;
    async fn get_account_info(&self) -> Option<GoogleAccountInfo> {
        None
    }
}

/// YouTube Music provider backed by `rustypipe` + direct InnerTube WEB_REMIX.
#[derive(Clone)]
pub struct YtMusicProvider {
    client: RustyPipe,
    http: reqwest::Client,
    cache_dir: Option<PathBuf>,
    google_session: Arc<Mutex<Option<GoogleSession>>>,
    pending_oauth: Arc<Mutex<Option<OauthDeviceCode>>>,
    stream_cache: Arc<std::sync::Mutex<HashMap<(String, u32), (StreamInfo, Instant)>>>,
    video_stream_cache: Arc<std::sync::Mutex<HashMap<(String, u32), (VideoStreamInfo, Instant)>>>,
}

impl Default for YtMusicProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl YtMusicProvider {
    fn build_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(12))
            .pool_idle_timeout(Duration::from_secs(30))
            .tcp_nodelay(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    pub fn new() -> Self {
        Self {
            client: RustyPipe::new(),
            http: Self::build_http_client(),
            cache_dir: None,
            google_session: Arc::new(Mutex::new(None)),
            pending_oauth: Arc::new(Mutex::new(None)),
            stream_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
            video_stream_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    pub fn with_cache_dir(cache_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&cache_dir);
        let session = GoogleSession::load(&GoogleSession::session_file(&cache_dir));
        let client = RustyPipe::builder()
            .storage_dir(cache_dir.clone())
            .build()
            .unwrap_or_else(|_| RustyPipe::new());
        Self {
            client,
            http: Self::build_http_client(),
            cache_dir: Some(cache_dir),
            google_session: Arc::new(Mutex::new(session)),
            pending_oauth: Arc::new(Mutex::new(None)),
            stream_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
            video_stream_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    fn map_track(item: TrackItem) -> Track {
        let artist = item
            .artists
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Artista desconocido".to_string());
        let artist_id = item.artists.first().and_then(|a| a.id.clone());
        let thumbnail_url = item
            .cover
            .last()
            .or_else(|| item.cover.first())
            .map(|c| c.url.clone());

        Track {
            id: item.id,
            title: item.name,
            artist,
            artist_id,
            album: item.album.as_ref().map(|a| a.name.clone()),
            album_id: item.album.map(|a| a.id),
            duration_secs: item.duration.map(|d| d as u64),
            thumbnail_url,
            local_path: None,
        }
    }

    fn map_album(item: AlbumItem) -> AlbumSummary {
        let artist = item
            .artists
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Artista desconocido".to_string());
        AlbumSummary {
            id: item.id,
            title: item.name,
            artist,
            year: item.year,
            thumbnail_url: item
                .cover
                .last()
                .or_else(|| item.cover.first())
                .map(|c| c.url.clone()),
        }
    }

    fn format_subscriber_count(c: u64) -> String {
        if c >= 1_000_000 {
            let m = c as f64 / 1_000_000.0;
            let raw = format!("{m:.2}");
            let trimmed = raw.trim_end_matches('0').trim_end_matches('.');
            format!("{trimmed} M de suscriptores")
        } else if c >= 1_000 {
            let k = c as f64 / 1_000.0;
            if k >= 100.0 || (k - k.round()).abs() < 0.05 {
                format!("{:.0} K suscriptores", k.round())
            } else {
                format!("{k:.1} K suscriptores")
            }
        } else {
            format!("{c} suscriptores")
        }
    }

    fn map_artist(item: ArtistItem) -> ArtistSummary {
        ArtistSummary {
            id: item.id,
            name: item.name,
            subscribers: item
                .subscriber_count
                .map(|c| Self::format_subscriber_count(c as u64)),
            thumbnail_url: item
                .avatar
                .last()
                .or_else(|| item.avatar.first())
                .map(|t| t.url.clone()),
        }
    }

    fn map_playlist(item: MusicPlaylistItem) -> PlaylistSummary {
        PlaylistSummary {
            id: item.id,
            title: item.name,
            author: item.channel.map(|c| c.name),
            track_count: item.track_count.map(|v| v as u32),
            thumbnail_url: item
                .thumbnail
                .last()
                .or_else(|| item.thumbnail.first())
                .map(|t| t.url.clone()),
        }
    }
}

#[async_trait]
impl MusicProvider for YtMusicProvider {
    async fn get_home_feed(&self) -> Result<HomeFeed, ProviderError> {
        self.get_home_feed_with_params(None).await
    }

    async fn get_home_feed_with_params(
        &self,
        mood_params: Option<&str>,
    ) -> Result<HomeFeed, ProviderError> {
        let session = self.google_session.lock().await.clone();
        let has_cookie_session = session.as_ref().is_some_and(|s| s.has_auth_cookies());
        let logged_in = has_cookie_session;

        let mut feed = match innertube::fetch_home_feed(&self.http, session.as_ref(), mood_params).await {
            Ok(f) => f,
            Err(_) => {
                // Fallback to rustypipe charts + new releases
                let q = self.client.query();
                let (charts_res, new_videos_res, new_albums_res, genres_res) =
                    tokio::time::timeout(Duration::from_secs(8), async {
                        tokio::join!(
                            q.music_charts(Some(Country::Us)),
                            q.music_new_videos(),
                            q.music_new_albums(),
                            q.music_genres()
                        )
                    })
                    .await
                    .map_err(|_| {
                        ProviderError::Api("Tiempo de espera agotado al cargar el feed".into())
                    })?;

                let mut quick_picks: Vec<Track> = Vec::new();
                let mut trending_tracks: Vec<Track> = Vec::new();
                let mut recommended_playlists: Vec<PlaylistSummary> = Vec::new();
                if let Ok(charts) = charts_res {
                    quick_picks.extend(charts.top_tracks.into_iter().map(Self::map_track));
                    trending_tracks.extend(charts.trending_tracks.into_iter().map(Self::map_track));
                    recommended_playlists.extend(charts.playlists.into_iter().map(Self::map_playlist));
                }
                let recommended_tracks: Vec<Track> = new_videos_res
                    .map(|v| v.into_iter().map(Self::map_track).collect())
                    .unwrap_or_default();
                let new_albums: Vec<AlbumSummary> = new_albums_res
                    .map(|a| a.into_iter().map(Self::map_album).collect())
                    .unwrap_or_default();
                let moods = genres_res
                    .map(|g| {
                        g.into_iter()
                            .take(14)
                            .map(|g| MoodGenre {
                                id: g.id,
                                title: g.name,
                                params: None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if quick_picks.is_empty() && recommended_tracks.is_empty() && new_albums.is_empty() {
                    return Err(ProviderError::Api(
                        "No se pudo cargar el feed de YT Music".into(),
                    ));
                }
                HomeFeed {
                    quick_picks,
                    recommended_tracks,
                    trending_tracks,
                    new_albums,
                    recommended_playlists,
                    moods,
                    is_logged_in: logged_in,
                    account: session.as_ref().and_then(|s| s.account.clone()),
                    ..Default::default()
                }
            }
        };

        if feed.quick_picks.is_empty() && !feed.recommended_tracks.is_empty() {
            feed.quick_picks = feed.recommended_tracks.clone();
        }
        if feed.quick_picks.is_empty() && !feed.trending_tracks.is_empty() {
            feed.quick_picks = feed.trending_tracks.clone();
        }
        if feed.quick_picks.is_empty() && mood_params.is_none() {
            let q = self.client.query();
            if let Ok((charts_res, new_videos_res)) =
                tokio::time::timeout(Duration::from_secs(8), async {
                    tokio::join!(q.music_charts(Some(Country::Us)), q.music_new_videos())
                })
                .await
            {
                if let Ok(charts) = charts_res {
                    feed.quick_picks
                        .extend(charts.top_tracks.into_iter().map(Self::map_track));
                    if feed.trending_tracks.is_empty() {
                        feed.trending_tracks
                            .extend(charts.trending_tracks.into_iter().map(Self::map_track));
                    }
                }
                if feed.quick_picks.is_empty() && !feed.trending_tracks.is_empty() {
                    feed.quick_picks = feed.trending_tracks.clone();
                }
                if feed.quick_picks.is_empty() {
                    if let Ok(videos) = new_videos_res {
                        feed.quick_picks
                            .extend(videos.into_iter().map(Self::map_track));
                    }
                }
            }
        }

        feed.is_logged_in = logged_in;
        if feed.account.is_none() {
            feed.account = session.as_ref().and_then(|s| s.account.clone());
        }

        // Hydrate authenticated personal playlists & history when logged in
        if logged_in && mood_params.is_none() {
            let qa = self.client.query().authenticated();
            let (hist_res, pl_res, liked_res) = tokio::join!(
                qa.music_history(),
                qa.music_saved_playlists(),
                qa.music_liked_tracks(),
            );
            if let Ok(hist) = hist_res {
                feed.listen_again
                    .extend(hist.items.into_iter().map(|h| Self::map_track(h.item)));
            }
            if let Ok(liked) = liked_res {
                let liked_count = liked.tracks.items.len() as u32;
                let thumb = liked
                    .thumbnail
                    .last()
                    .or_else(|| liked.thumbnail.first())
                    .map(|t| t.url.clone());
                feed.user_playlists.push(PlaylistSummary {
                    id: liked.id,
                    title: "Música que te gusta".to_string(),
                    author: Some("YouTube Music".to_string()),
                    track_count: Some(liked_count),
                    thumbnail_url: thumb,
                });
            }
            if let Ok(pls) = pl_res {
                feed.user_playlists
                    .extend(pls.items.into_iter().map(Self::map_playlist));
            }
            if !feed.user_playlists.is_empty() {
                let cards: Vec<HomeShelfCard> = feed
                    .user_playlists
                    .iter()
                    .map(|p| HomeShelfCard {
                        title: p.title.clone(),
                        subtitle: p
                            .author
                            .clone()
                            .unwrap_or_else(|| "Lista".to_string()),
                        thumbnail_url: p.thumbnail_url.clone(),
                        kind: ShelfCardKind::Playlist(p.clone()),
                    })
                    .collect();
                feed.shelves.push(HomeShelf {
                    title: "Tus listas".to_string(),
                    eyebrow: feed.account.as_ref().map(|a| a.name.to_uppercase()),
                    layout: HomeShelfLayout::CarouselCards,
                    tracks: Vec::new(),
                    cards,
                });
            }
        }

        Ok(feed)
    }

    async fn search(&self, query: &str) -> Result<SearchResults, ProviderError> {
        let q = self.client.query();
        let (songs_res, albums_res, artists_res, playlists_res) = tokio::join!(
            q.music_search_tracks(query),
            q.music_search_albums(query),
            q.music_search_artists(query),
            q.music_search_playlists(query, false),
        );

        let songs = songs_res
            .map(|r| r.items.items.into_iter().map(Self::map_track).collect())
            .unwrap_or_default();
        let albums = albums_res
            .map(|r| r.items.items.into_iter().map(Self::map_album).collect())
            .unwrap_or_default();
        let artists = artists_res
            .map(|r| r.items.items.into_iter().map(Self::map_artist).collect())
            .unwrap_or_default();
        let playlists = playlists_res
            .map(|r| r.items.items.into_iter().map(Self::map_playlist).collect())
            .unwrap_or_default();

        Ok(SearchResults {
            songs,
            albums,
            artists,
            playlists,
        })
    }

    async fn search_suggestions(&self, query: &str) -> Result<Vec<String>, ProviderError> {
        let res = self
            .client
            .query()
            .music_search_suggestion(query)
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;
        Ok(res.terms)
    }

    async fn get_album(&self, album_id: &str) -> Result<AlbumDetail, ProviderError> {
        let album = self
            .client
            .query()
            .music_album(album_id)
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;

        let artist = album
            .artists
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Artista desconocido".to_string());
        let summary = AlbumSummary {
            id: album.id,
            title: album.name,
            artist,
            year: album.year,
            thumbnail_url: album
                .cover
                .last()
                .or_else(|| album.cover.first())
                .map(|c| c.url.clone()),
        };
        let tracks = album.tracks.into_iter().map(Self::map_track).collect();
        Ok(AlbumDetail { summary, tracks })
    }

    async fn get_playlist(&self, playlist_id: &str) -> Result<PlaylistDetail, ProviderError> {
        let pl = self
            .client
            .query()
            .music_playlist(playlist_id)
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;

        let summary = PlaylistSummary {
            id: pl.id,
            title: pl.name,
            author: pl.channel.map(|c| c.name),
            track_count: Some(pl.tracks.items.len() as u32),
            thumbnail_url: pl
                .thumbnail
                .last()
                .or_else(|| pl.thumbnail.first())
                .map(|t| t.url.clone()),
        };
        let tracks = pl.tracks.items.into_iter().map(Self::map_track).collect();
        Ok(PlaylistDetail { summary, tracks })
    }

    async fn get_artist(&self, artist_id: &str) -> Result<ArtistDetail, ProviderError> {
        let artist = self
            .client
            .query()
            .music_artist(artist_id, false)
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;

        let summary = ArtistSummary {
            id: artist.id,
            name: artist.name,
            subscribers: artist
                .subscriber_count
                .map(|c| Self::format_subscriber_count(c as u64)),
            thumbnail_url: artist
                .header_image
                .last()
                .or_else(|| artist.header_image.first())
                .map(|t| t.url.clone()),
        };
        let top_tracks = artist.tracks.into_iter().map(Self::map_track).collect();
        let albums = artist.albums.into_iter().map(Self::map_album).collect();
        Ok(ArtistDetail {
            summary,
            top_tracks,
            albums,
        })
    }

    async fn get_mood_playlists(
        &self,
        genre_id: &str,
    ) -> Result<Vec<PlaylistSummary>, ProviderError> {
        let genre = self
            .client
            .query()
            .music_genre(genre_id)
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;

        let playlists = genre
            .sections
            .into_iter()
            .flat_map(|s| s.playlists)
            .map(Self::map_playlist)
            .collect();
        Ok(playlists)
    }

    async fn get_explore_feed(&self) -> Result<ExploreFeed, ProviderError> {
        let session = self.google_session.lock().await.clone();
        let mut feed = innertube::fetch_explore_feed(&self.http, session.as_ref())
            .await
            .unwrap_or_default();
        if !feed.new_albums.is_empty()
            && !feed.trending_tracks.is_empty()
            && !feed.moods.is_empty()
        {
            return Ok(feed);
        }

        let q = self.client.query();
        let (charts_res, albums_res, genres_res) = tokio::join!(
            async {
                if feed.trending_tracks.is_empty() {
                    Some(q.music_charts(Some(Country::Us)).await)
                } else {
                    None
                }
            },
            async {
                if feed.new_albums.is_empty() {
                    Some(q.music_new_albums().await)
                } else {
                    None
                }
            },
            async {
                if feed.moods.is_empty() {
                    Some(q.music_genres().await)
                } else {
                    None
                }
            }
        );

        if let Some(Ok(charts)) = charts_res {
            feed.trending_tracks = charts.top_tracks.into_iter().map(Self::map_track).collect();
        }
        if let Some(Ok(albums)) = albums_res {
            feed.new_albums = albums.into_iter().map(Self::map_album).collect();
        }
        if let Some(Ok(genres)) = genres_res {
            feed.moods = genres
                .into_iter()
                .map(|g| MoodGenre {
                    id: g.id,
                    title: g.name,
                    params: None,
                })
                .collect();
        }
        Ok(feed)
    }

    async fn get_radio(&self, track_id: &str) -> Result<Vec<Track>, ProviderError> {
        let radio = self
            .client
            .query()
            .music_radio_track(track_id)
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;
        Ok(radio.items.into_iter().map(Self::map_track).collect())
    }

    async fn get_related_tracks(&self, track_id: &str) -> Result<Vec<Track>, ProviderError> {
        let session = self.google_session.lock().await.clone();
        match innertube::fetch_related_tracks(&self.http, session.as_ref(), track_id).await {
            Ok(tracks) if !tracks.is_empty() => Ok(tracks),
            _ => self.get_radio(track_id).await,
        }
    }

    async fn get_lyrics(&self, track_id: &str) -> Result<Option<String>, ProviderError> {
        let details = self
            .client
            .query()
            .music_details(track_id)
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;

        let Some(lyrics_id) = details.lyrics_id else {
            return Ok(None);
        };
        let lyrics = self
            .client
            .query()
            .music_lyrics(lyrics_id)
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;

        Ok(Some(format!("{}\n\n{}", lyrics.body, lyrics.footer)))
    }

    async fn get_synced_lyrics(&self, track: &Track) -> Result<Option<LyricsData>, ProviderError> {
        if let Some(lrc) = innertube::fetch_lrclib_lyrics(&self.http, track).await {
            return Ok(Some(lrc));
        }
        match self.get_lyrics(&track.id).await? {
            Some(plain) => Ok(Some(LyricsData {
                plain,
                synced: Vec::new(),
                source: "YouTube Music".to_string(),
            })),
            None => Ok(None),
        }
    }

    async fn get_sponsor_segments(
        &self,
        track_id: &str,
    ) -> Result<Vec<SponsorSegment>, ProviderError> {
        Ok(innertube::fetch_sponsor_segments(&self.http, track_id).await)
    }

    async fn resolve_audio_stream(
        &self,
        track_id: &str,
        quality: AudioQuality,
    ) -> Result<StreamInfo, ProviderError> {
        let cache_key = (track_id.to_string(), quality.target_bitrate_kbps());
        if let Ok(guard) = self.stream_cache.lock() {
            if let Some((cached, fetched_at)) = guard.get(&cache_key) {
                if fetched_at.elapsed() < Duration::from_secs(4 * 3600) {
                    return Ok(cached.clone());
                }
            }
        }

        if let Ok(info) = innertube::fetch_player_stream(&self.http, track_id, quality).await {
            if let Ok(mut guard) = self.stream_cache.lock() {
                if guard.len() > 200 {
                    guard.clear();
                }
                guard.insert(cache_key, (info.clone(), Instant::now()));
            }
            return Ok(info);
        }

        let player = self
            .client
            .query()
            .player(track_id)
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;

        let target_bps = quality.target_bitrate_kbps() * 1000;
        let mut audio_streams = player.audio_streams;
        if audio_streams.is_empty() {
            return Err(ProviderError::NoAudioStream(track_id.to_string()));
        }

        audio_streams.sort_by_key(|s| {
            let is_m4a = s.mime.contains("mp4") || s.mime.contains("m4a");
            let m4a_penalty: u64 = if is_m4a { 0 } else { 10_000_000 };
            let bitrate_diff = (s.bitrate as i64 - target_bps as i64).unsigned_abs();
            m4a_penalty + bitrate_diff
        });

        let chosen = audio_streams.remove(0);
        let extension = if chosen.mime.contains("mp4") || chosen.mime.contains("m4a") {
            "m4a"
        } else if chosen.mime.contains("webm") {
            "webm"
        } else {
            "audio"
        };

        let info = StreamInfo {
            track_id: track_id.to_string(),
            url: chosen.url,
            mime_type: chosen.mime,
            extension: extension.to_string(),
            bitrate_bps: chosen.bitrate,
            content_length: if chosen.size > 0 {
                Some(chosen.size)
            } else {
                None
            },
        };
        if let Ok(mut guard) = self.stream_cache.lock() {
            guard.insert(cache_key, (info.clone(), Instant::now()));
        }
        Ok(info)
    }

    async fn resolve_video_stream(
        &self,
        track_id: &str,
        max_height: u32,
    ) -> Result<VideoStreamInfo, ProviderError> {
        let cache_key = (track_id.to_string(), max_height);
        if let Ok(guard) = self.video_stream_cache.lock() {
            if let Some((cached, fetched_at)) = guard.get(&cache_key) {
                if fetched_at.elapsed() < Duration::from_secs(4 * 3600) {
                    return Ok(cached.clone());
                }
            }
        }
        let info = match innertube::fetch_player_video_stream(&self.http, track_id, max_height).await {
            Ok(v) => v,
            Err(inner_err) => {
                let player = self
                    .client
                    .query()
                    .player(track_id)
                    .await
                    .map_err(|_| ProviderError::Api(inner_err.clone()))?;
                let adaptive_avc = player
                    .video_only_streams
                    .into_iter()
                    .filter(|s| s.mime.contains("video/mp4") && s.mime.contains("avc1"))
                    .map(|s| (s.height, s.width.max(1), s.fps as u32, s.url))
                    .collect();
                let muxed_mp4 = player
                    .video_streams
                    .into_iter()
                    .filter(|s| s.mime.contains("video/mp4"))
                    .map(|s| (s.height, s.width.max(1), s.fps as u32, s.url))
                    .collect();
                select_video_candidate(
                    track_id,
                    adaptive_avc,
                    muxed_mp4,
                    player.hls_manifest_url,
                    max_height,
                )
                .ok_or(ProviderError::Api(inner_err))?
            }
        };
        if let Ok(mut guard) = self.video_stream_cache.lock() {
            if guard.len() > 200 {
                guard.clear();
            }
            guard.insert(cache_key, (info.clone(), Instant::now()));
        }
        Ok(info)
    }

    async fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self
            .http
            .get(url)
            .timeout(Duration::from_secs(6))
            .send()
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ProviderError::Api(format!("HTTP {}", resp.status())));
        }
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| ProviderError::Api(e.to_string()))
    }

    async fn is_logged_in(&self) -> bool {
        if self
            .google_session
            .lock()
            .await
            .as_ref()
            .is_some_and(|s| s.has_auth_cookies())
        {
            return true;
        }
        self.client.user_auth_check_cookie().await.is_ok()
            || self.client.user_auth_check_login().await.is_ok()
    }

    async fn start_google_device_login(&self) -> Result<GoogleDeviceAuth, ProviderError> {
        let code = self
            .client
            .user_auth_get_code()
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;
        let out = GoogleDeviceAuth {
            user_code: code.user_code.clone(),
            verification_url: code.verification_url.clone(),
            interval_secs: code.interval.max(3) as u64,
        };
        *self.pending_oauth.lock().await = Some(code);
        Ok(out)
    }

    async fn poll_google_device_login(&self) -> Result<bool, ProviderError> {
        let code = self.pending_oauth.lock().await.take();
        let Some(code) = code else {
            return Ok(false);
        };
        let res = self.client.user_auth_login(&code).await;
        match res {
            Ok(true) => Ok(true),
            Ok(false) => {
                *self.pending_oauth.lock().await = Some(code);
                Ok(false)
            }
            Err(e) => {
                *self.pending_oauth.lock().await = Some(code);
                Err(ProviderError::Api(e.to_string()))
            }
        }
    }

    async fn login_with_cookie(&self, cookie: &str) -> Result<(), ProviderError> {
        let session = innertube::validate_and_hydrate_session(&self.http, cookie)
            .await
            .map_err(ProviderError::Api)?;
        if let Some(dir) = &self.cache_dir {
            session.save(&GoogleSession::session_file(dir));
        }
        *self.google_session.lock().await = Some(session);
        // Best-effort sync to rustypipe so authenticated rustypipe queries work too
        let _ = self.client.user_auth_set_cookie(cookie.trim()).await;
        Ok(())
    }

    async fn logout(&self) -> Result<(), ProviderError> {
        *self.pending_oauth.lock().await = None;
        *self.google_session.lock().await = None;
        if let Some(dir) = &self.cache_dir {
            let _ = std::fs::remove_file(GoogleSession::session_file(dir));
        }
        let _ = self.client.user_auth_remove_cookie().await;
        let _ = self.client.user_auth_logout().await;
        Ok(())
    }

    async fn get_account_info(&self) -> Option<GoogleAccountInfo> {
        self.google_session.lock().await.as_ref().and_then(|s| s.account.clone())
    }
}

fn select_video_candidate(
    track_id: &str,
    adaptive_avc: Vec<(u32, u32, u32, String)>,
    muxed_mp4: Vec<(u32, u32, u32, String)>,
    hls_url: Option<String>,
    max_height: u32,
) -> Option<VideoStreamInfo> {
    let cap = if max_height == 0 { 720 } else { max_height };
    let candidates = if !adaptive_avc.is_empty() {
        adaptive_avc
    } else {
        muxed_mp4
    };
    if let Some((h, w, _, url)) = candidates
        .iter()
        .filter(|(h, _, _, _)| *h <= cap)
        .max_by_key(|(h, _, fps, _)| (*h, *fps))
        .cloned()
        .or_else(|| {
            candidates
                .into_iter()
                .min_by_key(|(h, _, fps, _)| (*h, std::cmp::Reverse(*fps)))
        })
    {
        let ua = innertube::video_stream_user_agent(&url).to_string();
        return Some(VideoStreamInfo {
            track_id: track_id.to_string(),
            url,
            fallback_url: hls_url,
            user_agent: ua,
            width: w,
            height: h,
            is_hls: false,
        });
    }
    hls_url.map(|url| {
        let ua = innertube::video_stream_user_agent(&url).to_string();
        VideoStreamInfo {
            track_id: track_id.to_string(),
            url,
            fallback_url: None,
            user_agent: ua,
            width: 1280,
            height: 720,
            is_hls: true,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_video_candidate_handles_auto_cap_and_hls_fallback() {
        let adaptive = vec![
            (480, 854, 30, "https://rr1.googlevideo.com/videoplayback?c=IOS&q=480".into()),
            (720, 1280, 60, "https://rr1.googlevideo.com/videoplayback?c=IOS&q=720".into()),
            (1080, 1920, 60, "https://rr1.googlevideo.com/videoplayback?c=IOS&q=1080".into()),
        ];
        let auto = select_video_candidate(
            "t1",
            adaptive.clone(),
            vec![],
            Some("https://manifest.googlevideo.com/master.m3u8?c=IOS".into()),
            0,
        )
        .expect("auto picks 720p60");
        assert_eq!(auto.height, 720);
        assert!(auto.url.contains("q=720"));
        assert!(!auto.is_hls);

        let hls = select_video_candidate(
            "t2",
            vec![],
            vec![],
            Some("https://manifest.googlevideo.com/master.m3u8?c=MWEB".into()),
            720,
        )
        .expect("falls back to HLS");
        assert!(hls.is_hls);
        assert_eq!(hls.height, 720);
        assert!(hls.user_agent.contains("Pixel 8"));
    }

    #[test]
    fn subscriber_count_formatting_handles_millions_thousands_and_units() {
        assert_eq!(YtMusicProvider::format_subscriber_count(6_910_000), "6.91 M de suscriptores");
        assert_eq!(YtMusicProvider::format_subscriber_count(59_500_000), "59.5 M de suscriptores");
        assert_eq!(YtMusicProvider::format_subscriber_count(2_000_000), "2 M de suscriptores");
        assert_eq!(YtMusicProvider::format_subscriber_count(450_000), "450 K suscriptores");
        assert_eq!(YtMusicProvider::format_subscriber_count(12_400), "12.4 K suscriptores");
        assert_eq!(YtMusicProvider::format_subscriber_count(820), "820 suscriptores");
    }

    #[tokio::test]
    async fn live_home_and_search_return_music() {
        let provider = YtMusicProvider::new();
        match provider.get_home_feed().await {
            Ok(home) => {
                assert!(
                    !home.quick_picks.is_empty() || !home.shelves.is_empty(),
                    "expected non-empty quick_picks or shelves"
                );
            }
            Err(e) => {
                eprintln!("Live home feed skipped in CI/datacenter IP: {e}");
            }
        }
        match provider.search("daft punk").await {
            Ok(search) => {
                assert!(!search.songs.is_empty() || !search.albums.is_empty(), "expected search items");
            }
            Err(e) => {
                eprintln!("Search skipped in CI/datacenter IP: {e}");
            }
        }
    }
}
