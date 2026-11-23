// Now playing from Windows' media sessions (GSMTC): every player that shows in the volume flyout, Qistara included.
// WinRT events arrive on thread-pool threads; they update `State` and post WM_MEDIA so the UI thread redraws.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::Foundation::{TypedEventHandler, Uri};
use windows::Graphics::Imaging::*;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::IRandomAccessStreamReference;
use windows::Web::Http::HttpClient;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::*;

use crate::lrc;

pub const WM_MEDIA: u32 = WM_APP + 4;
const COVER_PX: u32 = 96;

#[derive(Clone)]
pub struct Cover {
    pub size: u32,
    /// BGRA, premultiplied, `size`×`size`
    pub pixels: Arc<Vec<u8>>,
    pub accent: [f32; 3],
}

#[derive(Clone, Default)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pos_ms: u64,
    pos_at: Option<Instant>,
    pub playing: bool,
    rate: f64,
    pub can_seek: bool,
    /// Identifies the track; cover and lyrics belong to the track whose key they carry.
    pub key: String,
    pub cover: Option<Cover>,
    cover_key: String,
    pub lyrics: Arc<Vec<lrc::Line>>,
    pub lyrics_key: String,
    /// The Qistara file the lyrics came from (word timing, translation, or a recitation's ayahs); None: LRCLIB.
    qistara: Option<Arc<Handoff>>,
    /// Right-to-left text (a Qur'an recitation)
    pub rtl: bool,
}

/// What Qistara writes to `%LOCALAPPDATA%\com.qistara.app\notch\now-lyrics.json` (its `src/lib/notch.ts`). Seconds.
#[derive(serde::Deserialize)]
struct Handoff {
    v: u32,
    title: String,
    artist: String,
    duration: f64,
    #[serde(default)]
    rtl: bool,
    lines: Vec<HandoffLine>,
}

#[derive(serde::Deserialize)]
struct HandoffLine {
    t: f64,
    text: String,
    tr: Option<String>,
    #[serde(default)]
    words: Vec<HandoffWord>,
}

#[derive(serde::Deserialize)]
struct HandoffWord {
    t: f64,
    text: String,
}

fn handoff_dir() -> Option<PathBuf> {
    let app = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("com.qistara.app");
    app.exists().then(|| app.join("notch"))
}

fn parse_handoff(json: &str) -> Option<Handoff> {
    serde_json::from_str::<Handoff>(json).ok().filter(|h| h.v == 1)
}

/// Qistara's file is about this track: same title and artist, and the same length give or take 2 s.
fn handoff_fits(h: &Handoff, np: &NowPlaying) -> bool {
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    let length_ok = np.duration_ms == 0 || h.duration <= 0.0 || (h.duration * 1000.0 - np.duration_ms as f64).abs() < 2000.0;
    same(&h.title, &np.title) && same(&h.artist, &np.artist) && length_ok && !h.lines.is_empty()
}

fn handoff_lines(h: &Handoff) -> Vec<lrc::Line> {
    let ms = |s: f64| (s.max(0.0) * 1000.0).round() as u64;
    h.lines
        .iter()
        .map(|l| lrc::Line {
            ms: ms(l.t),
            text: l.text.clone(),
            tr: l.tr.clone().filter(|t| !t.trim().is_empty()),
            words: l.words.iter().map(|w| lrc::Word { ms: ms(w.t), text: w.text.clone() }).collect(),
        })
        .collect()
}

impl NowPlaying {
    /// The playback position now, extrapolated from the last update so nothing has to poll.
    pub fn position(&self, now: Instant) -> u64 {
        let mut p = self.pos_ms as f64;
        if let (true, Some(at)) = (self.playing, self.pos_at) {
            p += now.duration_since(at).as_millis() as f64 * self.rate;
        }
        let p = p.max(0.0) as u64;
        if self.duration_ms > 0 { p.min(self.duration_ms) } else { p }
    }

    pub fn lyrics_ready(&self) -> bool {
        self.lyrics_key == self.key
    }
}

