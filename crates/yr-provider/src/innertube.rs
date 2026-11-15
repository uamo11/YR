use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, ORIGIN, REFERER, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use yr_core::{
    AlbumSummary, AudioQuality, ExploreFeed, GoogleAccountInfo, HomeFeed, HomeShelf,
    HomeShelfCard, HomeShelfLayout, LyricLine, LyricsData, MoodGenre, PlaylistSummary,
    ShelfCardKind, SponsorSegment, StreamInfo, Track,
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
    let mut parts = Vec::new();
    for (cookie_key, scheme) in [
        ("SAPISID", "SAPISIDHASH"),
        ("__Secure-3PAPISID", "SAPISID3PHASH"),
        ("__Secure-1PAPISID", "SAPISID1PHASH"),
    ] {
        if let Some(val) = cookie_value(cookie, cookie_key) {
            let mut hasher = Sha1::new();
            hasher.update(format!("{timestamp_secs} {val} {YTM_ORIGIN}"));
            let digest: String = hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            parts.push(format!("{scheme} {timestamp_secs}_{digest}"));
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
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
    let header = val
        .pointer("/actions/0/openPopupAction/popup/multiPageMenuRenderer/header/activeAccountHeaderRenderer");
    let Some(header) = header else {
        return Ok(None);
    };
    let name = runs_text(header.get("accountName"));
    if name.is_empty() {
        return Ok(None);
    }
    let handle = non_empty(runs_text(header.get("channelHandle")));
    let avatar_url = best_thumb(header.pointer("/accountPhoto/thumbnails"));
    Ok(Some(GoogleAccountInfo {
        name,
        handle,
        avatar_url,
    }))
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
            let Ok(cresp) = post_innertube(http, "browse", cbody, session).await else {
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

    let thumbnail_url = best_thumb(r.pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails"));

    let track = Track {
        id: video_id,
        title,
        artist: artist.clone(),
        artist_id,
        album,
        album_id,
        duration_secs: None,
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
        let artist = subtitle
            .split('•')
            .nth(1)
            .or_else(|| subtitle.split('•').next())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "YouTube Music".to_string());
        let track = Track {
            id: vid.to_string(),
            title: title.clone(),
            artist,
            artist_id: None,
            album: None,
            album_id: None,
            duration_secs: None,
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

fn best_thumb(thumbs: Option<&Value>) -> Option<String> {
    let arr = thumbs?.as_array()?;
    arr.last()
        .or_else(|| arr.first())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string())
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Fast direct stream extraction via InnerTube `ANDROID_VR` / `IOS` player clients.
/// Both return direct unencrypted `url` streams without signature/n-sig JS evaluation.
pub async fn fetch_player_stream(
    http: &reqwest::Client,
    video_id: &str,
    quality: AudioQuality,
) -> Result<StreamInfo, String> {
    let clients = [
        (
            "ANDROID_VR",
            "1.60.19",
            "com.google.android.apps.youtube.vr.oculus/1.60.19 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip",
            "28",
        ),
        (
            "IOS",
            "19.29.1",
            "com.google.ios.youtube/19.29.1 (iPhone16,2; U; CPU iOS 17_5_1 like Mac OS X;)",
            "5",
        ),
    ];

    let target_bps = quality.target_bitrate_kbps() * 1000;
    let mut last_err = String::from("No stream returned");

    for (client_name, client_version, ua, client_id) in clients {
        let body = json!({
            "context": {
                "client": {
                    "clientName": client_name,
                    "clientVersion": client_version,
                    "hl": "es-419",
                    "gl": "MX",
                    "androidSdkVersion": 32,
                    "osName": if client_name == "IOS" { "iOS" } else { "Android" },
                    "osVersion": if client_name == "IOS" { "17.5.1.21F90" } else { "12" }
                }
            },
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true
        });
        let resp = match http
            .post("https://www.youtube.com/youtubei/v1/player?prettyPrint=false")
            .header(CONTENT_TYPE, "application/json")
            .header(USER_AGENT, ua)
            .header("X-YouTube-Client-Name", client_id)
            .header("X-YouTube-Client-Version", client_version)
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
            last_err = format!("{client_name} HTTP {}", resp.status());
            continue;
        }
        let Ok(val) = resp.json::<Value>().await else {
            last_err = format!("{client_name} invalid JSON");
            continue;
        };
        let status = val
            .pointer("/playabilityStatus/status")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if status != "OK" {
            last_err = val
                .pointer("/playabilityStatus/reason")
                .and_then(|v| v.as_str())
                .unwrap_or(status)
                .to_string();
            continue;
        }
        let Some(formats) = val
            .pointer("/streamingData/adaptiveFormats")
            .and_then(|v| v.as_array())
        else {
            continue;
        };

        let mut candidates: Vec<(u64, StreamInfo)> = Vec::new();
        for f in formats {
            let Some(mime) = f.get("mimeType").and_then(|v| v.as_str()) else {
                continue;
            };
            if !mime.starts_with("audio/") {
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
            let is_m4a = mime.contains("mp4") || mime.contains("m4a");
            let ext = if is_m4a {
                "m4a"
            } else if mime.contains("webm") {
                "webm"
            } else {
                "audio"
            };
            // Strongly prefer M4A/AAC since yr-audio decodes isomp4+aac natively.
            let penalty = if is_m4a { 0_u64 } else { 50_000_000_u64 };
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

        if let Some((_, best)) = candidates.into_iter().min_by_key(|(score, _)| *score) {
            return Ok(best);
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
            if let Some(grid) = item.get("gridRenderer").and_then(|g| g.get("items")).and_then(|a| a.as_array()) {
                for cell in grid {
                    if let Some(nav) = cell.get("musicNavigationButtonRenderer") {
                        let title = runs_text(nav.get("buttonText"));
                        let browse_id = nav
                            .pointer("/clickCommand/browseEndpoint/browseId")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let params = nav
                            .pointer("/clickCommand/browseEndpoint/params")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        if !title.is_empty() {
                            if let Some(id) = browse_id {
                                tmp_feed.moods.push(MoodGenre { id, title, params });
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
    Ok(ExploreFeed {
        new_albums: tmp_feed.new_albums,
        trending_tracks: trending,
        moods: tmp_feed.moods,
    })
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
                                duration_secs: None,
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

pub async fn fetch_lrclib_lyrics(http: &reqwest::Client, track: &Track) -> Option<LyricsData> {
    let clean_title = track
        .title
        .split('(')
        .next()
        .unwrap_or(&track.title)
        .split('[')
        .next()
        .unwrap_or(&track.title)
        .trim();
    let mut req = http
        .get("https://lrclib.net/api/get")
        .timeout(std::time::Duration::from_secs(4))
        .header(USER_AGENT, "YR-Music-Rust/0.1 (https://github.com/uamo11/YR)")
        .query(&[("track_name", clean_title), ("artist_name", track.artist.as_str())]);
    if let Some(dur) = track.duration_secs {
        req = req.query(&[("duration", dur)]);
    }
    let val: Value = match req.send().await {
        Ok(r) if r.status().is_success() => r.json().await.ok()?,
        _ => {
            let q = format!("{clean_title} {}", track.artist);
            let sresp = http
                .get("https://lrclib.net/api/search")
                .timeout(std::time::Duration::from_secs(4))
                .header(USER_AGENT, "YR-Music-Rust/0.1")
                .query(&[("q", q.as_str())])
                .send()
                .await
                .ok()?;
            if !sresp.status().is_success() {
                return None;
            }
            let arr: Vec<Value> = sresp.json().await.ok()?;
            arr.into_iter().next()?
        }
    };

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
        assert!(hdr.contains("SAPISIDHASH 1700000000_"));
        assert!(hdr.contains("SAPISID3PHASH 1700000000_"));
    }

    #[tokio::test]
    async fn live_innertube_home_parses_shelves_and_moods() {
        let http = reqwest::Client::new();
        let feed = fetch_home_feed(&http, None, None)
            .await
            .expect("FEmusic_home fetch");
        assert!(!feed.moods.is_empty(), "expected Spanish mood chips");
        assert!(!feed.shelves.is_empty(), "expected YouTube Music shelves");
        assert!(!feed.quick_picks.is_empty(), "expected quick_picks");
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

    #[tokio::test]
    async fn live_fast_player_and_explore_and_lrclib_work() {
        let http = reqwest::Client::new();
        // Fast stream resolution via ANDROID_VR/IOS (Rick Astley / common music track)
        let stream = fetch_player_stream(&http, "dQw4w9WgXcQ", AudioQuality::High)
            .await
            .expect("fast player stream");
        assert!(stream.url.contains("googlevideo.com"));
        let explore = fetch_explore_feed(&http, None)
            .await
            .expect("explore feed");
        assert!(
            !explore.new_albums.is_empty() || !explore.moods.is_empty(),
            "expected explore sections"
        );
    }
}
