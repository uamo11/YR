use async_trait::async_trait;
use rustypipe::client::RustyPipe;
use rustypipe::model::{AlbumItem, MusicItem, MusicPlaylistItem, TrackItem};
use thiserror::Error;
use yr_core::{
    AlbumDetail, AlbumSummary, ArtistSummary, AudioQuality, PlaylistDetail, PlaylistSummary,
    SearchResults, StreamInfo, Track,
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
    async fn search(&self, query: &str) -> Result<SearchResults, ProviderError>;
    async fn get_album(&self, album_id: &str) -> Result<AlbumDetail, ProviderError>;
    async fn get_playlist(&self, playlist_id: &str) -> Result<PlaylistDetail, ProviderError>;
    async fn get_trending_music(&self) -> Result<Vec<Track>, ProviderError>;
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

    fn map_track(item: TrackItem) -> Track {
        let artist = item
            .artists
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Artista desconocido".to_string());
        let artist_id = item.artists.first().and_then(|a| a.id.clone());
        let thumbnail_url = item.cover.first().map(|c| c.url.clone());

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
            thumbnail_url: item.cover.first().map(|c| c.url.clone()),
        }
    }

    fn map_playlist(item: MusicPlaylistItem) -> PlaylistSummary {
        PlaylistSummary {
            id: item.id,
            title: item.name,
            author: item.channel.map(|c| c.name),
            track_count: item.track_count.map(|v| v as u32),
            thumbnail_url: item.thumbnail.first().map(|t| t.url.clone()),
        }
    }
}

#[async_trait]
impl MusicProvider for YtMusicProvider {
    async fn search(&self, query: &str) -> Result<SearchResults, ProviderError> {
        let res = self
            .client
            .query()
            .music_search_main(query)
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;

        let mut out = SearchResults::default();
        for item in res.items.items {
            match item {
                MusicItem::Track(t) => out.songs.push(Self::map_track(t)),
                MusicItem::Album(a) => out.albums.push(Self::map_album(a)),
                MusicItem::Artist(ar) => out.artists.push(ArtistSummary {
                    id: ar.id,
                    name: ar.name,
                    subscribers: ar.subscriber_count.map(|c| format!("{c} suscriptores")),
                    thumbnail_url: ar.avatar.first().map(|t| t.url.clone()),
                }),
                MusicItem::Playlist(p) => out.playlists.push(Self::map_playlist(p)),
                _ => {}
            }
        }
        Ok(out)
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
            thumbnail_url: album.cover.first().map(|c| c.url.clone()),
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
            thumbnail_url: pl.thumbnail.first().map(|t| t.url.clone()),
        };
        let tracks = pl.tracks.items.into_iter().map(Self::map_track).collect();
        Ok(PlaylistDetail { summary, tracks })
    }

    async fn get_trending_music(&self) -> Result<Vec<Track>, ProviderError> {
        let charts = self
            .client
            .query()
            .music_charts(None)
            .await
            .map_err(|e| ProviderError::Api(e.to_string()))?;

        Ok(charts.top_tracks.into_iter().map(Self::map_track).collect())
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

        // Prefer M4A/AAC streams so pure-Rust symphonia decodes with zero C dependencies,
        // falling back to any audio stream if needed.
        let target_bps = quality.target_bitrate_kbps() * 1000;
        let mut audio_streams: Vec<_> = player.audio_streams;
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
            content_length: if chosen.size > 0 { Some(chosen.size) } else { None },
        })
    }
}
