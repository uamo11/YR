use async_trait::async_trait;
use rustypipe::client::RustyPipe;
use rustypipe::model::{AlbumItem, ArtistItem, MusicPlaylistItem, TrackItem};
use rustypipe::param::Country;
use std::path::PathBuf;
use thiserror::Error;
use yr_core::{
    AlbumDetail, AlbumSummary, ArtistDetail, ArtistSummary, AudioQuality, HomeFeed, MoodGenre,
    PlaylistDetail, PlaylistSummary, SearchResults, StreamInfo, Track,
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
}

/// YouTube Music provider backed by `rustypipe` (pure Rust + rustls).
#[derive(Clone)]
pub struct YtMusicProvider {
    client: RustyPipe,
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
        }
    }

    pub fn with_cache_dir(cache_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&cache_dir);
        let client = RustyPipe::builder()
            .storage_dir(cache_dir)
            .build()
            .unwrap_or_else(|_| RustyPipe::new());
        Self { client }
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
        let q = self.client.query();
        let (charts_res, new_videos_res, new_albums_res, genres_res) = tokio::join!(
            q.music_charts(Some(Country::Us)),
            q.music_new_videos(),
            q.music_new_albums(),
            q.music_genres()
        );

        let mut quick_picks: Vec<Track> = Vec::new();
        if let Ok(charts) = charts_res {
            quick_picks.extend(charts.top_tracks.into_iter().map(Self::map_track));
            if quick_picks.is_empty() {
                quick_picks.extend(charts.trending_tracks.into_iter().map(Self::map_track));
            }
        }
        if quick_picks.is_empty() {
            if let Ok(videos) = new_videos_res {
                quick_picks.extend(videos.into_iter().map(Self::map_track));
            }
        }
        if quick_picks.is_empty() {
            if let Ok(tracks_page) = q.music_search_tracks("top hits").await {
                quick_picks.extend(tracks_page.items.items.into_iter().map(Self::map_track));
            }
        }

        let new_albums: Vec<AlbumSummary> = new_albums_res
            .map(|albums| albums.into_iter().map(Self::map_album).collect())
            .unwrap_or_default();

        let moods = genres_res
            .map(|genres| {
                genres
                    .into_iter()
                    .take(14)
                    .map(|g| MoodGenre {
                        id: g.id,
                        title: g.name,
                    })
                    .collect()
            })
            .unwrap_or_default();

        if quick_picks.is_empty() && new_albums.is_empty() {
            return Err(ProviderError::Api(
                "No se pudo cargar el feed de YT Music".into(),
            ));
        }

        Ok(HomeFeed {
            quick_picks,
            new_albums,
            moods,
        })
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
