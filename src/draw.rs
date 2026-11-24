// Drawing: Direct2D (software) into a premultiplied DIB, shown with UpdateLayeredWindow.
// Software D2D keeps RAM low (no D3D device); frames are drawn only while something moves.
// This file holds the renderer, the shared pieces (buttons, rings, bars, header) and the closed states;
// the open pages are in pages.rs.
use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::config::{Theme, parse_color};
use crate::lrc::Line;
use crate::media::Cover;

/// Something clickable, in box DIPs. Recorded while drawing so hit areas always match what's on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Previous,
    PlayPause,
    Next,
    Seek {
        left: f32,
        right: f32,
    },
    Line(u64),
    LyricsToggle,
    Page(usize),
    FocusToggle,
    FocusSkip,
    FocusReset,
    AddTask,
    /// Index into the app's task list
    Task(usize),
    ClearDone,
    Drink,
    Undrink,
    ShelfItem(usize),
    ShelfRemove(usize),
    ShelfClear,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Page {
    Music,
    Focus,
    Prayer,
    Water,
    Todo,
    Widgets,
    Shelf,
}

// Notch's own accents, one per kind of thing.
pub const CORAL: [f32; 3] = [1.0, 0.541, 0.357];
pub const TEAL: [f32; 3] = [0.36, 0.84, 0.69];
pub const AMBER: [f32; 3] = [1.0, 0.706, 0.329];
pub const AQUA: [f32; 3] = [0.298, 0.788, 0.941];
pub const MINT: [f32; 3] = [0.357, 0.839, 0.627];
pub const NEUTRAL: [f32; 3] = [0.82, 0.82, 0.88];

pub struct Music<'a> {
    pub title: &'a str,
    /// "artist · album"
    pub byline: &'a str,
    pub pos_ms: u64,
    pub duration_ms: u64,
    pub playing: bool,
    pub can_seek: bool,
    pub accent: [f32; 3],
    pub lines: &'a [Line],
    pub line: Option<usize>,
    /// Words of the current line already sung (word-timed lyrics light up word by word)
    pub lit: usize,
    pub lyrics_ready: bool,
    /// The lyrics column is shown (the toggle in the controls row)
    pub lyrics_on: bool,
    /// 1 right after the lyric line changes, springing to 0: the lines slide up into place.
    pub slide: f32,
}

pub struct FocusView<'a> {
    /// "Focus · 2 of 4", "Short break", "Ready to focus"
    pub label: &'a str,
    /// "18:42"
    pub left: &'a str,
    /// "until 22:50", or "" when stopped
    pub until: &'a str,
    /// Share of the phase still to go, 1 → 0
    pub fraction: f32,
    pub running: bool,
    pub on_break: bool,
    /// Focus rounds finished, of `rounds`
    pub done: u32,
    pub rounds: u32,
    /// "Today · 2 h 10 min · 4 sessions"
    pub today: &'a str,
}

pub struct PrayerView<'a> {
    pub names: &'a [String],
    pub times: [String; 5],
    /// Index of the next prayer today, 5 when it's tomorrow's Fajr
    pub next: usize,
    pub next_name: &'a str,
    pub next_time: String,
    /// "in 1 h 8 min"
    pub until: String,
    /// From the last prayer to the next, 0..1
    pub progress: f32,
    /// Where each prayer and now sit on the Fajr..Isha arc, 0..1
    pub marks: [f32; 5],
    pub now: f32,
}

pub struct WaterView {
    /// "1.25"
    pub drunk: String,
    /// "2"
    pub goal: String,
    pub frac: f32,
    pub by_hour: [u32; 24],
    pub hour_now: u32,
    /// "250 ml"
    pub glass: String,
}

pub struct TaskRow {
    /// Index into the app's task list
    pub index: usize,
    pub text: String,
    /// "15:30", or "Tue 09:00" when it isn't today
    pub due: String,
    pub done: bool,
}

pub struct ShelfItem {
    pub path: String,
    pub name: String,
    /// "2.4 MB"
    pub size: String,
}

pub enum Widget<'a> {
    /// (day of month, an open task due, today) for Monday..Sunday, plus a line under it
    Week {
        days: [(u32, bool, bool); 7],
        line: &'a str,
    },
    Battery {
        percent: u32,
        charging: bool,
    },
    Prayer {
        name: &'a str,
        time: &'a str,
        until: &'a str,
    },
    Water {
        drunk: &'a str,
        goal: &'a str,
        frac: f32,
    },
    Focus {
        today: &'a str,
        running: &'a str,
    },
    /// Nothing to show: a short reason
    Empty {
        title: &'a str,
        say: &'a str,
    },
}