#[derive(Default)]
struct State {
    manager: Option<Manager>,
    subs: Vec<(Session, [i64; 3])>,
    current: Option<Session>,
    now: Option<NowPlaying>,
    fetching: String,
    /// The last file Qistara shared
    handoff: Option<Arc<Handoff>>,
}

struct Shared {
    hwnd: isize, // HWND isn't Send; the handle value is
    state: Mutex<State>,
}

impl Shared {
    fn post(&self) {
        unsafe {
            let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_MEDIA, WPARAM(0), LPARAM(0));
        }
    }
}

pub struct Media(Arc<Shared>);

impl Media {
    pub fn start(hwnd: HWND) -> Self {
        let sh = Arc::new(Shared { hwnd: hwnd.0 as isize, state: Mutex::default() });
        let s = sh.clone();
        std::thread::spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(manager) = Manager::RequestAsync().and_then(|op| op.join()) else { return };
            let (a, b) = (s.clone(), s.clone());
            let _ = manager.SessionsChanged(&TypedEventHandler::new(move |_, _| {
                resubscribe(&a);
                refresh(&a);
                Ok(())
            }));
            let _ = manager.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
                refresh(&b);
                Ok(())
            }));
            s.state.lock().unwrap().manager = Some(manager);
            resubscribe(&s);
            refresh(&s);
        });
        watch_qistara(&sh);
        Self(sh)
    }

    pub fn snapshot(&self) -> Option<NowPlaying> {
        self.0.state.lock().unwrap().now.clone()
    }

    fn session(&self) -> Option<Session> {
        self.0.state.lock().unwrap().current.clone()
    }

    // Fire and forget: the player answers with an event, which redraws us.
    pub fn toggle(&self) {
        self.session().map(|s| s.TryTogglePlayPauseAsync());
    }

    pub fn pause(&self) {
        self.session().map(|s| s.TryPauseAsync());
    }

    pub fn next(&self) {
        self.session().map(|s| s.TrySkipNextAsync());
    }

    pub fn previous(&self) {
        self.session().map(|s| s.TrySkipPreviousAsync());
    }

    pub fn seek(&self, ms: u64) {
        self.session().map(|s| s.TryChangePlaybackPositionAsync(ms as i64 * 10_000));
    }
}

/// Reads Qistara's lyrics file now and whenever Qistara rewrites it. Only when Qistara is installed;
/// the watch blocks in the kernel, so it costs nothing between songs.
fn watch_qistara(sh: &Arc<Shared>) {
    let Some(dir) = handoff_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let sh = sh.clone();
    std::thread::spawn(move || unsafe {
        use windows::Win32::Storage::FileSystem::*;
        use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};
        let load = || {
            let h = std::fs::read_to_string(dir.join("now-lyrics.json")).ok().and_then(|t| parse_handoff(&t));
            sh.state.lock().unwrap().handoff = h.map(Arc::new);
            refresh(&sh);
        };
        load();
        let Ok(w) =
            FindFirstChangeNotificationW(&HSTRING::from(dir.as_os_str()), false, FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME)
        else {
            return;
        };
        while WaitForSingleObject(w, INFINITE) == WAIT_OBJECT_0 {
            load();
            if FindNextChangeNotification(w).is_err() {
                break;
            }
        }
    });
}

/// Playing beats paused, Qistara beats other players, Windows' own pick breaks ties.
fn score(playing: bool, qistara: bool, current: bool) -> i32 {
    playing as i32 * 4 + qistara as i32 * 2 + current as i32
}

fn app_id(s: &Session) -> String {
    s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default()
}

fn pick(m: &Manager) -> Option<Session> {
    let current = m.GetCurrentSession().ok().map(|s| app_id(&s));
    let mut best: Option<(i32, Session)> = None;
    for s in m.GetSessions().ok()? {
        let playing = s.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).is_ok_and(|st| st == Status::Playing);
        let id = app_id(&s);
        let sc = score(playing, id.to_lowercase().contains("qistara"), current.as_deref() == Some(id.as_str()));
        if best.as_ref().is_none_or(|(b, _)| sc > *b) {
            best = Some((sc, s));
        }
    }
    best.map(|(_, s)| s)
}

