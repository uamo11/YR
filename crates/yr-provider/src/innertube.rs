use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, ORIGIN, REFERER, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use yr_core::{
    AlbumSummary, AudioQuality, ExploreFeed, GoogleAccountInfo, HomeFeed, HomeShelf,
    HomeShelfCard, HomeShelfLayout, LyricLine, LyricsData, MoodGenre, PlaylistSummary,
    ShelfCardKind, SponsorSegment, StreamInfo, Track, VideoStreamInfo,
};

const YTM_ORIGIN: &str = "https://music.youtube.com";
const WEB_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const CLIENT_VERSION: &str = "1.20241113.01.00";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoogleSession {
    pub cookie: String,
    pub account: Option<GoogleAccountInfo>,
    pub session_index: Option<String>,
    pub page_id: Option<String>,
    pub visitor_data: Option<String>,
}

impl GoogleSession {
    pub fn has_auth_cookies(&self) -> bool {
        cookie_value(&self.cookie, "SAPISID").is_some()
            || cookie_value(&self.cookie, "__Secure-3PAPISID").is_some()
            || cookie_value(&self.cookie, "__Secure-1PAPISID").is_some()
    }

    pub fn load(path: &Path) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        let s: Self = serde_json::from_str(&raw).ok()?;
        if s.has_auth_cookies() {
            Some(s)
        } else {
            None
        }
    }

    pub fn save(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn session_file(cache_dir: &Path) -> PathBuf {
        cache_dir.join("google_session.json")
    }
}

pub fn cookie_value<'a>(cookie: &'a str, name: &str) -> Option<&'a str> {
    cookie.split(';').find_map(|part| {
        let part = part.trim();
        let (k, v) = part.split_once('=')?;
        if k.trim() == name {
            let val = v.trim();
            if !val.is_empty() {
                return Some(val);
            }
        }
        None
    })
}

pub fn build_sapisidhash_header(cookie: &str, timestamp_secs: i64) -> Option<String> {
    for (cookie_key, scheme) in [
        ("SAPISID", "SAPISIDHASH"),
        ("__Secure-3PAPISID", "SAPISIDHASH"),
        ("__Secure-1PAPISID", "SAPISIDHASH"),
    ] {
        if let Some(val) = cookie_value(cookie, cookie_key) {
            let mut hasher = Sha1::new();
            hasher.update(format!("{timestamp_secs} {val} {YTM_ORIGIN}"));
            let digest: String = hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            return Some(format!("{scheme} {timestamp_secs}_{digest}"));
        }
    }
    None
}

fn find_json_field<'a>(html: &'a str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let start = html.find(&pat)? + pat.len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    let val = &rest[..end];
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

pub async fn validate_and_hydrate_session(
    http: &reqwest::Client,
    raw_cookie: &str,
) -> Result<GoogleSession, String> {
    let cookie = raw_cookie.trim().to_string();
    let mut session = GoogleSession {
        cookie,
        account: None,
        session_index: None,
        page_id: None,
        visitor_data: None,
    };
    if !session.has_auth_cookies() {
        return Err("La cookie no contiene SAPISID ni __Secure-3PAPISID".to_string());
    }

    // Best-effort scrape session metadata from music.youtube.com
    if let Ok(resp) = http
        .get(YTM_ORIGIN)
        .header(USER_AGENT, WEB_UA)
        .header(COOKIE, &session.cookie)
        .send()
        .await
    {
        if let Ok(html) = resp.text().await {
            session.session_index = find_json_field(&html, "SESSION_INDEX").or(Some("0".to_string()));
            session.visitor_data = find_json_field(&html, "VISITOR_DATA");
            if let Some(ds) = find_json_field(&html, "DELEGATED_SESSION_ID") {
                session.page_id = Some(ds);
            } else if let Some(ds) = find_json_field(&html, "datasyncId") {
                if let Some((p1, p2)) = ds.split_once("||") {
                    if !p2.is_empty() {
                        session.page_id = Some(p1.to_string());
                    }
                }
            }
        }
    }

    // Fetch real account name + avatar photo via account_menu
    if let Ok(acc) = fetch_account_info(http, &session).await {
        session.account = acc;
    }

    Ok(session)
}

async fn post_innertube(
    http: &reqwest::Client,
    endpoint: &str,
    body: Value,
    session: Option<&GoogleSession>,
) -> Result<Value, String> {
    let url = format!("{YTM_ORIGIN}/youtubei/v1/{endpoint}?prettyPrint=false");
    let mut req = http
        .post(url)
        .header(CONTENT_TYPE, "application/json")
        .header(USER_AGENT, WEB_UA)
        .header(ORIGIN, YTM_ORIGIN)
        .header(REFERER, format!("{YTM_ORIGIN}/"))
        .header("X-YouTube-Client-Name", "67")
        .header("X-YouTube-Client-Version", CLIENT_VERSION);

    if let Some(s) = session {
        if s.has_auth_cookies() {
            let ts = chrono::Utc::now().timestamp();
            if let Some(auth) = build_sapisidhash_header(&s.cookie, ts) {
                req = req.header(AUTHORIZATION, auth);
            }
            req = req
                .header(COOKIE, &s.cookie)
                .header("X-Goog-AuthUser", s.session_index.as_deref().unwrap_or("0"));
            if let Some(pid) = &s.page_id {
                req = req.header("X-Goog-PageId", pid);
            }
        }
        if let Some(vd) = &s.visitor_data {
            req = req.header("X-Goog-Visitor-Id", vd);
        }
    }

    let resp = req
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json::<Value>().await.map_err(|e| e.to_string())
}

pub fn parse_account_info_from_value(val: &Value) -> Option<GoogleAccountInfo> {
    let header_candidates = [
        "/actions/0/openPopupAction/popup/multiPageMenuRenderer/header/activeAccountHeaderRenderer",
        "/actions/0/openPopupAction/popup/activeAccountHeaderRenderer",
        "/topbar/musicTopBarRenderer/activeAccountHeaderRenderer",
        "/header/activeAccountHeaderRenderer",
    ];
    for path in header_candidates {
        if let Some(header) = val.pointer(path) {
            let name = runs_text(header.get("accountName"));
            if !name.is_empty() {
                let handle = non_empty(runs_text(header.get("channelHandle")));
                let avatar_url = best_thumb(header.pointer("/accountPhoto/thumbnails"))
                    .or_else(|| best_thumb(header.pointer("/avatar/thumbnails")))
                    .or_else(|| best_thumb(header.pointer("/thumbnail/thumbnails")))
                    .or_else(|| best_thumb(header.pointer("/image/thumbnails")));
                return Some(GoogleAccountInfo {
                    name,
                    handle,
                    avatar_url,
                });
            }
        }
    }
    let topbar_thumb_candidates = [
        "/topbar/musicTopBarRenderer/accountButton/buttonRenderer/image/thumbnails",
        "/topbar/musicTopBarRenderer/accountButton/buttonRenderer/avatar/thumbnails",
        "/topbar/musicTopBarRenderer/accountButton/buttonRenderer/thumbnail/thumbnails",
        "/topbar/musicTopBarRenderer/accountPhoto/thumbnails",
    ];
    for path in topbar_thumb_candidates {
        if let Some(thumbs) = val.pointer(path) {
            if let Some(avatar_url) = best_thumb(Some(thumbs)) {
                return Some(GoogleAccountInfo {
                    name: "Mi Cuenta".to_string(),
                    handle: None,
                    avatar_url: Some(avatar_url),
                });
            }
        }
    }
    None
}

pub async fn fetch_account_info(
    http: &reqwest::Client,
    session: &GoogleSession,
) -> Result<Option<GoogleAccountInfo>, String> {
    let body = json!({
        "context": {
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": CLIENT_VERSION,
                "hl": "es-419",
                "gl": "MX"
            }
        }
    });
    let val = post_innertube(http, "account/account_menu", body, Some(session)).await?;
    Ok(parse_account_info_from_value(&val))
}