/// One slot of the compact closed notch.
pub enum Slot<'a> {
    Cover,
    Ring(f32, [f32; 3]),
    Glyph(&'a str, [f32; 3]),
    /// Little equalizer bars; `hop` 0..1 makes them jump once (a track change)
    Bars([f32; 3], f32),
    Text(&'a str, [f32; 3]),
}

/// What the closed notch shows.
pub enum Closed<'a> {
    Clock(&'a str),
    /// Compact live activity: [left | center | right]
    Live {
        left: Slot<'a>,
        center: &'a str,
        right: Slot<'a>,
    },
    /// The line being sung beside the cover (opt-in: [music] lyrics_in_pill)
    Lyric(&'a str),
    /// The resting eyes: lid 0 open → 1 shut, dozing draws them as lines, bounce lifts them (a wake-up wave)
    Eyes {
        lid: f32,
        dozing: bool,
        bounce: f32,
    },
}

pub struct Toast<'a> {
    pub glyph: &'a str,
    pub title: &'a str,
    /// A second line makes it a two-line pop-up
    pub sub: Option<&'a str>,
    /// Right-hand value ("70%", "15:30")
    pub value: Option<&'a str>,
    /// 0..1: a level bar instead of text (volume)
    pub level: Option<f32>,
    pub tint: [f32; 3],
    /// A progress ring around the icon (battery)
    pub ring: Option<f32>,
}

pub struct Scene<'a> {
    pub w: f32,
    pub h: f32,
    pub radius: f32,
    /// 0 = closed content, 1 = open content (cross-fade and stagger)
    pub open: f32,
    /// Hover "peek" before opening, 0..1: a touch brighter
    pub peek: f32,
    /// Line style with nothing to show: a thin line instead of a pill
    pub line: bool,
    pub pages: &'a [Page],
    /// Which page is showing; fractional while swiping.
    pub page: f32,
    pub hover: Option<Hit>,
    pub pressed: Option<Hit>,
    /// Pressed button scale, 1 → 0.94 and back on a spring
    pub press: f32,
    /// Side arrows' visibility (pointer near that side), 0..1
    pub arrows: (f32, f32),
    /// "Tue 24 Nov"
    pub date: &'a str,
    /// "22:32"
    pub time: &'a str,
    /// A problem to show in the open notch, e.g. a typo in config.toml
    pub note: Option<&'a str>,
    /// The header's mini ring while a focus timer runs
    pub focus_ring: Option<f32>,
    pub playing: bool,
    pub music: Option<Music<'a>>,
    pub focus: FocusView<'a>,
    pub prayer: Option<PrayerView<'a>>,
    pub water: Option<WaterView>,
    pub tasks: &'a [TaskRow],
    pub task_scroll: usize,
    pub widgets: [Widget<'a>; 2],
    pub shelf: &'a [ShelfItem],
    pub shelf_scroll: usize,
    /// Files are being dragged over the notch
    pub drop_hover: bool,
    pub closed: Closed<'a>,
    /// A short-lived pop-up; wins over the closed content
    pub toast: Option<Toast<'a>>,
    /// Pop-up entrance, 0 → 1 (with overshoot)
    pub pop: f32,
}

/// Height of a two-line pop-up, in DIPs.
pub const TOAST_TALL: f32 = 44.0;
/// Width of the volume pop-up, in DIPs.
pub const TOAST_BAR_W: f32 = 230.0;

pub(crate) struct Formats {
    pub clock: IDWriteTextFormat,
    pub activity: IDWriteTextFormat,
    pub title: IDWriteTextFormat,
    pub head_time: IDWriteTextFormat,
    pub head_date: IDWriteTextFormat,
    pub body: IDWriteTextFormat,
    pub body_semi: IDWriteTextFormat,
    pub caption: IDWriteTextFormat,
    pub caption_end: IDWriteTextFormat,
    pub caption_center: IDWriteTextFormat,
    pub label: IDWriteTextFormat,
    pub big: IDWriteTextFormat,
    pub mid: IDWriteTextFormat,
    pub lyric: IDWriteTextFormat,
    pub lyric_side: IDWriteTextFormat,
    pub value: IDWriteTextFormat,
    pub icon: IDWriteTextFormat,
    pub icon_small: IDWriteTextFormat,
    pub icon_big: IDWriteTextFormat,
}

pub struct Renderer {
    /// The window box in DIPs: the open notch plus room for the shoulders and spring overshoot.
    /// Its transparent part lets clicks through.
    pub box_w: f32,
    pub box_h: f32,
    factory: ID2D1Factory,
    round_caps: ID2D1StrokeStyle,
    dw: IDWriteFactory,
    pub(crate) rt: ID2D1DCRenderTarget,
    pub(crate) fill: ID2D1SolidColorBrush,
    pub(crate) ink: ID2D1SolidColorBrush,
    /// The unsung part of a word-timed line
    pub(crate) dim: ID2D1SolidColorBrush,
    pub(crate) accent: ID2D1SolidColorBrush,
    /// Any one-off colour
    pub(crate) paint: ID2D1SolidColorBrush,
    notch: D2D1_COLOR_F,
    glow: RefCell<Option<([u8; 3], ID2D1RadialGradientBrush)>>,
    cover: Option<(String, ID2D1BitmapBrush, f32)>,
    /// Shelf thumbnails by path, made the first time a tile shows (None: the shell had no picture)
    thumbs: RefCell<std::collections::HashMap<String, Option<ID2D1Bitmap>>>,
    pub(crate) f: Formats,
    hits: RefCell<Vec<(D2D_RECT_F, Hit)>>,
    /// The frame being drawn: for buttons (hover, press) and the staggered entrance
    pub(crate) state: Cell<FrameState>,
    mem: HDC,
    dib: HBITMAP,
    /// The DIB's pixels, for saving a frame in tests
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) bits: *mut c_void,
    old: HGDIOBJ,
    pub px_w: i32,
    pub px_h: i32,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct FrameState {
    pub open: f32,
    pub hover: Option<Hit>,
    pub pressed: Option<Hit>,
    pub press: f32,
    /// The translation in force (stagger), restored after a scaled button
    pub dy: f32,
}

pub(crate) fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

pub(crate) fn rgb(c: [f32; 3], a: f32) -> D2D1_COLOR_F {
    color(c[0], c[1], c[2], a)
}

fn theme_color(s: &str) -> D2D1_COLOR_F {
    let [r, g, b, a] = parse_color(s).unwrap_or([0.0, 0.0, 0.0, 1.0]);
    color(r, g, b, a)
}

pub(crate) fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F { left, top, right, bottom }
}

fn inside(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
}

pub(crate) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// The staggered entrance of block `k` (0 header, 1 left column, 2 right column…): opacity and a small rise.
pub fn stagger(open: f32, k: u32) -> (f32, f32) {
    let t = ((open - k as f32 * 0.08) / 0.62).clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - t) * (1.0 - t);
    (eased, (1.0 - eased) * 6.0)
}

/// Signed distance from a point to a rounded box (negative inside).
fn sd_round_box(x: f32, y: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let (qx, qy) = ((x - cx).abs() - hw + r, (y - cy).abs() - hh + r);
    (qx.max(0.0).hypot(qy.max(0.0))) + qx.max(qy).min(0.0) - r
}

/// The tray icon, `size` px square, BGRA with straight alpha: a dark screen with a white rim and the notch at its top.
/// Dark tile plus white rim reads on both light and dark taskbars.
pub fn icon_pixels(size: u32) -> Vec<u8> {
    let s = size as f32;
    let k = s / 16.0;
    let (rim, notch_w, notch_h) = (1.1 * k, 8.0 * k, 3.4 * k);
    let cover = |d: f32| (0.5 - d).clamp(0.0, 1.0);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
            let d = sd_round_box(x, y, s / 2.0, s / 2.0, s / 2.0 - 0.5, s / 2.0 - 0.5, 3.6 * k);
            let (outer, inner) = (cover(d), cover(d + rim));
            // The notch hangs from the rim; its top corners sit under the rim, so only the bottom ones round.
            let top = 0.5 + rim - notch_h / 2.0;
            let n = cover(sd_round_box(x, y, s / 2.0, top + notch_h * 0.75, notch_w / 2.0, notch_h * 0.75, notch_h / 2.0)) * inner;
            // Premultiplied layers: dark inside, white rim, white notch.
            let dark = 0.92 * inner;
            let white = (outer - inner) + n;
            let a = (dark * (1.0 - white) + white).min(1.0);
            let c = if a > 0.0 { (white + dark * (1.0 - white) * 0.12) / a } else { 0.0 };
            let v = (c.min(1.0) * 255.0).round() as u8;
            out.extend([v, v, v, (a * 255.0).round() as u8]);
        }
    }
    out
}