/// Listens to every session, so a player that starts playing can take over the notch.
fn resubscribe(sh: &Arc<Shared>) {
    let (old, manager) = {
        let mut st = sh.state.lock().unwrap();
        (std::mem::take(&mut st.subs), st.manager.clone())
    };
    for (s, [a, b, c]) in old {
        let _ = s.RemoveMediaPropertiesChanged(a);
        let _ = s.RemovePlaybackInfoChanged(b);
        let _ = s.RemoveTimelinePropertiesChanged(c);
    }
    let Some(sessions) = manager.and_then(|m| m.GetSessions().ok()) else { return };
    let mut subs = Vec::new();
    for s in sessions {
        let tokens = (|| {
            Ok::<_, Error>([
                s.MediaPropertiesChanged(&on_change(sh))?,
                s.PlaybackInfoChanged(&on_change(sh))?,
                s.TimelinePropertiesChanged(&on_change(sh))?,
            ])
        })();
        if let Ok(t) = tokens {
            subs.push((s, t));
        }
    }
    sh.state.lock().unwrap().subs = subs;
}

fn on_change<A: RuntimeType + 'static, B: RuntimeType + 'static>(sh: &Arc<Shared>) -> TypedEventHandler<A, B> {
    let sh = sh.clone();
    TypedEventHandler::new(move |_, _| {
        refresh(&sh);
        Ok(())
    })
}

fn refresh(sh: &Arc<Shared>) {
    let Some(manager) = sh.state.lock().unwrap().manager.clone() else { return };
    let best = pick(&manager);
    let read = best.as_ref().and_then(|s| read(s).ok());
    let mut st = sh.state.lock().unwrap();
    st.current = best;
    let Some((mut np, thumb)) = read else {
        st.now = None;
        drop(st);
        sh.post();
        return;
    };
    if let Some(old) = st.now.as_ref().filter(|o| o.key == np.key) {
        (np.cover, np.cover_key) = (old.cover.clone(), old.cover_key.clone());
        (np.lyrics, np.lyrics_key) = (old.lyrics.clone(), old.lyrics_key.clone());
        (np.qistara, np.rtl) = (old.qistara.clone(), old.rtl);
    }
    // Qistara's own lyrics win over LRCLIB while its file is about this track.
    let fits = st.handoff.clone().filter(|h| handoff_fits(h, &np));
    match (fits, &np.qistara) {
        (Some(h), Some(used)) if Arc::ptr_eq(&h, used) => {} // already showing this file
        (Some(h), _) => {
            np.lyrics = Arc::new(handoff_lines(&h));
            (np.lyrics_key, np.rtl, np.qistara) = (np.key.clone(), h.rtl, Some(h));
        }
        (None, Some(_)) => {
            // Qistara moved on (or found nothing); look the track up like any other.
            (np.lyrics, np.rtl, np.qistara) = (Arc::default(), false, None);
            np.lyrics_key.clear();
        }
        (None, None) => {}
    }
    // A thumbnail can show up after the title, so whether one exists is part of what the cover was made from.
    let cover_key = format!("{}#{}", np.key, thumb.is_some());
    let need = np.cover_key != cover_key || np.lyrics_key != np.key;
    let job = format!("{cover_key}|{}", np.duration_ms > 0);
    let fetch = need && st.fetching != job;
    if fetch {
        st.fetching = job;
    }
    st.now = Some(np.clone());
    drop(st);
    sh.post();
    if fetch {
        let sh = sh.clone();
        // WinRT objects cross threads through an agile reference.
        let thumb = thumb.and_then(|t| AgileReference::new(&t).ok());
        std::thread::spawn(move || {
            let cover =
                if np.cover_key != cover_key { thumb.and_then(|t| t.resolve().ok()).and_then(|t| decode_cover(&t).ok()) } else { np.cover.clone() };
            let lyrics = if np.lyrics_key != np.key { find_lyrics(&np) } else { Some(np.lyrics.to_vec()) };
            let mut st = sh.state.lock().unwrap();
            st.fetching.clear();
            if let Some(now) = st.now.as_mut().filter(|n| n.key == np.key) {
                (now.cover, now.cover_key) = (cover, cover_key);
                // Qistara's file may have arrived while LRCLIB was being asked; it wins.
                if let Some(l) = lyrics.filter(|_| now.qistara.is_none()) {
                    (now.lyrics, now.lyrics_key) = (Arc::new(l), np.key.clone());
                }
            }
            drop(st);
            sh.post();
        });
    }
}