pub async fn fetch_home_feed(
    http: &reqwest::Client,
    session: Option<&GoogleSession>,
    mood_params: Option<&str>,
) -> Result<HomeFeed, String> {
    let mut body = json!({
        "context": {
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": CLIENT_VERSION,
                "hl": "es-419",
                "gl": "MX"
            }
        },
        "browseId": "FEmusic_home"
    });
    if let Some(p) = mood_params.filter(|s| !s.is_empty()) {
        body["params"] = Value::String(p.to_string());
    }

    let first = post_innertube(http, "browse", body, session).await?;
    let mut visitor_data = first
        .pointer("/responseContext/visitorData")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut feed = HomeFeed::default();
    feed.is_logged_in = session.map(|s| s.has_auth_cookies()).unwrap_or(false);
    feed.account = session.and_then(|s| s.account.clone());
    if feed.account.is_none() {
        if let Some(acc) = parse_account_info_from_value(&first) {
            feed.account = Some(acc);
        }
    }

    let sec = first.pointer(
        "/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer",
    );
    if let Some(sec) = sec {
        parse_mood_chips(sec, &mut feed.moods);
        if let Some(contents) = sec.get("contents").and_then(|c| c.as_array()) {
            for item in contents {
                parse_shelf_item(item, &mut feed);
            }
        }
        let mut next_token = extract_continuation(sec);
        for _ in 0..2 {
            let Some(ctok) = next_token.take() else {
                break;
            };
            let mut cbody = json!({
                "context": {
                    "client": {
                        "clientName": "WEB_REMIX",
                        "clientVersion": CLIENT_VERSION,
                        "hl": "es-419",
                        "gl": "MX"
                    }
                },
                "continuation": ctok
            });
            if let Some(vd) = &visitor_data {
                cbody["context"]["client"]["visitorData"] = Value::String(vd.clone());
            }
            let Ok(Ok(cresp)) = tokio::time::timeout(
                std::time::Duration::from_secs(4),
                post_innertube(http, "browse", cbody, session),
            )
            .await
            else {
                break;
            };
            if visitor_data.is_none() {
                visitor_data = cresp
                    .pointer("/responseContext/visitorData")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
            let Some(csec) = cresp.pointer("/continuationContents/sectionListContinuation") else {
                break;
            };
            if let Some(contents) = csec.get("contents").and_then(|c| c.as_array()) {
                for item in contents {
                    parse_shelf_item(item, &mut feed);
                }
            }
            next_token = extract_continuation(csec);
        }
    }

    // Backfill legacy HomeFeed fields from parsed shelves so everything stays coherent
    for shelf in &feed.shelves {
        match shelf.layout {
            HomeShelfLayout::QuickPicks4Row => {
                if feed.quick_picks.is_empty() {
                    feed.quick_picks = shelf.tracks.clone();
                } else if feed.recommended_tracks.is_empty() {
                    feed.recommended_tracks = shelf.tracks.clone();
                }
            }
            HomeShelfLayout::CarouselCards | HomeShelfLayout::SpeedDialGrid => {
                for card in &shelf.cards {
                    match &card.kind {
                        ShelfCardKind::Album(a) => {
                            if feed.new_albums.len() < 16 {
                                feed.new_albums.push(a.clone());
                            }
                        }
                        ShelfCardKind::Playlist(p) => {
                            if feed.recommended_playlists.len() < 16 {
                                feed.recommended_playlists.push(p.clone());
                            }
                        }
                        ShelfCardKind::Track(t) => {
                            if feed.recommended_tracks.len() < 16 {
                                feed.recommended_tracks.push(t.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    if feed.quick_picks.is_empty() && !feed.recommended_tracks.is_empty() {
        feed.quick_picks = feed.recommended_tracks.clone();
    }

    if feed.shelves.is_empty() && feed.quick_picks.is_empty() {
        return Err("FEmusic_home devolvió un feed vacío".to_string());
    }
    Ok(feed)
}

fn parse_mood_chips(sec: &Value, out: &mut Vec<MoodGenre>) {
    let Some(chips) = sec.pointer("/header/chipCloudRenderer/chips").and_then(|c| c.as_array())
    else {
        return;
    };
    for chip in chips {
        let Some(r) = chip.get("chipCloudChipRenderer") else {
            continue;
        };
        let title = runs_text(r.get("text"));
        if title.is_empty() {
            continue;
        }
        let browse_id = r
            .pointer("/navigationEndpoint/browseEndpoint/browseId")
            .and_then(|v| v.as_str())
            .unwrap_or("FEmusic_home")
            .to_string();
        let params = r
            .pointer("/navigationEndpoint/browseEndpoint/params")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        out.push(MoodGenre {
            id: browse_id,
            title,
            params,
        });
    }
}

fn extract_continuation(container: &Value) -> Option<String> {
    container
        .pointer("/continuations/0/nextContinuationData/continuation")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn parse_shelf_item(item: &Value, feed: &mut HomeFeed) {
    let (shelf_obj, is_grid) = if let Some(v) = item.get("musicCarouselShelfRenderer") {
        (v, false)
    } else if let Some(v) = item.get("musicShelfRenderer") {
        (v, false)
    } else if let Some(v) = item.get("gridRenderer") {
        (v, true)
    } else {
        return;
    };

    let header = shelf_obj
        .pointer("/header/musicCarouselShelfBasicHeaderRenderer")
        .or_else(|| shelf_obj.pointer("/header/musicShelfHeaderRenderer"))
        .or_else(|| shelf_obj.get("header"));
    let title = header
        .map(|h| runs_text(h.get("title")))
        .unwrap_or_default();
    if title.is_empty() {
        return;
    }
    let eyebrow = header
        .map(|h| runs_text(h.get("strapline")))
        .and_then(non_empty);

    let raw_items = shelf_obj
        .get("contents")
        .or_else(|| shelf_obj.get("items"))
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    if raw_items.is_empty() {
        return;
    }

    let mut tracks = Vec::new();
    let mut cards = Vec::new();
    let mut responsive_count = 0usize;
    let mut two_row_count = 0usize;

    for raw in &raw_items {
        if let Some(r) = raw.get("musicResponsiveListItemRenderer") {
            responsive_count += 1;
            if let Some((track, subtitle)) = parse_responsive_track(r) {
                cards.push(HomeShelfCard {
                    title: track.title.clone(),
                    subtitle,
                    thumbnail_url: track.thumbnail_url.clone(),
                    kind: ShelfCardKind::Track(track.clone()),
                });
                tracks.push(track);
            }
        } else if let Some(r) = raw.get("musicTwoRowItemRenderer") {
            two_row_count += 1;
            if let Some(card) = parse_two_row_card(r) {
                if let ShelfCardKind::Track(t) = &card.kind {
                    tracks.push(t.clone());
                }
                cards.push(card);
            }
        }
    }

    if cards.is_empty() && tracks.is_empty() {
        return;
    }

    let lower = title.to_lowercase();
    let layout = if is_grid
        || lower.contains("accesos directos")
        || lower.contains("speed dial")
    {
        feed.speed_dial = cards.clone();
        HomeShelfLayout::SpeedDialGrid
    } else if responsive_count >= two_row_count && !tracks.is_empty() {
        HomeShelfLayout::QuickPicks4Row
    } else {
        HomeShelfLayout::CarouselCards
    };

    feed.shelves.push(HomeShelf {
        title,
        eyebrow,
        layout,
        tracks,
        cards,
    });
}

fn parse_responsive_track(r: &Value) -> Option<(Track, String)> {
    let flex = r.get("flexColumns")?.as_array()?;
    let col0 = flex
        .first()
        .and_then(|c| c.pointer("/musicResponsiveListItemFlexColumnRenderer/text"));
    let col1 = flex
        .get(1)
        .and_then(|c| c.pointer("/musicResponsiveListItemFlexColumnRenderer/text"));
    let col2 = flex
        .get(2)
        .and_then(|c| c.pointer("/musicResponsiveListItemFlexColumnRenderer/text"));

    let title = runs_text(col0);
    if title.is_empty() {
        return None;
    }

    let video_id = r
        .pointer("/playlistItemData/videoId")
        .or_else(|| r.pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId"))
        .or_else(|| {
            col0.and_then(|c| c.pointer("/runs/0/navigationEndpoint/watchEndpoint/videoId"))
        })
        .and_then(|v| v.as_str())?
        .to_string();

    let (artist, artist_id) = col1
        .and_then(|c| c.get("runs"))
        .and_then(|runs| runs.as_array())
        .and_then(|arr| {
            let first_non_sep = arr.iter().find(|run| {
                let txt = run.get("text").and_then(|t| t.as_str()).unwrap_or("").trim();
                !txt.is_empty() && txt != "•" && txt != "," && txt != "&"
            });
            first_non_sep.map(|run| {
                let name = run
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Artista desconocido")
                    .to_string();
                let id = run
                    .pointer("/navigationEndpoint/browseEndpoint/browseId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                (name, id)
            })
        })
        .unwrap_or_else(|| ("YouTube Music".to_string(), None));

    let subtitle_full = runs_text(col1);
    let (album, album_id) = col2
        .map(|c| {
            let name = non_empty(runs_text(Some(c)));
            let id = c
                .pointer("/runs/0/navigationEndpoint/browseEndpoint/browseId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (name, id)
        })
        .unwrap_or((None, None));

    let fixed_dur = r
        .pointer("/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text")
        .map(|v| runs_text(Some(v)))
        .and_then(|s| parse_duration_text(&s));

    let thumbnail_url = best_thumb(r.pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails"));
    let duration_secs = fixed_dur.or_else(|| parse_duration_text(&subtitle_full));

    let track = Track {
        id: video_id,
        title,
        artist: artist.clone(),
        artist_id,
        album,
        album_id,
        duration_secs,
        thumbnail_url,
        local_path: None,
    };
    let subtitle = if subtitle_full.is_empty() {
        artist
    } else {
        subtitle_full
    };
    Some((track, subtitle))
}

fn is_non_artist_segment(seg: &str) -> bool {
    let low = seg.trim().to_lowercase();
    if low.is_empty() {
        return true;
    }
    let media_types = [
        "canción",
        "cancion",
        "song",
        "video",
        "vídeo",
        "álbum",
        "album",
        "sencillo",
        "single",
        "ep",
        "playlist",
        "lista",
        "episodio",
        "episode",
        "pódcast",
        "podcast",
    ];
    if media_types.contains(&low.as_str()) {
        return true;
    }
    let count_tokens = [
        "vistas",
        "views",
        "reproducciones",
        "plays",
        "suscriptores",
        "subscribers",
        "canciones",
        "songs",
    ];
    if count_tokens.iter().any(|tok| low.contains(tok)) {
        return true;
    }
    if seg.contains(':') && parse_duration_text(seg).is_some() {
        return true;
    }
    let trimmed = seg.trim();
    if trimmed.len() == 4 && trimmed.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    false
}

pub fn extract_artist_from_subtitle(subtitle: &str) -> String {
    for part in subtitle.split('•') {
        let seg = part.trim();
        if !is_non_artist_segment(seg) {
            return seg.to_string();
        }
    }
    "YouTube Music".to_string()
}

fn parse_two_row_card(r: &Value) -> Option<HomeShelfCard> {
    let title = runs_text(r.get("title"));
    if title.is_empty() {
        return None;
    }
    let subtitle = runs_text(r.get("subtitle"));
    let thumbnail_url = best_thumb(
        r.pointer("/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails"),
    );

    let nav = r
        .get("navigationEndpoint")
        .or_else(|| r.pointer("/title/runs/0/navigationEndpoint"));

    if let Some(vid) = nav
        .and_then(|n| n.pointer("/watchEndpoint/videoId"))
        .and_then(|v| v.as_str())
    {
        let (run_artist, run_artist_id) = r
            .pointer("/subtitle/runs")
            .and_then(|v| v.as_array())
            .and_then(|runs| {
                runs.iter().find_map(|run| {
                    let browse_id = run
                        .pointer("/navigationEndpoint/browseEndpoint/browseId")
                        .and_then(|v| v.as_str())?;
                    let page_type = run
                        .pointer("/navigationEndpoint/browseEndpoint/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if page_type == "MUSIC_PAGE_TYPE_ARTIST" || browse_id.starts_with("UC") {
                        let name = run
                            .get("text")
                            .and_then(|t| t.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())?;
                        Some((name.to_string(), Some(browse_id.to_string())))
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_else(|| (extract_artist_from_subtitle(&subtitle), None));
        let duration_secs = parse_duration_text(&subtitle);
        let track = Track {
            id: vid.to_string(),
            title: title.clone(),
            artist: run_artist,
            artist_id: run_artist_id,
            album: None,
            album_id: None,
            duration_secs,
            thumbnail_url: thumbnail_url.clone(),
            local_path: None,
        };
        return Some(HomeShelfCard {
            title,
            subtitle,
            thumbnail_url,
            kind: ShelfCardKind::Track(track),
        });
    }

    if let Some(browse_id) = nav
        .and_then(|n| n.pointer("/browseEndpoint/browseId"))
        .and_then(|v| v.as_str())
    {
        let page_type = nav
            .and_then(|n| {
                n.pointer("/browseEndpoint/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
            })
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let is_album = page_type == "MUSIC_PAGE_TYPE_ALBUM" || browse_id.starts_with("MPREb_");
        if is_album {
            let artist = subtitle
                .split('•')
                .last()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| subtitle.clone());
            return Some(HomeShelfCard {
                title: title.clone(),
                subtitle,
                thumbnail_url: thumbnail_url.clone(),
                kind: ShelfCardKind::Album(AlbumSummary {
                    id: browse_id.to_string(),
                    title,
                    artist,
                    year: None,
                    thumbnail_url,
                }),
            });
        }
        let pl_id = browse_id.strip_prefix("VL").unwrap_or(browse_id).to_string();
        return Some(HomeShelfCard {
            title: title.clone(),
            subtitle: subtitle.clone(),
            thumbnail_url: thumbnail_url.clone(),
            kind: ShelfCardKind::Playlist(PlaylistSummary {
                id: pl_id,
                title,
                author: non_empty(subtitle),
                track_count: None,
                thumbnail_url,
            }),
        });
    }

    None
}

fn runs_text(val: Option<&Value>) -> String {
    let Some(val) = val else {
        return String::new();
    };
    if let Some(s) = val.get("simpleText").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    val.get("runs")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|r| r.get("text").and_then(|t| t.as_str()))
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// Parses `"M:SS"`, `"H:MM:SS"`, or bullet-delimited subtitles like `"Artist • 3:44"` into seconds.
fn parse_duration_text(text: &str) -> Option<u64> {
    text.split('•').rev().find_map(|segment| {
        let s = segment.trim();
        let parts: Vec<&str> = s.split(':').collect();
        match parts.as_slice() {
            [m, sec] => {
                let mins: u64 = m.trim().parse().ok()?;
                let secs: u64 = sec.trim().parse().ok()?;
                if secs < 60 {
                    Some(mins * 60 + secs)
                } else {
                    None
                }
            }
            [h, m, sec] => {
                let hrs: u64 = h.trim().parse().ok()?;
                let mins: u64 = m.trim().parse().ok()?;
                let secs: u64 = sec.trim().parse().ok()?;
                if mins < 60 && secs < 60 {
                    Some(hrs * 3600 + mins * 60 + secs)
                } else {
                    None
                }
            }
            _ => None,
        }
    })
}

fn best_thumb(thumbs: Option<&Value>) -> Option<String> {
    let arr = thumbs?.as_array()?;
    let raw = arr
        .last()
        .or_else(|| arr.first())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())?;
    let trimmed = raw.trim();
    if trimmed.starts_with("//") {
        Some(format!("https:{trimmed}"))
    } else {
        Some(trimmed.to_string())
    }
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

const VISIONOS_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_6) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15";
const ANDROID_VR_UA: &str = "com.google.android.apps.youtube.vr.oculus/1.60.19 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip";
const IOS_PLAYER_UA: &str = "com.google.ios.youtube/20.03.02 (iPhone16,2; U; CPU iOS 18_2_1 like Mac OS X;)";
const ANDROID_PLAYER_UA: &str = "com.google.android.youtube/20.10.38 (Linux; U; Android 14; Mobile) gzip";

pub fn video_stream_user_agent(url: &str) -> &'static str {
    if url.contains("c=MWEB") || url.contains("c=WEB") {
        "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36"
    } else if url.contains("c=VISIONOS") {
        VISIONOS_UA
    } else if url.contains("c=IOS") {
        IOS_PLAYER_UA
    } else if url.contains("c=ANDROID_VR") {
        ANDROID_VR_UA
    } else if url.contains("c=ANDROID") {
        ANDROID_PLAYER_UA
    } else {
        "Mozilla/5.0 (Linux; Android 14; Mobile) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36"
    }
}
static VISITOR_DATA_CACHE: OnceLock<RwLock<Option<(String, Instant)>>> = OnceLock::new();

fn decode_visitor_data(raw: &str) -> String {
    raw.replace("\\u003d", "=")
        .replace("\\u003D", "=")
        .replace("%3D", "=")
        .replace("%3d", "=")
        .replace("%2B", "+")
        .replace("%2b", "+")
        .replace("%2F", "/")
        .replace("%2f", "/")
}

async fn get_or_fetch_visitor_data(
    http: &reqwest::Client,
    force_refresh: bool,
) -> Result<String, String> {
    let cache = VISITOR_DATA_CACHE.get_or_init(|| RwLock::new(None));
    if !force_refresh {
        let guard = cache.read().await;
        if let Some((ref vd, fetched_at)) = *guard {
            if fetched_at.elapsed() < Duration::from_secs(1800) {
                return Ok(vd.clone());
            }
        }
    }

    for page_url in [YTM_ORIGIN, "https://www.youtube.com/"] {
        let Ok(resp) = http
            .get(page_url)
            .header(USER_AGENT, WEB_UA)
            .header("Accept-Language", "es-419,es;q=0.9,en;q=0.8")
            .send()
            .await
        else {
            continue;
        };
        if let Ok(html) = resp.text().await {
            if let Some(raw) = find_json_field(&html, "VISITOR_DATA")
                .or_else(|| find_json_field(&html, "visitorData"))
            {
                let vd = decode_visitor_data(&raw);
                if !vd.is_empty() {
                    let mut guard = cache.write().await;
                    *guard = Some((vd.clone(), Instant::now()));
                    return Ok(vd);
                }
            }
        }
    }
    Err("visitorData unavailable".to_string())
}

fn pick_best_audio_stream(video_id: &str, val: &Value, quality: AudioQuality) -> Option<StreamInfo> {
    let target_bps = quality.target_bitrate_kbps() * 1000;
    let formats = val
        .pointer("/streamingData/adaptiveFormats")
        .and_then(|v| v.as_array())?;

    let mut candidates: Vec<(u64, StreamInfo)> = Vec::new();
    for f in formats {
        let Some(mime) = f.get("mimeType").and_then(|v| v.as_str()) else {
            continue;
        };
        let is_m4a = mime.contains("mp4") || mime.contains("m4a") || mime.contains("aac");
        let is_mp3 = mime.contains("mpeg") || mime.contains("mp3");
        if !is_m4a && !is_mp3 {
            continue;
        }
        let Some(url) = f.get("url").and_then(|v| v.as_str()) else {
            continue;
        };
        let bitrate = f
            .get("bitrate")
            .and_then(|v| v.as_u64())
            .unwrap_or(128_000) as u32;
        let content_length = f
            .get("contentLength")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok());
        let ext = if is_m4a { "m4a" } else { "mp3" };
        // Strongly prefer M4A/AAC since yr-audio decodes isomp4+aac natively.
        let penalty = if is_m4a { 0_u64 } else { 10_000_000_u64 };
        let diff = (bitrate as i64 - target_bps as i64).unsigned_abs();
        candidates.push((
            penalty + diff,
            StreamInfo {
                track_id: video_id.to_string(),
                url: url.to_string(),
                mime_type: mime.to_string(),
                extension: ext.to_string(),
                bitrate_bps: bitrate,
                content_length,
            },
        ));
    }

    candidates
        .into_iter()
        .min_by_key(|(score, _)| *score)
        .map(|(_, best)| best)
}

fn pick_best_video_stream(
    video_id: &str,
    val: &Value,
    max_height: u32,
    user_agent: &str,
) -> Option<VideoStreamInfo> {
    let hls_url = val
        .pointer("/streamingData/hlsManifestUrl")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let collect_video =
        |arr: Option<&Vec<Value>>, mime_sub: &str, codec_sub: Option<&str>| -> Vec<(u32, u32, u32, String)> {
            let mut list = Vec::new();
            let Some(items) = arr else {
                return list;
            };
            for f in items {
                let Some(mime) = f.get("mimeType").and_then(|v| v.as_str()) else {
                    continue;
                };
                if !mime.contains(mime_sub) {
                    continue;
                }
                if let Some(c) = codec_sub {
                    if !mime.contains(c) {
                        continue;
                    }
                }
                let Some(url) = f.get("url").and_then(|v| v.as_str()) else {
                    continue;
                };
                let height = f.get("height").and_then(|v| v.as_u64()).unwrap_or(360) as u32;
                let width = f
                    .get("width")
                    .and_then(|v| v.as_u64())
                    .unwrap_or((height as u64 * 16) / 9) as u32;
                let fps = f.get("fps").and_then(|v| v.as_u64()).unwrap_or(30) as u32;
                list.push((height, width, fps, url.to_string()));
            }
            list
        };

    let adaptive = val
        .pointer("/streamingData/adaptiveFormats")
        .and_then(|v| v.as_array());
    let muxed = val
        .pointer("/streamingData/formats")
        .and_then(|v| v.as_array());
    let mut candidates = collect_video(adaptive, "video/mp4", Some("avc1"));
    if candidates.is_empty() {
        candidates = collect_video(muxed, "video/mp4", None);
    }
    if candidates.is_empty() {
        candidates = collect_video(adaptive, "video/webm", None);
    }

    let cap = if max_height == 0 { 720 } else { max_height };
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
        return Some(VideoStreamInfo {
            track_id: video_id.to_string(),
            url,
            fallback_url: hls_url,
            user_agent: user_agent.to_string(),
            width: w,
            height: h,
            is_hls: false,
        });
    }

    hls_url.map(|url| VideoStreamInfo {
        track_id: video_id.to_string(),
        url,
        fallback_url: None,
        user_agent: user_agent.to_string(),
        width: 1280,
        height: 720,
        is_hls: true,
    })
}

async fn fetch_visionos_player_json(
    http: &reqwest::Client,
    video_id: &str,
) -> Result<Value, String> {
    let mut last_err = "VISIONOS player failed".to_string();
    for attempt in 0..2 {
        let vd = get_or_fetch_visitor_data(http, attempt > 0).await?;
        let body = json!({
            "videoId": video_id,
            "context": {
                "client": {
                    "clientName": "VISIONOS",
                    "clientVersion": "0.1",
                    "osName": "VISION_OS",
                    "osVersion": "1.3",
                    "deviceMake": "Apple",
                    "deviceModel": "RealityDevice14,1",
                    "platform": "MOBILE",
                    "hl": "es",
                    "gl": "US",
                    "visitorData": vd
                }
            },
            "contentCheckOk": true,
            "racyCheckOk": true
        });
        for endpoint in [
            "https://music.youtube.com/youtubei/v1/player?prettyPrint=false",
            "https://www.youtube.com/youtubei/v1/player?prettyPrint=false",
        ] {
            let resp = match http
                .post(endpoint)
                .header(CONTENT_TYPE, "application/json")
                .header(USER_AGENT, VISIONOS_UA)
                .header("X-Goog-Visitor-Id", &vd)
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    last_err = e.to_string();
                    continue;
                }
            };
            if !resp.status().is_success() {
                last_err = format!("HTTP {}", resp.status());
                continue;
            }
            let Ok(val) = resp.json::<Value>().await else {
                last_err = "Invalid JSON from VISIONOS player".to_string();
                continue;
            };
            let status = val
                .pointer("/playabilityStatus/status")
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN");
            if status == "OK" && val.get("streamingData").is_some() {
                return Ok(val);
            }
            last_err = format!("VISIONOS playabilityStatus={status}");
        }
    }
    Err(last_err)
}

async fn fetch_mobile_player_json(
    http: &reqwest::Client,
    video_id: &str,
    client_name: &str,
    client_version: &str,
    ua: &str,
    client_id: &str,
    visitor_data: Option<&str>,
) -> Result<Value, String> {
    let client_obj = if client_name == "IOS" {
        json!({
            "clientName": client_name,
            "clientVersion": client_version,
            "deviceMake": "Apple",
            "deviceModel": "iPhone16,2",
            "hl": "es-419",
            "gl": "MX",
            "osName": "iPhone",
            "osVersion": "18.2.1.22C161",
            "platform": "MOBILE"
        })
    } else {
        let mut c = json!({
            "clientName": "ANDROID",
            "clientVersion": client_version,
            "hl": "es-419",
            "gl": "MX",
            "androidSdkVersion": 34,
            "osName": "Android",
            "osVersion": "14"
        });
        if let Some(vd) = visitor_data.filter(|s| !s.is_empty()) {
            c["visitorData"] = Value::String(vd.to_string());
        }
        c
    };
    let body = json!({
        "context": {
            "client": client_obj
        },
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true
    });
    let mut req = http
        .post("https://www.youtube.com/youtubei/v1/player?prettyPrint=false")
        .header(CONTENT_TYPE, "application/json")
        .header(USER_AGENT, ua)
        .header("X-YouTube-Client-Name", client_id)
        .header("X-YouTube-Client-Version", client_version);
    if client_name == "ANDROID" {
        if let Some(vd) = visitor_data.filter(|s| !s.is_empty()) {
            req = req.header("X-Goog-Visitor-Id", vd);
        }
    }
    let resp = req
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{client_name} HTTP {}", resp.status()));
    }
    let val = resp
        .json::<Value>()
        .await
        .map_err(|_| format!("{client_name} invalid JSON"))?;
    let status = val
        .pointer("/playabilityStatus/status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if status != "OK" {
        return Err(val
            .pointer("/playabilityStatus/reason")
            .and_then(|v| v.as_str())
            .unwrap_or(status)
            .to_string());
    }
    Ok(val)
}

async fn fetch_visionos_stream(
    http: &reqwest::Client,
    video_id: &str,
    quality: AudioQuality,
) -> Result<StreamInfo, String> {
    let val = fetch_visionos_player_json(http, video_id).await?;
    pick_best_audio_stream(video_id, &val, quality)
        .ok_or_else(|| "No audio stream in VISIONOS response".to_string())
}

/// Resolves a direct hardware-decodable H.264 MP4, VP9 WebM, or HLS video stream via InnerTube.
pub async fn fetch_player_video_stream(
    http: &reqwest::Client,
    video_id: &str,
    max_height: u32,
) -> Result<VideoStreamInfo, String> {
    if let Ok(val) = fetch_visionos_player_json(http, video_id).await {
        if let Some(v) = pick_best_video_stream(video_id, &val, max_height, VISIONOS_UA) {
            return Ok(v);
        }
    }
    let vd = get_or_fetch_visitor_data(http, false).await.ok();
    let clients = [
        ("IOS", "20.03.02", IOS_PLAYER_UA, "5", None),
        ("ANDROID", "20.10.38", ANDROID_PLAYER_UA, "3", vd.as_deref()),
    ];
    let mut last_err = "No compatible MP4/HLS video stream".to_string();
    for (client_name, client_version, ua, client_id, c_vd) in clients {
        match fetch_mobile_player_json(http, video_id, client_name, client_version, ua, client_id, c_vd).await {
            Ok(val) => {
                if let Some(v) = pick_best_video_stream(video_id, &val, max_height, ua) {
                    return Ok(v);
                }
            }
            Err(e) => {
                if last_err.starts_with("No ") || !e.contains("HTTP ") {
                    last_err = e;
                }
            }
        }
    }
    Err(last_err)
}

/// Fast direct stream extraction via InnerTube `VISIONOS 0.1` (+ visitorData),
/// falling back to `IOS 20.03.02` / `ANDROID 20.10.38` player clients.
pub async fn fetch_player_stream(
    http: &reqwest::Client,
    video_id: &str,
    quality: AudioQuality,
) -> Result<StreamInfo, String> {
    if let Ok(stream) = fetch_visionos_stream(http, video_id, quality).await {
        return Ok(stream);
    }
    let vd = get_or_fetch_visitor_data(http, false).await.ok();
    let clients = [
        ("IOS", "20.03.02", IOS_PLAYER_UA, "5", None),
        ("ANDROID", "20.10.38", ANDROID_PLAYER_UA, "3", vd.as_deref()),
    ];

    let mut last_err = String::from("No stream returned");
    for (client_name, client_version, ua, client_id, c_vd) in clients {
        match fetch_mobile_player_json(http, video_id, client_name, client_version, ua, client_id, c_vd)
            .await
        {
            Ok(val) => {
                if let Some(best) = pick_best_audio_stream(video_id, &val, quality) {
                    return Ok(best);
                }
            }
            Err(e) => {
                if last_err.starts_with("No ") || !e.contains("HTTP ") {
                    last_err = e;
                }
            }
        }
    }

    Err(last_err)
}

pub async fn fetch_explore_feed(
    http: &reqwest::Client,
    session: Option<&GoogleSession>,
) -> Result<ExploreFeed, String> {
    let body = json!({
        "context": {
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": CLIENT_VERSION,
                "hl": "es-419",
                "gl": "MX"
            }
        },
        "browseId": "FEmusic_explore"
    });
    let val = post_innertube(http, "browse", body, session).await?;
    let mut tmp_feed = HomeFeed::default();
    if let Some(contents) = val
        .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .and_then(|c| c.as_array())
    {
        for item in contents {
            parse_shelf_item(item, &mut tmp_feed);
            let button_arrays = [
                item.pointer("/gridRenderer/items").and_then(|a| a.as_array()),
                item.pointer("/musicCarouselShelfRenderer/contents")
                    .and_then(|a| a.as_array()),
            ];
            for arr_opt in button_arrays {
                let Some(cells) = arr_opt else { continue };
                for cell in cells {
                    if let Some(nav) = cell.get("musicNavigationButtonRenderer") {
                        let title = runs_text(nav.get("buttonText"));
                        let browse_id = nav
                            .pointer("/clickCommand/browseEndpoint/browseId")
                            .and_then(|v| v.as_str());
                        let params = nav
                            .pointer("/clickCommand/browseEndpoint/params")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        if !title.is_empty()
                            && browse_id == Some("FEmusic_moods_and_genres_category")
                        {
                            if let Some(p) = params.filter(|s| !s.is_empty()) {
                                tmp_feed.moods.push(MoodGenre {
                                    id: p,
                                    title,
                                    params: None,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    let mut trending = tmp_feed.trending_tracks;
    if trending.is_empty() {
        trending = tmp_feed.quick_picks;
    }
    if trending.is_empty() {
        trending = tmp_feed.recommended_tracks;
    }
    for shelf in &tmp_feed.shelves {
        for card in &shelf.cards {
            match &card.kind {
                ShelfCardKind::Album(a) => {
                    if tmp_feed.new_albums.len() < 24 {
                        tmp_feed.new_albums.push(a.clone());
                    }
                }
                ShelfCardKind::Track(t) => {
                    if !trending.iter().any(|x| x.id == t.id) {
                        trending.push(t.clone());
                    }
                }
                _ => {}
            }
        }
        for t in &shelf.tracks {
            if !trending.iter().any(|x| x.id == t.id) {
                trending.push(t.clone());
            }
        }
    }
    Ok(ExploreFeed {
        new_albums: tmp_feed.new_albums,
        trending_tracks: trending,
        moods: tmp_feed.moods,
    })
}

pub async fn fetch_mood_playlists(
    http: &reqwest::Client,
    session: Option<&GoogleSession>,
    genre_params: &str,
) -> Result<Vec<PlaylistSummary>, String> {
    let body = json!({
        "context": {
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": CLIENT_VERSION,
                "hl": "es-419",
                "gl": "MX"
            }
        },
        "browseId": "FEmusic_moods_and_genres_category",
        "params": genre_params
    });
    let val = post_innertube(http, "browse", body, session).await?;
    let mut tmp = HomeFeed::default();
    if let Some(contents) = val
        .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .and_then(|c| c.as_array())
    {
        for item in contents {
            parse_shelf_item(item, &mut tmp);
        }
    }
    let mut out = tmp.recommended_playlists;
    if out.is_empty() {
        for shelf in tmp.shelves {
            for card in shelf.cards {
                if let ShelfCardKind::Playlist(p) = card.kind {
                    out.push(p);
                }
            }
        }
    }
    Ok(out)
}

pub async fn fetch_browse_collection(
    http: &reqwest::Client,
    session: Option<&GoogleSession>,
    raw_id: &str,
) -> Result<(String, Option<String>, Option<u16>, Option<String>, Vec<Track>), String> {
    let browse_id = if raw_id.starts_with("VL") || raw_id.starts_with("MPREb_") {
        raw_id.to_string()
    } else {
        format!("VL{raw_id}")
    };
    let body = json!({
        "context": {
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": CLIENT_VERSION,
                "hl": "es-419",
                "gl": "MX"
            }
        },
        "browseId": browse_id
    });
    let val = post_innertube(http, "browse", body, session).await?;

    let hdr = val
        .pointer("/contents/twoColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicResponsiveHeaderRenderer")
        .or_else(|| val.pointer("/header/musicDetailHeaderRenderer"))
        .or_else(|| val.pointer("/header/musicEditablePlaylistDetailHeaderRenderer/header/musicDetailHeaderRenderer"));

    let title = hdr
        .map(|h| runs_text(h.get("title")))
        .and_then(non_empty)
        .unwrap_or_else(|| "Lista de reproducción".to_string());
    let subtitle = hdr.map(|h| runs_text(h.get("subtitle"))).unwrap_or_default();
    let strapline = hdr
        .and_then(|h| h.get("straplineTextOne"))
        .map(|v| runs_text(Some(v)))
        .and_then(non_empty);
    let artist_or_author = strapline.or_else(|| {
        let ex = extract_artist_from_subtitle(&subtitle);
        if ex == "YouTube Music" {
            non_empty(subtitle.clone())
        } else {
            Some(ex)
        }
    });
    let year = subtitle.split('•').find_map(|seg| {
        let s = seg.trim();
        if s.len() == 4 && s.chars().all(|c| c.is_ascii_digit()) {
            s.parse::<u16>().ok()
        } else {
            None
        }
    });
    let thumb = hdr.and_then(|h| {
        best_thumb(h.pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails"))
            .or_else(|| best_thumb(h.pointer("/thumbnail/croppedSquareThumbnailRenderer/thumbnail/thumbnails")))
    });

    let sections = val
        .pointer("/contents/twoColumnBrowseResultsRenderer/secondaryContents/sectionListRenderer/contents")
        .or_else(|| val.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents"))
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();

    let mut tracks = Vec::new();
    for sec in &sections {
        let items = sec
            .pointer("/musicPlaylistShelfRenderer/contents")
            .or_else(|| sec.pointer("/musicShelfRenderer/contents"))
            .and_then(|c| c.as_array());
        if let Some(arr) = items {
            for entry in arr {
                if let Some(r) = entry.get("musicResponsiveListItemRenderer") {
                    if let Some((mut t, _)) = parse_responsive_track(r) {
                        if t.artist == "YouTube Music" {
                            if let Some(ref a) = artist_or_author {
                                t.artist = a.clone();
                            }
                        }
                        if t.thumbnail_url.is_none() {
                            t.thumbnail_url = thumb.clone();
                        }
                        tracks.push(t);
                    }
                }
            }
        }
    }

    if tracks.is_empty() {
        return Err("La colección no devolvió pistas".to_string());
    }
    Ok((title, artist_or_author, year, thumb, tracks))
}

pub async fn fetch_related_tracks(
    http: &reqwest::Client,
    session: Option<&GoogleSession>,
    video_id: &str,
) -> Result<Vec<Track>, String> {
    let body = json!({
        "context": {
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": CLIENT_VERSION,
                "hl": "es-419",
                "gl": "MX"
            }
        },
        "videoId": video_id,
        "enablePersistentPlaylistPanel": true
    });
    let val = post_innertube(http, "next", body, session).await?;
    let mut tracks = Vec::new();
    let mut related_browse_id: Option<String> = None;

    if let Some(tabs) = val
        .pointer("/contents/singleColumnMusicWatchNextResultsRenderer/tabbedRenderer/watchNextTabbedResultsRenderer/tabs")
        .and_then(|v| v.as_array())
    {
        for tab in tabs {
            if let Some(panel_items) = tab
                .pointer("/tabRenderer/content/musicQueueRenderer/content/playlistPanelRenderer/contents")
                .and_then(|v| v.as_array())
            {
                for entry in panel_items {
                    if let Some(vid) = entry.get("playlistPanelVideoRenderer") {
                        let id = vid
                            .get("videoId")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        if id.is_empty() || id == video_id {
                            continue;
                        }
                        let title = runs_text(vid.get("title"));
                        let artist = runs_text(vid.get("shortBylineText"));
                        let dur_secs = parse_duration_text(&runs_text(vid.get("lengthText")))
                            .or_else(|| parse_duration_text(&runs_text(vid.get("longBylineText"))));
                        let thumb = best_thumb(vid.pointer("/thumbnail/thumbnails"));
                        if !title.is_empty() {
                            tracks.push(Track {
                                id: id.to_string(),
                                title,
                                artist: if artist.is_empty() {
                                    "YouTube Music".into()
                                } else {
                                    artist
                                },
                                artist_id: None,
                                album: None,
                                album_id: None,
                                duration_secs: dur_secs,
                                thumbnail_url: thumb,
                                local_path: None,
                            });
                        }
                    }
                }
            }
            if let Some(bid) = tab
                .pointer("/tabRenderer/endpoint/browseEndpoint/browseId")
                .and_then(|v| v.as_str())
            {
                if related_browse_id.is_none() && (bid.starts_with("MPTR") || tab.pointer("/tabRenderer/title").and_then(|v| v.as_str()).is_some_and(|t| t.to_lowercase().contains("relac") || t.to_lowercase().contains("relat"))) {
                    related_browse_id = Some(bid.to_string());
                }
            }
        }
    }

    if let Some(bid) = related_browse_id {
        let rbody = json!({
            "context": {
                "client": {
                    "clientName": "WEB_REMIX",
                    "clientVersion": CLIENT_VERSION,
                    "hl": "es-419",
                    "gl": "MX"
                }
            },
            "browseId": bid
        });
        if let Ok(rval) = post_innertube(http, "browse", rbody, session).await {
            let mut rf = HomeFeed::default();
            if let Some(contents) = rval
                .pointer("/contents/sectionListRenderer/contents")
                .and_then(|c| c.as_array())
            {
                for item in contents {
                    parse_shelf_item(item, &mut rf);
                }
            }
            for t in rf.quick_picks.into_iter().chain(rf.recommended_tracks.into_iter()) {
                if !tracks.iter().any(|existing| existing.id == t.id) {
                    tracks.push(t);
                }
            }
        }
    }

    Ok(tracks)
}

pub fn parse_lrc_lines(lrc: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw_line in lrc.lines() {
        let trimmed = raw_line.trim();
        let Some(rest) = trimmed.strip_prefix('[') else {
            continue;
        };
        let Some((ts_str, text)) = rest.split_once(']') else {
            continue;
        };
        let Some((min_str, sec_str)) = ts_str.split_once(':') else {
            continue;
        };
        let Ok(mins) = min_str.trim().parse::<f64>() else {
            continue;
        };
        let Ok(secs) = sec_str.trim().parse::<f64>() else {
            continue;
        };
        let lyric_text = text.trim().to_string();
        if !lyric_text.is_empty() {
            lines.push(LyricLine {
                time_secs: mins * 60.0 + secs,
                text: lyric_text,
            });
        }
    }
    lines.sort_by(|a, b| a.time_secs.total_cmp(&b.time_secs));
    lines
}

fn strip_bracketed_segments(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut round_depth = 0usize;
    let mut square_depth = 0usize;
    for ch in raw.chars() {
        match ch {
            '(' => round_depth += 1,
            ')' => {
                round_depth = round_depth.saturating_sub(1);
                out.push(' ');
            }
            '[' => square_depth += 1,
            ']' => {
                square_depth = square_depth.saturating_sub(1);
                out.push(' ');
            }
            _ if round_depth == 0 && square_depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

pub fn clean_lyrics_title(raw: &str) -> String {
    let mut base = strip_bracketed_segments(raw);
    for sep in [" - ", " | "] {
        if let Some(pos) = base.find(sep) {
            let tail_low = base[pos + sep.len()..].to_lowercase();
            let is_noise = [
                "official",
                "oficial",
                "video",
                "audio",
                "lyric",
                "letra",
                "remaster",
                "hd",
                "4k",
                "live",
                "en vivo",
                "visualizer",
            ]
            .iter()
            .any(|tok| tail_low.contains(tok));
            if is_noise {
                base.truncate(pos);
            }
        }
    }
    let low = base.to_lowercase();
    let feat_pos = [" feat.", " feat ", " ft.", " ft "]
        .iter()
        .filter_map(|pat| low.find(pat))
        .min();
    if let Some(pos) = feat_pos {
        base.truncate(pos);
    }
    let collapsed = base.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .trim_matches(|c: char| c.is_whitespace() || c == '-' || c == '|')
        .to_string()
}

pub fn clean_lyrics_artist(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    let low = s.to_lowercase();
    if let Some(pos) = low.find(" - topic") {
        s.truncate(pos);
    }
    let low2 = s.to_lowercase();
    if low2.ends_with("vevo") && s.len() > 4 {
        let trimmed = s[..s.len() - 4].trim_end();
        if !trimmed.is_empty() {
            s = trimmed.to_string();
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn primary_lyrics_artist(raw: &str) -> String {
    let cleaned = clean_lyrics_artist(raw);
    let low = cleaned.to_lowercase();
    let delims = [",", " & ", " x ", " feat.", " feat ", " ft.", " ft ", " / "];
    let earliest = delims
        .iter()
        .filter_map(|d| low.find(d))
        .filter(|&p| p > 0)
        .min();
    match earliest {
        Some(pos) => cleaned[..pos].trim().to_string(),
        None => cleaned,
    }
}

pub fn pick_best_lrclib_entry(entries: &[Value], target_dur_secs: Option<u64>) -> Option<Value> {
    let mut best: Option<(i32, Value)> = None;
    for item in entries {
        let synced_raw = item
            .get("syncedLyrics")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let plain_raw = item
            .get("plainLyrics")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let has_synced = !parse_lrc_lines(synced_raw).is_empty();
        let has_plain = !plain_raw.trim().is_empty();
        if !has_synced && !has_plain {
            continue;
        }
        let mut score = if has_synced { 100 } else { 20 };
        if let (Some(target), Some(dur)) =
            (target_dur_secs, item.get("duration").and_then(|v| v.as_f64()))
        {
            let diff = (dur - target as f64).abs();
            if diff <= 3.0 {
                score += 30;
            } else if diff <= 10.0 {
                score += 15;
            } else if diff > 30.0 {
                score -= 30;
            }
        }
        let replace = match &best {
            Some((best_score, _)) => score > *best_score,
            None => true,
        };
        if replace {
            best = Some((score, item.clone()));
        }
    }
    best.map(|(_, v)| v)
}

fn extract_lrclib_data(val: &Value) -> Option<LyricsData> {
    let synced_raw = val.get("syncedLyrics").and_then(|v| v.as_str()).unwrap_or("");
    let plain_raw = val.get("plainLyrics").and_then(|v| v.as_str()).unwrap_or("");
    let synced = parse_lrc_lines(synced_raw);
    let plain = if !plain_raw.trim().is_empty() {
        plain_raw.to_string()
    } else if !synced.is_empty() {
        synced
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        return None;
    };

    Some(LyricsData {
        plain,
        synced,
        source: "LRCLIB".to_string(),
    })
}

pub async fn fetch_lrclib_lyrics(http: &reqwest::Client, track: &Track) -> Option<LyricsData> {
    let clean_title = clean_lyrics_title(&track.title);
    let clean_artist = clean_lyrics_artist(&track.artist);
    let primary_artist = primary_lyrics_artist(&track.artist);
    if clean_title.is_empty() {
        return None;
    }

    if !clean_artist.is_empty() {
        let mut req = http
            .get("https://lrclib.net/api/get")
            .timeout(std::time::Duration::from_secs(4))
            .header(USER_AGENT, "YR-Music-Rust/0.1 (https://github.com/uamo11/YR)")
            .query(&[
                ("track_name", clean_title.as_str()),
                ("artist_name", clean_artist.as_str()),
            ]);
        if let Some(dur) = track.duration_secs {
            req = req.query(&[("duration", dur)]);
        }
        if let Ok(r) = req.send().await {
            if r.status().is_success() {
                if let Ok(val) = r.json::<Value>().await {
                    if let Some(data) = extract_lrclib_data(&val) {
                        return Some(data);
                    }
                }
            }
        }
    }

    if !primary_artist.is_empty() {
        if let Ok(sresp) = http
            .get("https://lrclib.net/api/search")
            .timeout(std::time::Duration::from_secs(4))
            .header(USER_AGENT, "YR-Music-Rust/0.1 (https://github.com/uamo11/YR)")
            .query(&[
                ("track_name", clean_title.as_str()),
                ("artist_name", primary_artist.as_str()),
            ])
            .send()
            .await
        {
            if sresp.status().is_success() {
                if let Ok(arr) = sresp.json::<Vec<Value>>().await {
                    if let Some(best) = pick_best_lrclib_entry(&arr, track.duration_secs) {
                        if let Some(data) = extract_lrclib_data(&best) {
                            return Some(data);
                        }
                    }
                }
            }
        }
    }

    let q = if primary_artist.is_empty() {
        clean_title.clone()
    } else {
        format!("{clean_title} {primary_artist}")
    };
    let sresp = http
        .get("https://lrclib.net/api/search")
        .timeout(std::time::Duration::from_secs(4))
        .header(USER_AGENT, "YR-Music-Rust/0.1 (https://github.com/uamo11/YR)")
        .query(&[("q", q.as_str())])
        .send()
        .await
        .ok()?;
    if !sresp.status().is_success() {
        return None;
    }
    let arr: Vec<Value> = sresp.json().await.ok()?;
    let best = pick_best_lrclib_entry(&arr, track.duration_secs)?;
    extract_lrclib_data(&best)
}

pub async fn fetch_sponsor_segments(
    http: &reqwest::Client,
    video_id: &str,
) -> Vec<SponsorSegment> {
    let url = format!(
        "https://sponsor.ajay.app/api/skipSegments?videoID={video_id}&categories=[\"sponsor\",\"selfpromo\",\"interaction\",\"intro\",\"outro\",\"preview\",\"music_offtopic\"]"
    );
    let Ok(resp) = http
        .get(url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
    else {
        return Vec::new();
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let Ok(arr) = resp.json::<Vec<Value>>().await else {
        return Vec::new();
    };
    arr.into_iter()
        .filter_map(|item| {
            let seg = item.get("segment")?.as_array()?;
            let start = seg.first()?.as_f64()?;
            let end = seg.get(1)?.as_f64()?;
            let cat = item
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("sponsor")
                .to_string();
            if end > start + 0.5 {
                Some(SponsorSegment {
                    start_secs: start,
                    end_secs: end,
                    category: cat,
                })
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sapisidhash_computes_deterministic_header() {
        let cookie = "PREF=f6=400; SAPISID=test_sapisid_123; __Secure-3PAPISID=test_3p_456; SID=abc";
        let hdr = build_sapisidhash_header(cookie, 1_700_000_000).expect("header");
        assert!(hdr.starts_with("SAPISIDHASH 1700000000_"));

        let cookie_3p = "PREF=f6=400; __Secure-3PAPISID=test_3p_456; SID=abc";
        let hdr_3p = build_sapisidhash_header(cookie_3p, 1_700_000_000).expect("header");
        assert!(hdr_3p.starts_with("SAPISIDHASH 1700000000_"));
    }

    #[tokio::test]
    async fn live_innertube_home_parses_shelves_and_moods() {
        let http = reqwest::Client::new();
        match fetch_home_feed(&http, None, None).await {
            Ok(feed) => {
                assert!(!feed.moods.is_empty(), "expected Spanish mood chips");
                assert!(!feed.shelves.is_empty(), "expected YouTube Music shelves");
                assert!(
                    !feed.quick_picks.is_empty()
                        || !feed.recommended_playlists.is_empty()
                        || !feed.new_albums.is_empty(),
                    "expected home music items"
                );
            }
            Err(e) => {
                eprintln!("Live home feed skipped in CI/datacenter IP: {e}");
            }
        }
    }

    #[test]
    fn parse_duration_text_handles_short_long_and_subtitles() {
        assert_eq!(parse_duration_text("3:29"), Some(209));
        assert_eq!(parse_duration_text("0:05"), Some(5));
        assert_eq!(parse_duration_text("1:02:05"), Some(3725));
        assert_eq!(
            parse_duration_text("Mon Laferte • 4:38"),
            Some(278)
        );
        assert_eq!(parse_duration_text("2.8 M de vistas"), None);
        assert_eq!(parse_duration_text(""), None);
    }

    #[test]
    fn parse_lrc_lines_extracts_sorted_timestamps() {
        let lrc = "[00:12.50] Primera línea\n[00:05.00] Intro línea\n[bad] ignorar\n";
        let lines = parse_lrc_lines(lrc);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "Intro línea");
        assert!((lines[0].time_secs - 5.0).abs() < 0.01);
        assert_eq!(lines[1].text, "Primera línea");
        assert!((lines[1].time_secs - 12.5).abs() < 0.01);
    }

    #[test]
    fn pick_best_video_stream_respects_height_cap_and_attaches_hls() {
        let sample = json!({
            "streamingData": {
                "hlsManifestUrl": "https://manifest.googlevideo.com/test.m3u8",
                "adaptiveFormats": [
                    {
                        "itag": 247,
                        "mimeType": "video/webm; codecs=\"vp9\"",
                        "width": 1280,
                        "height": 720,
                        "fps": 30,
                        "url": "https://rr1.googlevideo.com/vp9_720"
                    },
                    {
                        "itag": 135,
                        "mimeType": "video/mp4; codecs=\"avc1.4D401E\"",
                        "width": 854,
                        "height": 480,
                        "fps": 30,
                        "url": "https://rr1.googlevideo.com/avc_480"
                    },
                    {
                        "itag": 136,
                        "mimeType": "video/mp4; codecs=\"avc1.4D401F\"",
                        "width": 1280,
                        "height": 720,
                        "fps": 30,
                        "url": "https://rr1.googlevideo.com/avc_720"
                    }
                ]
            }
        });
        let s480 = pick_best_video_stream("vid1", &sample, 480, VISIONOS_UA).expect("480p video");
        assert_eq!(s480.height, 480);
        assert_eq!(s480.url, "https://rr1.googlevideo.com/avc_480");
        assert_eq!(
            s480.fallback_url.as_deref(),
            Some("https://manifest.googlevideo.com/test.m3u8")
        );
        assert!(!s480.is_hls);

        let s720 = pick_best_video_stream("vid1", &sample, 720, VISIONOS_UA).expect("720p video");
        assert_eq!(s720.height, 720);
        assert_eq!(s720.url, "https://rr1.googlevideo.com/avc_720");

        let s_auto = pick_best_video_stream("vid1", &sample, 0, VISIONOS_UA).expect("Auto caps at 720p");
        assert_eq!(s_auto.height, 720);
        assert_eq!(s_auto.url, "https://rr1.googlevideo.com/avc_720");

        assert_eq!(
            video_stream_user_agent("https://rr1.googlevideo.com/videoplayback?c=IOS"),
            IOS_PLAYER_UA
        );
        assert_eq!(
            video_stream_user_agent("https://rr1.googlevideo.com/videoplayback?c=ANDROID_VR"),
            ANDROID_VR_UA
        );
        assert_eq!(
            video_stream_user_agent("https://rr1.googlevideo.com/videoplayback?c=ANDROID"),
            ANDROID_PLAYER_UA
        );
        assert!(video_stream_user_agent("https://rr1.googlevideo.com/videoplayback?c=MWEB").contains("Pixel 8"));

        let vp9_only = json!({
            "streamingData": {
                "adaptiveFormats": [
                    {
                        "itag": 247,
                        "mimeType": "video/webm; codecs=\"vp9\"",
                        "width": 1280,
                        "height": 720,
                        "fps": 30,
                        "url": "https://rr1.googlevideo.com/vp9_720"
                    }
                ]
            }
        });
        let vp9_pick = pick_best_video_stream("vid2", &vp9_only, 0, IOS_PLAYER_UA).expect("VP9 WebM fallback");
        assert_eq!(vp9_pick.height, 720);
        assert_eq!(vp9_pick.url, "https://rr1.googlevideo.com/vp9_720");
    }

    #[test]
    fn pick_best_audio_stream_rejects_webm_and_prefers_m4a() {
        let webm_only = json!({
            "streamingData": {
                "adaptiveFormats": [
                    {
                        "itag": 251,
                        "mimeType": "audio/webm; codecs=\"opus\"",
                        "bitrate": 160000,
                        "url": "https://rr1.googlevideo.com/opus_only"
                    }
                ]
            }
        });
        assert!(pick_best_audio_stream("vid_webm", &webm_only, AudioQuality::High).is_none());

        let mixed = json!({
            "streamingData": {
                "adaptiveFormats": [
                    {
                        "itag": 251,
                        "mimeType": "audio/webm; codecs=\"opus\"",
                        "bitrate": 160000,
                        "url": "https://rr1.googlevideo.com/opus"
                    },
                    {
                        "itag": 140,
                        "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
                        "bitrate": 129000,
                        "contentLength": "3456789",
                        "url": "https://rr1.googlevideo.com/m4a"
                    }
                ]
            }
        });
        let best = pick_best_audio_stream("vid_m4a", &mixed, AudioQuality::High).expect("m4a stream");
        assert_eq!(best.extension, "m4a");
        assert_eq!(best.url, "https://rr1.googlevideo.com/m4a");
    }

    #[tokio::test]
    async fn live_fast_player_and_explore_and_lrclib_work() {
        let http = reqwest::Client::new();
        // Fast stream resolution via ANDROID_VR/IOS (Rick Astley / common music track)
        match fetch_player_stream(&http, "dQw4w9WgXcQ", AudioQuality::High).await {
            Ok(stream) => {
                assert!(stream.url.contains("googlevideo.com"));
            }
            Err(e) => {
                eprintln!("Live stream resolution skipped in CI/datacenter IP: {e}");
            }
        }
        match fetch_explore_feed(&http, None).await {
            Ok(explore) => {
                assert!(
                    !explore.new_albums.is_empty() || !explore.moods.is_empty(),
                    "expected explore sections"
                );
            }
            Err(e) => {
                eprintln!("Explore feed skipped in CI/datacenter IP: {e}");
            }
        }
    }

    #[test]
    fn clean_lyrics_title_and_artist_strip_noise() {
        assert_eq!(
            clean_lyrics_title("Ella (Video Oficial) [HD] - Official Music Video"),
            "Ella"
        );
        assert_eq!(
            clean_lyrics_title("Desde Mis Ojos feat. Sech (Remix)"),
            "Desde Mis Ojos"
        );
        assert_eq!(
            clean_lyrics_artist("Cali Y El Dandee - Topic"),
            "Cali Y El Dandee"
        );
        assert_eq!(clean_lyrics_artist("CamilaVEVO"), "Camila");
        assert_eq!(
            primary_lyrics_artist("Cali Y El Dandee, Sebastián Yatra & Mau"),
            "Cali Y El Dandee"
        );
    }

    #[test]
    fn pick_best_lrclib_entry_prefers_synced_and_matching_duration() {
        let entries = vec![
            json!({
                "plainLyrics": "Only plain text",
                "syncedLyrics": null,
                "duration": 197.0
            }),
            json!({
                "plainLyrics": "Wrong duration synced",
                "syncedLyrics": "[00:10.00] Far duration line",
                "duration": 290.0
            }),
            json!({
                "plainLyrics": "Right duration synced",
                "syncedLyrics": "[00:12.00] Exact line",
                "duration": 198.0
            }),
            json!({
                "plainLyrics": "   ",
                "syncedLyrics": ""
            }),
        ];
        let best = pick_best_lrclib_entry(&entries, Some(197)).expect("picked best");
        assert_eq!(best["plainLyrics"], "Right duration synced");
    }

    #[test]
    fn extract_artist_from_subtitle_skips_views_types_and_durations() {
        assert_eq!(
            extract_artist_from_subtitle("Beéle • 213 M de vistas"),
            "Beéle"
        );
        assert_eq!(
            extract_artist_from_subtitle("Canción • Chris Lebron • 3:16"),
            "Chris Lebron"
        );
        assert_eq!(
            extract_artist_from_subtitle("Video • Camila • 42 M de vistas"),
            "Camila"
        );
        assert_eq!(
            extract_artist_from_subtitle("Álbum • Bad Bunny • 2025"),
            "Bad Bunny"
        );
    }
}
