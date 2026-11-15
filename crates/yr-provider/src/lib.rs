pub mod innertube;

use async_trait::async_trait;
use innertube::GoogleSession;
use rustypipe::client::{OauthDeviceCode, RustyPipe};
use rustypipe::model::{AlbumItem, ArtistItem, MusicPlaylistItem, TrackItem};
use rustypipe::param::Country;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;
use yr_core::{
    AlbumDetail, AlbumSummary, ArtistDetail, ArtistSummary, AudioQuality, GoogleDeviceAuth,
    HomeFeed, HomeShelf, HomeShelfCard, HomeShelfLayout, MoodGenre, PlaylistDetail,
    PlaylistSummary, SearchResults, ShelfCardKind, StreamInfo, Track,
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
    async fn get_radio(&self, track_id: &str) -> Result<Vec<Track>, ProviderError>;
    async fn get_lyrics(&self, track_id: &str) -> Result<Option<String>, ProviderError>;
    async fn resolve_audio_stream(
        &self,
        track_id: &str,
        quality: AudioQuality,
    ) -> Result<StreamInfo, ProviderError>;
    async fn is_logged_in(&self) -> bool;
    async fn start_google_device_login(&self) -> Result<GoogleDeviceAuth, ProviderError>;
    async fn poll_google_device_login(&self) -> Result<bool, ProviderError>;
    async fn login_with_cookie(&self, cookie: &str) -> Result<(), ProviderError>;
    async fn logout(&self) -> Result<(), ProviderError>;
}

/// YouTube Music provider backed by `rustypipe` + direct InnerTube WEB_REMIX.
#[derive(Clone)]
pub struct YtMusicProvider {
    client: RustyPipe,
    http: reqwest::Client,
    cache_dir: Option<PathBuf>,
    google_session: Arc<Mutex<Option<GoogleSession>>>,
    pending_oauth: Arc<Mutex<Option<OauthDeviceCode>>>,
}

impl Default for YtMusicProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl YtMusicProvider {
    pub fn new() -> Self {
        Self {
            client: RustyPipe::new(),
            http: reqwest::Client::new(),
            cache_dir: None,
            google_session: Arc::new(Mutex::new(None)),
            pending_oauth: Arc::new(Mutex::new(None)),
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
            http: reqwest::Client::new(),
            cache_dir: Some(cache_dir),
            google_session: Arc::new(Mutex::new(session)),
            pending_oauth: Arc::new(Mutex::new(None)),
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

    fn map_artist(item: ArtistItem) -> ArtistSummary {
        ArtistSummary {
            id: item.id,
            name: item.name,
            subscribers: item
                .subscriber_count
                .map(|c| format!("{c} suscriptores")),
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
        let logged_in = has_cookie_session || self.is_logged_in().await;

        let mut feed = match innertube::fetch_home_feed(&self.http, session.as_ref(), mood_params).await {
            Ok(f) => f,
            Err(_) => {
                // Fallback to rustypipe charts + new releases
                let q = self.client.query();
                let (charts_res, new_videos_res, new_albums_res, genres_res) = tokio::join!(
                    q.music_charts(Some(Country::Us)),
                    q.music_new_videos(),
                    q.music_new_albums(),
                    q.music_genres()
                );

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
                .map(|c| format!("{c} suscriptores")),
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

    async fn get_radio(&self, track_id: &str) -> Result<Vec<Track>, ProviderError> {
        let radio = self
            .client
            .query()
            .music_radio_track(track_id)
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;
        Ok(radio.items.into_iter().map(Self::map_track).collect())
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

    async fn resolve_audio_stream(
        &self,
        track_id: &str,
        quality: AudioQuality,
    ) -> Result<StreamInfo, ProviderError> {
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

        Ok(StreamInfo {
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
        })
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn live_home_and_search_return_music() {
        let provider = YtMusicProvider::new();
        let home = provider.get_home_feed().await.expect("home feed");
        assert!(
            !home.quick_picks.is_empty(),
            "expected non-empty quick_picks"
        );
        let search = provider.search("daft punk").await.expect("search");
        assert!(!search.songs.is_empty(), "expected non-empty songs");
        assert!(!search.albums.is_empty(), "expected non-empty albums");
    }
}