fn read(s: &Session) -> Result<(NowPlaying, Option<IRandomAccessStreamReference>)> {
    let props = s.TryGetMediaPropertiesAsync()?.join()?;
    let info = s.GetPlaybackInfo()?;
    let tl = s.GetTimelineProperties()?;
    let playing = info.PlaybackStatus()? == Status::Playing;
    let rate = info.PlaybackRate().and_then(|r| r.Value()).unwrap_or(1.0);
    let ticks_ms = |t: i64| (t / 10_000).max(0) as u64;
    let duration_ms = ticks_ms(tl.EndTime()?.Duration - tl.StartTime()?.Duration);
    // Position is as of LastUpdatedTime; carry it forward to now.
    let mut pos_ms = ticks_ms(tl.Position()?.Duration);
    let updated = tl.LastUpdatedTime()?.UniversalTime;
    let ft = unsafe { GetSystemTimePreciseAsFileTime() };
    let now_ft = ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64;
    let since = now_ft - updated;
    if playing && updated > 0 && (0..86_400 * 10_000_000).contains(&since) {
        pos_ms += (ticks_ms(since) as f64 * rate) as u64;
    }
    let (app, title, artist, album) = (app_id(s), props.Title()?.to_string(), props.Artist()?.to_string(), props.AlbumTitle()?.to_string());
    let np = NowPlaying {
        key: format!("{app}\n{title}\n{artist}\n{album}"),
        title,
        artist,
        album,
        duration_ms,
        pos_ms,
        pos_at: Some(Instant::now()),
        playing,
        rate,
        can_seek: info.Controls()?.IsPlaybackPositionEnabled()?,
        ..Default::default()
    };
    Ok((np, props.Thumbnail().ok()))
}

/// Decodes the thumbnail to a square, center-cropped BGRA tile (YouTube thumbnails are 16:9).
fn decode_cover(thumb: &IRandomAccessStreamReference) -> Result<Cover> {
    let stream = thumb.OpenReadAsync()?.join()?;
    let dec = BitmapDecoder::CreateAsync(&stream)?.join()?;
    let (w, h) = (dec.PixelWidth()?.max(1), dec.PixelHeight()?.max(1));
    let scale = COVER_PX as f64 / w.min(h) as f64;
    let (sw, sh) = (((w as f64 * scale).round() as u32).max(COVER_PX), ((h as f64 * scale).round() as u32).max(COVER_PX));
    let t = BitmapTransform::new()?;
    t.SetScaledWidth(sw)?;
    t.SetScaledHeight(sh)?;
    t.SetInterpolationMode(BitmapInterpolationMode::Fant)?;
    t.SetBounds(BitmapBounds { X: (sw - COVER_PX) / 2, Y: (sh - COVER_PX) / 2, Width: COVER_PX, Height: COVER_PX })?;
    let px = dec
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &t,
            ExifOrientationMode::RespectExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )?
        .join()?
        .DetachPixelData()?
        .to_vec();
    Ok(Cover { size: COVER_PX, accent: accent(&px), pixels: Arc::new(px) })
}