/// An HICON for the notification area at the given DPI.
pub fn tray_icon(dpi: u32) -> Option<HICON> {
    unsafe {
        let size = windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXSMICON, dpi).max(16) as u32;
        let px = icon_pixels(size);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        // The alpha channel does the shaping; the mask only has to exist.
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

/// "3:07", or "1:02:03" past an hour.
pub fn clock_text(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// Shoulder (the concave curve where the notch meets the screen edge) for a notch `h` tall at open-mix `open`.
pub fn shoulder(h: f32, radius: f32, open: f32) -> f32 {
    (5.0 + 5.0 * open).min(h / 2.0).min(radius.max(1.0))
}

impl Renderer {
    pub fn new(dpi: u32, theme: &Theme) -> Result<Self> {
        let font = HSTRING::from(theme.font.as_str());
        let text = theme_color(&theme.text);
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 0.0,
                dpiY: 0.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = factory.CreateDCRenderTarget(&props)?;
            // ClearType needs an opaque background; on a transparent surface it fringes.
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;

            // Windows 11 has Segoe Fluent Icons; Windows 10 has the same glyphs in Segoe MDL2 Assets.
            let mut fonts = None;
            dw.GetSystemFontCollection(&mut fonts, false)?;
            let (mut i, mut fluent) = (0u32, BOOL(0));
            if let Some(c) = &fonts {
                c.FindFamilyName(w!("Segoe Fluent Icons"), &mut i, &mut fluent)?;
            }
            let icons = HSTRING::from(if fluent.as_bool() { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" });
            // Big numbers read best in the Display cut when the theme uses Segoe UI Variable.
            let display = HSTRING::from(theme.font.replace("Variable Text", "Variable Display"));

            let fmt =
                |family: &HSTRING, size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT, wrap: bool| -> Result<IDWriteTextFormat> {
                    let f = dw.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!(""))?;
                    f.SetTextAlignment(align)?;
                    f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                    f.SetWordWrapping(if wrap { DWRITE_WORD_WRAPPING_WRAP } else { DWRITE_WORD_WRAPPING_NO_WRAP })?;
                    if !wrap {
                        let sign = dw.CreateEllipsisTrimmingSign(&f)?;
                        let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                        f.SetTrimming(&trim, &sign)?;
                    }
                    Ok(f)
                };
            let (lead, center, end) = (DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_TRAILING);
            let (bold, semi, normal) = (DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_FONT_WEIGHT_NORMAL);
            let f = Formats {
                clock: fmt(&font, 12.5, semi, center, false)?,
                activity: fmt(&font, 12.5, semi, lead, false)?,
                title: fmt(&font, 14.5, bold, lead, false)?,
                head_time: fmt(&font, 12.5, semi, end, false)?,
                head_date: fmt(&font, 12.0, normal, end, false)?,
                body: fmt(&font, 12.5, normal, lead, false)?,
                body_semi: fmt(&font, 13.5, semi, lead, false)?,
                caption: fmt(&font, 11.0, normal, lead, false)?,
                caption_end: fmt(&font, 11.0, normal, end, false)?,
                caption_center: fmt(&font, 10.5, normal, center, false)?,
                label: fmt(&font, 12.0, semi, center, false)?,
                big: fmt(&display, 32.0, semi, lead, false)?,
                mid: fmt(&display, 21.0, semi, lead, false)?,
                lyric: fmt(&font, 16.0, semi, center, true)?,
                lyric_side: fmt(&font, 12.5, normal, center, false)?,
                value: fmt(&display, 16.0, semi, end, false)?,
                icon: fmt(&icons, 16.0, normal, center, false)?,
                icon_small: fmt(&icons, 10.0, normal, center, false)?,
                icon_big: fmt(&icons, 19.0, normal, center, false)?,
            };
            let caps = D2D1_STROKE_STYLE_PROPERTIES { startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, ..Default::default() };
            let notch = theme_color(&theme.background);
            let mut r = Self {
                // Room for the shoulders on both sides, and for the springs to overshoot.
                box_w: theme.open_width + 48.0,
                box_h: theme.open_height + 30.0,
                round_caps: factory.CreateStrokeStyle(&caps, None)?,
                factory,
                fill: rt.CreateSolidColorBrush(&notch, None)?,
                ink: rt.CreateSolidColorBrush(&text, None)?,
                dim: rt.CreateSolidColorBrush(&text, None)?,
                accent: rt.CreateSolidColorBrush(&text, None)?,
                paint: rt.CreateSolidColorBrush(&text, None)?,
                notch,
                glow: RefCell::default(),
                cover: None,
                thumbs: RefCell::default(),
                dw,
                f,
                hits: RefCell::default(),
                state: Cell::default(),
                rt,
                mem: HDC::default(),
                dib: HBITMAP::default(),
                bits: std::ptr::null_mut(),
                old: HGDIOBJ::default(),
                px_w: 0,
                px_h: 0,
            };
            r.set_dpi(dpi)?;
            Ok(r)
        }
    }

    /// (Re)creates the pixel surface for a DPI. Called at start and on WM_DPICHANGED.
    pub fn set_dpi(&mut self, dpi: u32) -> Result<()> {
        let scale = dpi as f32 / 96.0;
        let (w, h) = ((self.box_w * scale).ceil() as i32, (self.box_h * scale).ceil() as i32);
        unsafe {
            self.free_surface();
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
            self.old = SelectObject(mem, dib.into());
            self.rt.BindDC(mem, &RECT { left: 0, top: 0, right: w, bottom: h })?;
            self.rt.SetDpi(dpi as f32, dpi as f32);
            (self.mem, self.dib, self.bits, self.px_w, self.px_h) = (mem, dib, bits, w, h);
        }
        Ok(())
    }

    fn free_surface(&mut self) {
        if !self.mem.is_invalid() {
            unsafe {
                SelectObject(self.mem, self.old);
                let _ = DeleteObject(self.dib.into());
                let _ = DeleteDC(self.mem);
            }
            self.mem = HDC::default();
        }
    }

    /// Uploads the album art once per track.
    pub fn set_cover(&mut self, key: &str, cover: Option<&Cover>) {
        let Some(c) = cover else {
            self.cover = None;
            return;
        };
        if self.cover.as_ref().is_some_and(|(k, _, _)| k == key) {
            return;
        }
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        let brush = unsafe {
            self.rt
                .CreateBitmap(D2D_SIZE_U { width: c.size, height: c.size }, Some(c.pixels.as_ptr().cast()), c.size * 4, &props)
                .and_then(|b| self.rt.CreateBitmapBrush(&b, None, None))
        };
        self.cover = brush.ok().map(|b| (key.to_string(), b, c.size as f32));
    }

    pub(crate) fn measure_in(&self, f: &IDWriteTextFormat, t: &str) -> f32 {
        let wide: Vec<u16> = t.encode_utf16().collect();
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            self.dw.CreateTextLayout(&wide, f, 2000.0, 50.0).and_then(|l| l.GetMetrics(&mut m)).map_or(0.0, |_| m.widthIncludingTrailingWhitespace)
        }
    }

    /// The open notch's width, which pages are laid out for.
    pub(crate) fn open_w(&self) -> f32 {
        self.box_w - 48.0
    }

    /// Width of closed-pill text, to size the pill around it.
    pub fn measure(&self, t: &str) -> f32 {
        self.measure_in(&self.f.activity, t)
    }

    /// Width of a pop-up's title or second line.
    pub fn measure_toast(&self, title: &str, sub: Option<&str>, value: Option<&str>) -> f32 {
        let lines = self.measure_in(&self.f.activity, title).max(sub.map_or(0.0, |s| self.measure_in(&self.f.caption, s)));
        let value = value.map_or(0.0, |v| self.measure_in(&self.f.value, v) + 14.0);
        lines + value
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits.borrow().iter().rev().find(|(r, _)| inside(r, x, y)).map(|(_, h)| *h)
    }

    pub(crate) fn add_hit(&self, r: D2D_RECT_F, h: Hit, opacity: f32) {
        if opacity > 0.6 {
            let dy = self.state.get().dy;
            self.hits.borrow_mut().push((rect(r.left, r.top + dy, r.right, r.bottom + dy), h));
        }
    }

    pub(crate) fn text(&self, t: &str, f: &IDWriteTextFormat, r: D2D_RECT_F, b: &ID2D1SolidColorBrush, opacity: f32) {
        let wide: Vec<u16> = t.encode_utf16().collect();
        unsafe {
            b.SetOpacity(opacity);
            self.rt.DrawText(&wide, f, &r, b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    /// Text in a one-off colour.
    pub(crate) fn text_in(&self, t: &str, f: &IDWriteTextFormat, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        unsafe { self.paint.SetColor(&c) };
        self.text(t, f, r, &self.paint, 1.0);
    }

    pub(crate) fn round(&self, r: D2D_RECT_F, radius: f32, b: &ID2D1Brush) {
        unsafe { self.rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }, b) }
    }

    pub(crate) fn round_in(&self, r: D2D_RECT_F, radius: f32, c: D2D1_COLOR_F) {
        unsafe {
            self.paint.SetColor(&c);
            self.paint.SetOpacity(1.0);
        }
        self.round(r, radius, &self.paint);
    }

    pub(crate) fn dot(&self, cx: f32, cy: f32, r: f32, c: D2D1_COLOR_F) {
        unsafe {
            self.paint.SetColor(&c);
            self.paint.SetOpacity(1.0);
            self.rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r }, &self.paint);
        }
    }

    /// A soft card behind a group of things.
    pub(crate) fn card(&self, r: D2D_RECT_F, o: f32) {
        self.round_in(r, 12.0, color(0.09, 0.09, 0.10, o));
    }

    /// Small grey text.
    pub(crate) fn caption(&self, t: &str, r: D2D_RECT_F, o: f32) {
        self.text(t, &self.f.caption, r, &self.ink, 0.5 * o);
    }

    pub(crate) fn progress(&self, x0: f32, x1: f32, y: f32, frac: f32, accent: [f32; 3], o: f32) {
        self.round_in(rect(x0, y - 2.0, x1, y + 2.0), 2.0, color(1.0, 1.0, 1.0, 0.12 * o));
        let end = x0 + (x1 - x0) * frac.clamp(0.0, 1.0);
        if end > x0 + 0.5 {
            self.round_in(rect(x0, y - 2.0, end.max(x0 + 4.0), y + 2.0), 2.0, rgb(accent, o));
        }
    }

    /// A ring that empties clockwise from 12 o'clock.
    pub(crate) fn ring(&self, cx: f32, cy: f32, r: f32, stroke: f32, frac: f32, accent: [f32; 3], o: f32) {
        unsafe {
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
            self.paint.SetColor(&color(1.0, 1.0, 1.0, 0.14 * o));
            self.paint.SetOpacity(1.0);
            self.rt.DrawEllipse(&e, &self.paint, stroke, None);
            self.accent.SetColor(&rgb(accent, 1.0));
            self.accent.SetOpacity(o);
            if frac >= 0.999 {
                self.rt.DrawEllipse(&e, &self.accent, stroke, None);
                return;
            }
            if frac < 0.002 {
                return;
            }
            let a = frac * std::f32::consts::TAU;
            let Ok(geo) = self.factory.CreatePathGeometry() else { return };
            let Ok(sink) = geo.Open() else { return };
            sink.BeginFigure(Vector2 { X: cx, Y: cy - r }, D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: Vector2 { X: cx + r * a.sin(), Y: cy - r * a.cos() },
                size: D2D_SIZE_F { width: r, height: r },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if frac > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            if sink.Close().is_ok() {
                self.rt.DrawGeometry(&geo, &self.accent, stroke, &self.round_caps);
            }
        }
    }

    /// An open, smooth curve through points (a polyline with round joins is plenty at this size).
    pub(crate) fn curve(&self, pts: &[(f32, f32)], stroke: f32, c: D2D1_COLOR_F) {
        if pts.len() < 2 {
            return;
        }
        unsafe {
            let Ok(geo) = self.factory.CreatePathGeometry() else { return };
            let Ok(sink) = geo.Open() else { return };
            sink.BeginFigure(Vector2 { X: pts[0].0, Y: pts[0].1 }, D2D1_FIGURE_BEGIN_HOLLOW);
            for p in &pts[1..] {
                sink.AddLine(Vector2 { X: p.0, Y: p.1 });
            }
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            if sink.Close().is_ok() {
                self.paint.SetColor(&c);
                self.paint.SetOpacity(1.0);
                self.rt.DrawGeometry(&geo, &self.paint, stroke, &self.round_caps);
            }
        }
    }

    /// Runs `draw` scaled around (cx, cy), on top of the current stagger translation.
    fn scaled(&self, cx: f32, cy: f32, s: f32, draw: impl FnOnce()) {
        let dy = self.state.get().dy;
        unsafe {
            self.rt.SetTransform(&Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: cx - s * cx, M32: cy - s * cy + dy });
        }
        draw();
        unsafe { self.rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: dy }) };
    }

    /// Hover highlight and press scale for a button at `r`.
    fn button_state(&self, hit: Hit) -> (bool, f32) {
        let st = self.state.get();
        let hovered = st.hover == Some(hit);
        let scale = if st.pressed == Some(hit) { st.press } else { 1.0 };
        (hovered, scale)
    }

    /// A round icon button. `filled`: the main action (white disc, dark glyph).
    pub(crate) fn icon_button(&self, cx: f32, cy: f32, r: f32, glyph: &str, hit: Hit, filled: bool, o: f32) {
        let (hovered, scale) = self.button_state(hit);
        let area = rect(cx - r, cy - r, cx + r, cy + r);
        self.scaled(cx, cy, scale, || {
            if filled {
                self.dot(cx, cy, r, color(1.0, 1.0, 1.0, o));
                self.text(glyph, &self.f.icon, area, &self.fill, o);
            } else {
                if hovered {
                    self.dot(cx, cy, r, color(1.0, 1.0, 1.0, 0.10 * o));
                }
                self.text(glyph, &self.f.icon, area, &self.ink, 0.9 * o);
            }
        });
        self.add_hit(area, hit, o);
    }

    /// A pill button with an icon and a label; returns its width. `tint`: an accent-coloured primary button.
    pub(crate) fn pill_button(&self, x: f32, y: f32, glyph: &str, label: &str, hit: Hit, tint: Option<[f32; 3]>, o: f32) -> f32 {
        let tw = self.measure_in(&self.f.label, label);
        let w = 12.0 + 14.0 + 6.0 + tw + 12.0;
        let r = rect(x, y, x + w, y + 26.0);
        let (hovered, scale) = self.button_state(hit);
        self.scaled(x + w / 2.0, y + 13.0, scale, || {
            let bg = match (tint, hovered) {
                (Some(c), h) => rgb(c, (if h { 0.32 } else { 0.22 }) * o),
                (None, true) => color(1.0, 1.0, 1.0, 0.16 * o),
                (None, false) => color(1.0, 1.0, 1.0, 0.09 * o),
            };
            self.round_in(r, 13.0, bg);
            let fg = tint.map_or(color(1.0, 1.0, 1.0, o), |c| rgb(c, o));
            self.text_in(glyph, &self.f.icon_small, rect(x + 10.0, y, x + 28.0, y + 26.0), fg);
            self.text_in(label, &self.f.label, rect(x + 30.0, y, x + w - 10.0, y + 26.0), fg);
        });
        self.add_hit(r, hit, o);
        w
    }

    pub(crate) fn art(&self, r: D2D_RECT_F, radius: f32, accent: [f32; 3], o: f32) {
        unsafe {
            match &self.cover {
                Some((_, brush, size)) => {
                    let (w, h) = (r.right - r.left, r.bottom - r.top);
                    // In brush space, which the stagger translation moves along with the shape.
                    brush.SetTransform(&Matrix3x2 { M11: w / size, M12: 0.0, M21: 0.0, M22: h / size, M31: r.left, M32: r.top });
                    brush.SetOpacity(o);
                    self.round(r, radius, brush);
                }
                None => {
                    self.round_in(r, radius, rgb(accent, 0.22 * o));
                    self.text_in("\u{E8D6}", &self.f.icon, r, rgb(accent, 0.9 * o));
                }
            }
        }
    }

    /// Little equalizer bars; `hop` lifts them once.
    fn bars(&self, cx: f32, cy: f32, c: [f32; 3], hop: f32, o: f32) {
        let heights = [0.45, 0.85, 0.6, 1.0, 0.5];
        for (i, k) in heights.iter().enumerate() {
            let h = 10.0 * (k + hop * (0.5 - (i as f32 - 2.0).abs() * 0.12)).min(1.3);
            let x = cx - 8.0 + i as f32 * 4.0;
            self.round_in(rect(x - 1.1, cy + 5.0 - h, x + 1.1, cy + 5.0), 1.1, rgb(c, o));
        }
    }

    /// The shell's picture for a file, uploaded once. 80 px covers 40 DIP tiles up to 200% scaling.
    pub(crate) fn thumb(&self, path: &str) -> Option<ID2D1Bitmap> {
        if let Some(b) = self.thumbs.borrow().get(path) {
            return b.clone();
        }
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        let bmp = crate::shelf::thumbnail(path, 80).and_then(|t| unsafe {
            self.rt.CreateBitmap(D2D_SIZE_U { width: t.w, height: t.h }, Some(t.pixels.as_ptr().cast()), t.w * 4, &props).ok()
        });
        self.thumbs.borrow_mut().insert(path.to_string(), bmp.clone());
        bmp
    }

    /// The line being sung, clipped to its space (an ayah can be long). With word timing, the words sung so far
    /// are bright and the rest dim.
    pub(crate) fn sung(&self, line: &Line, text: &str, lit: usize, r: D2D_RECT_F, o: f32) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dw.CreateTextLayout(&wide, &self.f.lyric, r.right - r.left, r.bottom - r.top) else { return };
            self.ink.SetOpacity(o);
            self.dim.SetOpacity(0.38 * o);
            let base = if line.words.is_empty() {
                &self.ink
            } else {
                for (start, length) in crate::lrc::word_spans(text, &line.words).into_iter().take(lit).flatten() {
                    let _ = layout.SetDrawingEffect(&self.ink, DWRITE_TEXT_RANGE { startPosition: start, length });
                }
                &self.dim
            };
            self.rt.DrawTextLayout(Vector2 { X: r.left, Y: r.top }, &layout, base, D2D1_DRAW_TEXT_OPTIONS_CLIP);
        }
    }

    /// The notch's outline: flat top on the screen edge, concave shoulders, rounded bottom corners.
    fn notch_shape(&self, left: f32, w: f32, h: f32, radius: f32, shoulder: f32) -> Option<ID2D1PathGeometry> {
        let (r, s, right) = (radius.min(h).min(w / 2.0).max(0.0), shoulder, left + w);
        let arc = |to: (f32, f32), radius: f32, dir| D2D1_ARC_SEGMENT {
            point: Vector2 { X: to.0, Y: to.1 },
            size: D2D_SIZE_F { width: radius, height: radius },
            rotationAngle: 0.0,
            sweepDirection: dir,
            arcSize: D2D1_ARC_SIZE_SMALL,
        };
        unsafe {
            let geo = self.factory.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            sink.BeginFigure(Vector2 { X: left - s, Y: 0.0 }, D2D1_FIGURE_BEGIN_FILLED);
            sink.AddArc(&arc((left, s), s, D2D1_SWEEP_DIRECTION_CLOCKWISE));
            sink.AddLine(Vector2 { X: left, Y: h - r });
            sink.AddArc(&arc((left + r, h), r, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE));
            sink.AddLine(Vector2 { X: right - r, Y: h });
            sink.AddArc(&arc((right, h - r), r, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE));
            sink.AddLine(Vector2 { X: right, Y: s });
            sink.AddArc(&arc((right + s, 0.0), s, D2D1_SWEEP_DIRECTION_CLOCKWISE));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close().ok()?;
            Some(geo)
        }
    }

    /// The accent's soft light along the notch's bottom edge, cached per colour.
    fn glow(&self, accent: [f32; 3], cx: f32, h: f32, w: f32, o: f32) {
        let key = accent.map(|c| (c * 255.0) as u8);
        unsafe {
            let mut cache = self.glow.borrow_mut();
            if cache.as_ref().is_none_or(|(k, _)| *k != key) {
                let stops =
                    [D2D1_GRADIENT_STOP { position: 0.0, color: rgb(accent, 0.30) }, D2D1_GRADIENT_STOP { position: 1.0, color: rgb(accent, 0.0) }];
                let Ok(coll) = self.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) else { return };
                let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: Vector2::default(),
                    gradientOriginOffset: Vector2::default(),
                    radiusX: 1.0,
                    radiusY: 1.0,
                };
                let Ok(b) = self.rt.CreateRadialGradientBrush(&props, None, &coll) else { return };
                *cache = Some((key, b));
            }
            let Some((_, b)) = cache.as_ref() else { return };
            b.SetCenter(Vector2 { X: cx, Y: h + 6.0 });
            b.SetRadiusX(w * 0.42);
            b.SetRadiusY(h * 0.42);
            b.SetOpacity(o);
            self.rt.FillRectangle(&rect(cx - w / 2.0, 0.0, cx + w / 2.0, h), b);
        }
    }

    pub fn draw(&self, s: &Scene) -> Result<()> {
        self.hits.borrow_mut().clear();
        self.state.set(FrameState { open: s.open.clamp(0.0, 1.0), hover: s.hover, pressed: s.pressed, press: s.press, dy: 0.0 });
        let left = (self.box_w - s.w) / 2.0;
        let pill = rect(left, 0.0, left + s.w, s.h);
        let o = s.open.clamp(0.0, 1.0);
        unsafe {
            self.rt.BeginDraw();
            self.rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 });
            self.rt.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            // A peeking notch lightens a touch: it heard you.
            let n = self.notch;
            let lift = 0.05 * s.peek;
            self.fill.SetColor(&color(n.r + lift, n.g + lift, n.b + lift * 1.2, n.a));
            self.fill.SetOpacity(1.0);

            if s.line && s.toast.is_none() && o < 0.01 {
                // Line style: a thin line, with an invisible band under it so the pointer still finds it.
                self.round_in(rect(pill.left, 0.0, pill.right, 12.0), 0.0, color(0.0, 0.0, 0.0, 2.0 / 255.0));
                self.round(rect(pill.left, -2.0, pill.right, 4.0), 2.0, &self.fill);
                return self.rt.EndDraw(None, None);
            }

            match self.notch_shape(left, s.w, s.h, s.radius, shoulder(s.h, s.radius, o)) {
                Some(geo) => self.rt.FillGeometry(&geo, &self.fill, None),
                None => self.round(D2D_RECT_F { top: -s.radius, ..pill }, s.radius, &self.fill),
            }
            self.rt.PushAxisAlignedClip(&pill, D2D1_ANTIALIAS_MODE_ALIASED);

            if o > 0.001 {
                self.glow(self.page_accent(s), left + s.w / 2.0, s.h, s.w, o);
            }
            if o < 0.999 {
                self.closed(s, pill, 1.0 - o);
            }
            if o > 0.001 {
                // Content is laid out for the open size and stays centred while the notch grows around it.
                let ow = self.open_w();
                let origin = left + (s.w - ow) / 2.0;
                self.header(s, origin, o);
                for (i, page) in s.pages.iter().enumerate() {
                    let x = origin + (i as f32 - s.page) * ow;
                    if x >= left + s.w || x + ow <= left {
                        continue;
                    }
                    // Pages fade a little as they slide away.
                    let po = o * (1.0 - (i as f32 - s.page).abs()).clamp(0.0, 1.0);
                    self.page(s, *page, x, po);
                }
                self.set_block(0.0);
                if s.pages.len() > 1 {
                    self.dots(s, left, o);
                    self.arrows(s, left, o);
                }
            }

            self.rt.PopAxisAlignedClip();
            self.rt.EndDraw(None, None)
        }
    }

    /// The accent of the page in view (blended while swiping).
    fn page_accent(&self, s: &Scene) -> [f32; 3] {
        let of = |i: usize| s.pages.get(i).map_or(NEUTRAL, |p| self.accent_of(s, *p));
        let (a, b) = (of(s.page.floor().max(0.0) as usize), of(s.page.ceil().max(0.0) as usize));
        let t = s.page.fract();
        [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
    }

    pub(crate) fn accent_of(&self, s: &Scene, p: Page) -> [f32; 3] {
        match p {
            Page::Music => s.music.as_ref().map_or(NEUTRAL, |m| m.accent),
            Page::Focus if s.focus.on_break => TEAL,
            Page::Focus => CORAL,
            Page::Prayer => AMBER,
            Page::Water => AQUA,
            Page::Todo => MINT,
            Page::Widgets | Page::Shelf => NEUTRAL,
        }
    }

    /// Sets the staggered entrance for block `k` (by open-mix): moves what follows down a little, returns opacity.
    pub(crate) fn block(&self, k: u32, o: f32) -> f32 {
        let (op, dy) = stagger(self.state.get().open, k);
        self.set_block(dy);
        o * op
    }

    fn set_block(&self, dy: f32) {
        let mut st = self.state.get();
        st.dy = dy;
        self.state.set(st);
        unsafe { self.rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: dy }) };
    }

    /// Title on the left (an accent dot, then the page's name), date · time and what's running on the right.
    fn header(&self, s: &Scene, left: f32, o: f32) {
        let o = self.block(0, o);
        let (top, bottom) = (8.0, 30.0);
        for (i, page) in s.pages.iter().enumerate() {
            let near = (1.0 - (i as f32 - s.page).abs()).clamp(0.0, 1.0);
            if near <= 0.01 {
                continue;
            }
            let title = match page {
                Page::Music => "Now playing",
                Page::Focus => "Focus",
                Page::Prayer => "Prayer",
                Page::Water => "Water",
                Page::Todo => "To-do",
                Page::Widgets => "Widgets",
                Page::Shelf => "Shelf",
            };
            let slide = (i as f32 - s.page) * 18.0;
            self.dot(left + 22.0 + slide, (top + bottom) / 2.0, 3.5, rgb(self.accent_of(s, *page), near * o));
            self.text(title, &self.f.title, rect(left + 32.0 + slide, top, left + 200.0 + slide, bottom), &self.ink, near * o);
        }
        let right = left + self.open_w() - 18.0;
        if let Some(note) = s.note {
            // A problem in config.toml wins the header's right side until it's fixed.
            self.text_in(note, &self.f.caption_end, rect(left + 150.0, top, right, bottom), rgb(AMBER, o));
            return;
        }
        let mut x = right;
        if s.playing {
            let c = s.music.as_ref().map_or(NEUTRAL, |m| m.accent);
            self.bars(x - 8.0, (top + bottom) / 2.0, c, 0.0, o);
            x -= 26.0;
        }
        let tw = self.measure_in(&self.f.head_time, s.time);
        self.text(s.time, &self.f.head_time, rect(x - 80.0, top, x, bottom), &self.ink, o);
        x -= tw + 8.0;
        self.text(s.date, &self.f.head_date, rect(x - 120.0, top, x, bottom), &self.ink, 0.55 * o);
        x -= self.measure_in(&self.f.head_date, s.date) + 14.0;
        if let Some(f) = s.focus_ring {
            self.ring(x - 6.0, (top + bottom) / 2.0, 6.0, 2.0, f, if s.focus.on_break { TEAL } else { CORAL }, o);
        }
    }

    fn dots(&self, s: &Scene, left: f32, o: f32) {
        let n = s.pages.len();
        let y = s.h - 9.0;
        // The dot for the page in view stretches into a little pill.
        let widths: Vec<f32> = (0..n).map(|i| 5.0 + 9.0 * (1.0 - (i as f32 - s.page).abs()).clamp(0.0, 1.0)).collect();
        let total: f32 = widths.iter().sum::<f32>() + 5.0 * (n - 1) as f32;
        let mut x = left + (s.w - total) / 2.0;
        for (i, w) in widths.iter().enumerate() {
            let near = (w - 5.0) / 9.0;
            self.round_in(rect(x, y - 2.5, x + w, y + 2.5), 2.5, color(1.0, 1.0, 1.0, (0.28 + 0.72 * near) * o));
            self.add_hit(rect(x - 3.0, y - 7.0, x + w + 3.0, s.h), Hit::Page(i), o);
            x += w + 5.0;
        }
    }

    /// ‹ › on the sides, faded in only while the pointer is near that side.
    fn arrows(&self, s: &Scene, left: f32, o: f32) {
        let cy = (36.0 + s.h - 22.0) / 2.0;
        let cur = s.page.round() as usize;
        if cur > 0 && s.arrows.0 > 0.01 {
            self.side_arrow(left + 14.0, cy, "\u{E76B}", Hit::Page(cur - 1), s.arrows.0 * o);
        }
        if cur + 1 < s.pages.len() && s.arrows.1 > 0.01 {
            self.side_arrow(left + s.w - 14.0, cy, "\u{E76C}", Hit::Page(cur + 1), s.arrows.1 * o);
        }
    }

    fn side_arrow(&self, cx: f32, cy: f32, glyph: &str, hit: Hit, o: f32) {
        self.dot(cx, cy, 11.0, color(0.16, 0.16, 0.18, 0.95 * o));
        self.text(glyph, &self.f.icon_small, rect(cx - 11.0, cy - 11.0, cx + 11.0, cy + 11.0), &self.ink, o);
        // Generous target: the whole side strip.
        self.add_hit(rect(cx - 14.0, cy - 30.0, cx + 14.0, cy + 30.0), hit, if o > 0.3 { 1.0 } else { 0.0 });
    }

    fn closed(&self, s: &Scene, pill: D2D_RECT_F, o: f32) {
        let (cx, cy) = ((pill.left + pill.right) / 2.0, s.h / 2.0);
        if let Some(t) = &s.toast {
            // Pops in from the middle: a little small, then settles (the pop spring overshoots).
            let sc = 0.9 + 0.1 * s.pop;
            self.scaled(cx, cy, sc, || self.toast(t, pill, s.h, o * s.pop.min(1.0)));
            return;
        }
        match &s.closed {
            Closed::Clock(t) => self.text(t, &self.f.clock, pill, &self.ink, o),
            Closed::Lyric(line) => {
                let side = (s.h - 10.0).max(0.0);
                let art = rect(pill.left + 6.0, 5.0, pill.left + 6.0 + side, 5.0 + side);
                self.art(art, side * 0.3, NEUTRAL, o);
                self.text(line, &self.f.activity, rect(art.right + 9.0, 0.0, pill.right - 14.0, s.h), &self.ink, o);
            }
            Closed::Live { left, center, right } => {
                let slot_w = s.h;
                self.slot(left, rect(pill.left + 6.0, 0.0, pill.left + 6.0 + slot_w, s.h), o);
                self.text(center, &self.f.clock, rect(pill.left + slot_w, 0.0, pill.right - slot_w, s.h), &self.ink, o);
                self.slot(right, rect(pill.right - 6.0 - slot_w, 0.0, pill.right - 6.0, s.h), o);
            }
            Closed::Eyes { lid, dozing, bounce } => {
                let y = cy - bounce * 3.0;
                for dx in [-12.0, 12.0] {
                    let x = cx + dx;
                    if *dozing {
                        self.round_in(rect(x - 5.0, y + 1.0, x + 5.0, y + 3.0), 1.0, color(0.75, 0.9, 1.0, 0.55 * o));
                    } else {
                        let h = (12.0 * (1.0 - lid)).max(1.6);
                        // A soft glow behind each eye, then the eye.
                        self.round_in(rect(x - 7.0, y - h / 2.0 - 2.0, x + 7.0, y + h / 2.0 + 2.0), 6.0, color(0.55, 0.8, 1.0, 0.16 * o));
                        self.round_in(rect(x - 4.5, y - h / 2.0, x + 4.5, y + h / 2.0), 4.5_f32.min(h / 2.0), color(0.9, 0.97, 1.0, o));
                    }
                }
            }
        }
    }

    fn slot(&self, slot: &Slot, r: D2D_RECT_F, o: f32) {
        let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
        match slot {
            Slot::Cover => {
                let side = r.bottom - r.top - 10.0;
                self.art(rect(cx - side / 2.0, cy - side / 2.0, cx + side / 2.0, cy + side / 2.0), side * 0.3, NEUTRAL, o);
            }
            Slot::Ring(f, c) => self.ring(cx, cy, 6.5, 2.2, *f, *c, o),
            Slot::Glyph(g, c) => self.text_in(g, &self.f.icon_small, r, rgb(*c, o)),
            Slot::Bars(c, hop) => self.bars(cx, cy, *c, *hop, o),
            Slot::Text(t, c) => self.text_in(t, &self.f.caption_center, r, rgb(*c, o)),
        }
    }

    /// Pop-ups: [icon disc (with a ring) | title (+ second line) | value], or [icon | level bar | value].
    fn toast(&self, t: &Toast, pill: D2D_RECT_F, h: f32, o: f32) {
        let d = (h - 12.0).clamp(18.0, 30.0);
        let (ix, iy) = (pill.left + 8.0 + d / 2.0, h / 2.0);
        self.dot(ix, iy, d / 2.0, rgb(t.tint, 0.20 * o));
        if let Some(f) = t.ring {
            self.ring(ix, iy, d / 2.0 - 1.5, 2.2, f, t.tint, o);
        }
        let glyph_f = if d > 24.0 { &self.f.icon } else { &self.f.icon_small };
        self.text_in(t.glyph, glyph_f, rect(ix - d / 2.0, iy - d / 2.0, ix + d / 2.0, iy + d / 2.0), rgb(t.tint, o));
        let after = ix + d / 2.0 + 9.0;
        let value_w = t.value.map_or(0.0, |v| self.measure_in(&self.f.value, v) + 14.0);
        if let Some(level) = t.level {
            let bar_r = pill.right - 14.0 - value_w.max(30.0);
            self.progress(after, bar_r, iy, level, [1.0, 1.0, 1.0], o);
        } else if let Some(sub) = t.sub {
            self.text(t.title, &self.f.activity, rect(after, iy - 16.0, pill.right - 12.0 - value_w, iy), &self.ink, o);
            self.text(sub, &self.f.caption, rect(after, iy - 1.0, pill.right - 12.0 - value_w, iy + 15.0), &self.ink, 0.6 * o);
        } else {
            self.text(t.title, &self.f.activity, rect(after, 0.0, pill.right - 12.0 - value_w, h), &self.ink, o);
        }
        if let Some(v) = t.value {
            self.text(v, &self.f.value, rect(pill.right - 12.0 - value_w, 0.0, pill.right - 12.0, h), &self.ink, o);
        }
    }

    /// Shows the last drawn frame with per-pixel alpha at screen position (x, y).
    pub fn present(&self, hwnd: HWND, x: i32, y: i32) -> Result<()> {
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                None,
                Some(&POINT { x, y }),
                Some(&SIZE { cx: self.px_w, cy: self.px_h }),
                Some(self.mem),
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.free_surface();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_icon_shape() {
        for size in [16, 20, 24, 32] {
            let px = icon_pixels(size);
            let at = |x: u32, y: u32| &px[((y * size + x) * 4) as usize..][..4];
            assert_eq!(px.len(), (size * size * 4) as usize);
            assert_eq!(at(0, 0)[3], 0, "rounded corner is see-through ({size})");
            let mid = size / 2;
            assert!(at(mid, mid)[3] > 200 && at(mid, mid)[0] < 60, "dark middle ({size})");
            let notch = at(mid, (size as f32 * 0.16) as u32);
            assert!(notch[3] > 200 && notch[0] > 200, "white notch near the top ({size}): {notch:?}");
            assert!(at(0, mid)[0] > 200 && at(0, mid)[3] > 100, "white rim on the left edge ({size})");
        }
    }

    #[test]
    fn times() {
        assert_eq!(clock_text(0), "0:00");
        assert_eq!(clock_text(187_900), "3:07");
        assert_eq!(clock_text(3_723_000), "1:02:03");
    }

    #[test]
    fn staggered_entrance() {
        assert_eq!(stagger(0.0, 0), (0.0, 6.0));
        assert_eq!(stagger(1.0, 2), (1.0, 0.0), "everything is in place once open");
        let (header, right) = (stagger(0.4, 0).0, stagger(0.4, 2).0);
        assert!(header > right, "the header leads, the right column follows");
    }

    #[test]
    fn shoulders_grow_as_it_opens() {
        assert!(shoulder(26.0, 13.0, 0.0) < shoulder(160.0, 30.0, 1.0));
        assert!(shoulder(4.0, 2.0, 0.0) <= 2.0, "a thin line has tiny shoulders");
    }
}