/// The cover's most vivid colour, brightened enough to read on a dark notch. Grey covers get near-white.
pub fn accent(bgra: &[u8]) -> [f32; 3] {
    let mut best = (0.0f32, [0.92, 0.92, 0.92]);
    for px in bgra.as_chunks::<4>().0.iter().step_by(7) {
        let a = px[3] as f32;
        if a < 200.0 {
            continue;
        }
        let [r, g, b] = [px[2] as f32 / a, px[1] as f32 / a, px[0] as f32 / a];
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        if max < 0.25 {
            continue;
        }
        let score = (max - min) / max * max;
        if score > best.0 {
            best = (score, [r, g, b]);
        }
    }
    if best.0 < 0.15 {
        return [0.92, 0.92, 0.92];
    }
    let [r, g, b] = best.1;
    let lift = 0.85 / r.max(g).max(b);
    [(r * lift).min(1.0), (g * lift).min(1.0), (b * lift).min(1.0)]
}

fn cache_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("notch").join("lyrics"))
}

/// FNV-1a: a stable file name for a song (std's hasher may change between Rust releases).
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// Cached lyrics, else LRCLIB. `None` means the network failed (try again later); an empty list means there are none.
fn find_lyrics(np: &NowPlaying) -> Option<Vec<lrc::Line>> {
    let file = cache_dir().map(|d| d.join(format!("{:016x}.lrc", fnv(&format!("{}\n{}\n{}", np.artist, np.title, np.duration_ms / 1000)))));
    if let Some(text) = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()) {
        return Some(lrc::parse(&text));
    }
    let found = lrclib(np).ok()?;
    if let Some(f) = &file {
        let _ = std::fs::create_dir_all(f.parent().unwrap());
        let _ = std::fs::write(f, found.as_deref().unwrap_or("")); // empty file = LRCLIB has none, don't ask again
    }
    Some(found.map(|t| lrc::parse(&t)).unwrap_or_default())
}

fn lrclib(np: &NowPlaying) -> Result<Option<String>> {
    let esc = |s: &str| Uri::EscapeComponent(&HSTRING::from(s)).map(|h| h.to_string());
    let client = HttpClient::new()?;
    client.DefaultRequestHeaders()?.UserAgent()?.TryParseAdd(h!("notch/0.1.0"))?;
    let get = |url: String| -> Result<Option<String>> {
        let resp = client.GetAsync(&Uri::CreateUri(&HSTRING::from(url))?)?.join()?;
        if resp.StatusCode()?.0 == 404 {
            return Ok(None);
        }
        if !resp.IsSuccessStatusCode()? {
            return Err(Error::from(E_FAIL));
        }
        Ok(Some(resp.Content()?.ReadAsStringAsync()?.join()?.to_string()))
    };
    let (title, artist) = (esc(&np.title)?, esc(&np.artist)?);
    if np.duration_ms > 0 {
        let mut url = format!("https://lrclib.net/api/get?track_name={title}&artist_name={artist}&duration={}", np.duration_ms / 1000);
        if !np.album.is_empty() {
            url += &format!("&album_name={}", esc(&np.album)?);
        }
        if let Some(found) = get(url)?.and_then(|b| synced(&b, np.duration_ms)) {
            return Ok(Some(found));
        }
    }
    let url = format!("https://lrclib.net/api/search?track_name={title}&artist_name={artist}");
    Ok(get(url)?.and_then(|b| synced(&b, np.duration_ms)))
}

/// The synced lyrics from an LRCLIB answer: one record, or a search list (closest length wins).
fn synced(body: &str, duration_ms: u64) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let lyrics = |i: &serde_json::Value| i["syncedLyrics"].as_str().filter(|s| !s.trim().is_empty()).map(String::from);
    match v.as_array() {
        None => lyrics(&v),
        Some(list) => list
            .iter()
            .filter(|i| lyrics(i).is_some())
            .min_by_key(|i| (i["duration"].as_f64().unwrap_or(0.0) * 1000.0 - duration_ms as f64).abs() as u64)
            .and_then(lyrics),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_preference() {
        assert!(score(true, false, false) > score(false, true, true), "anything playing beats a paused Qistara");
        assert!(score(true, true, false) > score(true, false, true), "Qistara wins when both play");
        assert!(score(false, false, true) > score(false, false, false));
    }

    #[test]
    fn accent_picks_vivid_colour() {
        let mut px = Vec::new();
        for i in 0..400 {
            px.extend(if i % 3 == 0 { [20, 30, 220, 255] } else { [90, 90, 90, 255] }); // BGRA: some red, mostly grey
        }
        let [r, g, b] = accent(&px);
        assert!(r > 0.8 && g < 0.2 && b < 0.2, "{r} {g} {b}");
        assert_eq!(accent(&[128, 128, 128, 255].repeat(50)), [0.92, 0.92, 0.92]);
        assert_eq!(accent(&[]), [0.92, 0.92, 0.92]);
    }

    #[test]
    fn lrclib_answers() {
        assert_eq!(synced(r#"{"syncedLyrics":"[00:01.00]hi","plainLyrics":"hi"}"#, 0).as_deref(), Some("[00:01.00]hi"));
        assert_eq!(synced(r#"{"syncedLyrics":null,"plainLyrics":"hi"}"#, 0), None);
        let list = r#"[{"duration":100,"syncedLyrics":"[00:01.00]far"},{"duration":201,"syncedLyrics":"[00:01.00]near"},{"duration":200,"syncedLyrics":null}]"#;
        assert_eq!(synced(list, 200_000).as_deref(), Some("[00:01.00]near"));
        assert_eq!(synced("not json", 0), None);
    }

    #[test]
    fn qistara_handoff() {
        let json = r#"{"v":1,"title":"Paper Sky","artist":"Lumen Coast","duration":180.4,"rtl":false,"lines":[
            {"t":1.5,"end":3,"text":"we fold","tr":"kami melipat","words":[{"t":1.5,"end":2,"text":"we "},{"t":2,"end":3,"text":"fold"}]},
            {"t":3,"end":5,"text":"the light","tr":""}]}"#;
        let h = parse_handoff(json).unwrap();
        let np = |title: &str, ms| NowPlaying { title: title.into(), artist: "lumen coast ".into(), duration_ms: ms, ..Default::default() };
        assert!(handoff_fits(&h, &np("Paper Sky", 181_000)), "case, spaces and a second of length don't matter");
        assert!(handoff_fits(&h, &np("Paper Sky", 0)), "no length from the player");
        assert!(!handoff_fits(&h, &np("Paper Sky", 200_000)), "another recording");
        assert!(!handoff_fits(&h, &np("Other Song", 180_000)));
        let lines = handoff_lines(&h);
        assert_eq!((lines[0].ms, lines[0].tr.as_deref(), lines[0].words.len()), (1500, Some("kami melipat"), 2));
        assert_eq!(lines[0].words[1].ms, 2000);
        assert_eq!(lines[1].tr, None, "an empty translation shows nothing");
        assert!(parse_handoff(r#"{"v":2,"title":"","artist":"","duration":0,"lines":[]}"#).is_none(), "a future format is ignored");
        assert!(parse_handoff("{broken").is_none());
    }

    /// Talks to lrclib.net: `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn lrclib_live() {
        let np = NowPlaying { title: "Bohemian Rhapsody".into(), artist: "Queen".into(), duration_ms: 354_000, ..Default::default() };
        let lines = lrc::parse(&lrclib(&np).unwrap().expect("LRCLIB knows this one"));
        assert!(lines.len() > 20, "{} lines", lines.len());
    }

    #[test]
    fn stable_cache_names() {
        assert_eq!(fnv(""), 0xcbf29ce484222325);
        assert_eq!(fnv("a"), 0xaf63dc4c8601ec8c);
    }

    #[test]
    fn position_runs_forward_only_while_playing() {
        let at = Instant::now();
        let mut np = NowPlaying { pos_ms: 1000, pos_at: Some(at), playing: true, rate: 1.0, duration_ms: 3000, ..Default::default() };
        assert_eq!(np.position(at + std::time::Duration::from_millis(500)), 1500);
        assert_eq!(np.position(at + std::time::Duration::from_secs(60)), 3000, "clamped to the end");
        np.playing = false;
        assert_eq!(np.position(at + std::time::Duration::from_millis(500)), 1000);
    }
}
