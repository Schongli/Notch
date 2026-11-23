// Drawing: Direct2D (software) into a premultiplied DIB, shown with UpdateLayeredWindow.
// Software D2D keeps RAM low (no D3D device); the surface is small, so CPU cost per frame is tiny.
use std::cell::RefCell;
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
    Page(usize),
    FocusToggle,
    FocusSkip,
    FocusReset,
    AddTask,
    /// Index into the app's task list
    Task(usize),
    ClearDone,
    ShelfItem(usize),
    ShelfRemove(usize),
    ShelfClear,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Page {
    Music,
    Lyrics,
    Clock,
    Focus,
    Todo,
    Shelf,
}

pub struct FocusView<'a> {
    pub label: &'a str,
    /// "18:42"
    pub left: &'a str,
    /// Share of the phase still to go, 1 → 0
    pub fraction: f32,
    pub running: bool,
    pub on_break: bool,
    /// Focus rounds finished, of `rounds`
    pub done: u32,
    pub rounds: u32,
}

pub struct TaskRow {
    /// Index into the app's task list
    pub index: usize,
    pub text: String,
    /// "15:30", or "Tue 09:00" when it isn't today
    pub due: String,
    pub done: bool,
}

pub struct Prayers<'a> {
    pub names: &'a [String],
    pub times: [String; 5],
    pub next: usize,
    /// "Asr in 1 h 8 min"
    pub coming: String,
}

/// What the closed pill shows: an icon glyph, or the album cover when `glyph` is None.
pub struct Activity<'a> {
    pub glyph: Option<&'a str>,
    pub text: &'a str,
}

pub struct Music<'a> {
    pub title: &'a str,
    pub artist: &'a str,
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
    /// 1 right after the lyric line changes, springing to 0: the lines slide up into place.
    pub slide: f32,
}

pub struct Scene<'a> {
    pub w: f32,
    pub h: f32,
    pub radius: f32,
    /// 0 = closed content, 1 = open content (cross-fade)
    pub open: f32,
    pub pages: &'a [Page],
    /// Which page is showing; fractional while swiping.
    pub page: f32,
    pub time: &'a str,
    pub date: &'a str,
    /// A problem to show in the open notch, e.g. a typo in config.toml
    pub note: Option<&'a str>,
    pub music: Option<Music<'a>>,
    pub focus: FocusView<'a>,
    pub tasks: &'a [TaskRow],
    pub prayers: Option<Prayers<'a>>,
    /// Shelf file paths, newest first
    pub shelf: &'a [String],
    /// Files are being dragged over the notch
    pub drop_hover: bool,
    /// What the closed pill shows instead of the clock: focus countdown, music, a coming prayer.
    pub activity: Option<Activity<'a>>,
    /// A short-lived system pop-up (volume, charger, headphones, Caps Lock); wins over the activity.
    pub toast: Option<Toast<'a>>,
}

pub struct Toast<'a> {
    /// An icon-font glyph
    pub glyph: &'a str,
    pub text: &'a str,
    /// 0..1 for a level bar (volume); the text then sits at the right end.
    pub level: Option<f32>,
}

/// Closed-pill width for a toast, in DIPs: icon + bar or text + padding.
pub const TOAST_BAR_W: f32 = 230.0;

struct Formats {
    clock: IDWriteTextFormat,
    activity: IDWriteTextFormat,
    title: IDWriteTextFormat,
    body: IDWriteTextFormat,
    tiny: IDWriteTextFormat,
    tiny_end: IDWriteTextFormat,
    lyric: IDWriteTextFormat,
    lyric_side: IDWriteTextFormat,
    big: IDWriteTextFormat,
    label: IDWriteTextFormat,
    note: IDWriteTextFormat,
    icon: IDWriteTextFormat,
    ring: IDWriteTextFormat,
    tiny_center: IDWriteTextFormat,
    icon_small: IDWriteTextFormat,
}

pub struct Renderer {
    /// The window box in DIPs: the open notch plus room for spring overshoot. Its transparent part lets clicks through.
    pub box_w: f32,
    pub box_h: f32,
    factory: ID2D1Factory,
    round_caps: ID2D1StrokeStyle,
    dw: IDWriteFactory,
    rt: ID2D1DCRenderTarget,
    fill: ID2D1SolidColorBrush,
    ink: ID2D1SolidColorBrush,
    /// The unsung part of a word-timed line
    dim: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    cover: Option<(String, ID2D1BitmapBrush, f32)>,
    /// Shelf thumbnails by path, made the first time a tile shows (None: the shell had no picture)
    thumbs: RefCell<std::collections::HashMap<String, Option<ID2D1Bitmap>>>,
    f: Formats,
    hits: RefCell<Vec<(D2D_RECT_F, Hit)>>,
    mem: HDC,
    dib: HBITMAP,
    /// The DIB's pixels, for saving a frame in tests
    #[cfg_attr(not(test), allow(dead_code))]
    bits: *mut c_void,
    old: HGDIOBJ,
    pub px_w: i32,
    pub px_h: i32,
}

fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

fn theme_color(s: &str) -> D2D1_COLOR_F {
    let [r, g, b, a] = parse_color(s).unwrap_or([0.0, 0.0, 0.0, 1.0]);
    color(r, g, b, a)
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F { left, top, right, bottom }
}

fn inside(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
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
            let (semi, normal) = (DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_FONT_WEIGHT_NORMAL);
            let f = Formats {
                clock: fmt(&font, 13.0, semi, center, false)?,
                activity: fmt(&font, 13.0, semi, lead, false)?,
                title: fmt(&font, 16.0, semi, lead, false)?,
                body: fmt(&font, 13.0, normal, lead, false)?,
                tiny: fmt(&font, 11.0, normal, lead, false)?,
                tiny_end: fmt(&font, 11.0, normal, end, false)?,
                lyric: fmt(&font, 17.0, semi, center, true)?,
                lyric_side: fmt(&font, 13.0, normal, center, false)?,
                big: fmt(&font, 44.0, semi, lead, false)?,
                label: fmt(&font, 14.0, normal, lead, false)?,
                note: fmt(&font, 13.0, normal, lead, true)?,
                icon: fmt(&icons, 18.0, normal, center, false)?,
                ring: fmt(&font, 22.0, semi, center, false)?,
                tiny_center: fmt(&font, 11.0, normal, center, false)?,
                icon_small: fmt(&icons, 9.0, normal, center, false)?,
            };
            let caps = D2D1_STROKE_STYLE_PROPERTIES { startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, ..Default::default() };
            let mut r = Self {
                box_w: theme.open_width + 40.0,
                box_h: theme.open_height + 30.0,
                round_caps: factory.CreateStrokeStyle(&caps, None)?,
                factory,
                fill: rt.CreateSolidColorBrush(&theme_color(&theme.background), None)?,
                ink: rt.CreateSolidColorBrush(&text, None)?,
                dim: rt.CreateSolidColorBrush(&text, None)?,
                accent: rt.CreateSolidColorBrush(&text, None)?,
                cover: None,
                thumbs: RefCell::default(),
                dw,
                f,
                hits: RefCell::default(),
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

    /// Width of the closed pill's activity text, to size the pill around it.
    pub fn measure(&self, t: &str) -> f32 {
        let wide: Vec<u16> = t.encode_utf16().collect();
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            self.dw
                .CreateTextLayout(&wide, &self.f.activity, 2000.0, 50.0)
                .and_then(|l| l.GetMetrics(&mut m))
                .map_or(0.0, |_| m.widthIncludingTrailingWhitespace)
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits.borrow().iter().rev().find(|(r, _)| inside(r, x, y)).map(|(_, h)| *h)
    }

    fn add_hit(&self, r: D2D_RECT_F, h: Hit, opacity: f32) {
        if opacity > 0.6 {
            self.hits.borrow_mut().push((r, h));
        }
    }

    fn text(&self, t: &str, f: &IDWriteTextFormat, r: D2D_RECT_F, b: &ID2D1SolidColorBrush, opacity: f32) {
        let wide: Vec<u16> = t.encode_utf16().collect();
        unsafe {
            b.SetOpacity(opacity);
            self.rt.DrawText(&wide, f, &r, b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    fn round(&self, r: D2D_RECT_F, radius: f32, b: &ID2D1Brush) {
        unsafe { self.rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }, b) }
    }

    fn art(&self, r: D2D_RECT_F, radius: f32, o: f32) {
        unsafe {
            match &self.cover {
                Some((_, brush, size)) => {
                    let (w, h) = (r.right - r.left, r.bottom - r.top);
                    brush.SetTransform(&Matrix3x2 { M11: w / size, M12: 0.0, M21: 0.0, M22: h / size, M31: r.left, M32: r.top });
                    brush.SetOpacity(o);
                    self.round(r, radius, &brush.cast().unwrap());
                }
                None => {
                    self.accent.SetOpacity(0.22 * o);
                    self.round(r, radius, &self.accent.cast().unwrap());
                    let f = &self.f.icon;
                    self.text("\u{E8D6}", f, r, &self.accent, 0.9 * o);
                }
            }
        }
    }

    pub fn draw(&self, s: &Scene) -> Result<()> {
        self.hits.borrow_mut().clear();
        let left = (self.box_w - s.w) / 2.0;
        let pill = rect(left, 0.0, left + s.w, s.h);
        let accent = s.music.as_ref().map_or([1.0, 1.0, 1.0], |m| m.accent);
        unsafe {
            self.accent.SetColor(&color(accent[0], accent[1], accent[2], 1.0));
            self.rt.BeginDraw();
            self.rt.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            // The play glyph is cut out in the background colour, which changes this brush's opacity.
            self.fill.SetOpacity(1.0);
            // The top edge sits on the screen edge, so the top corners are pushed off-surface: only the bottom ones round.
            self.round(D2D_RECT_F { top: -s.radius, ..pill }, s.radius, &self.fill.cast()?);
            self.rt.PushAxisAlignedClip(&pill, D2D1_ANTIALIAS_MODE_ALIASED);

            if s.open < 0.999 {
                self.closed(s, pill, 1.0 - s.open.max(0.0));
            }
            if s.open > 0.001 {
                let o = s.open.clamp(0.0, 1.0);
                for (i, page) in s.pages.iter().enumerate() {
                    let x = left + (i as f32 - s.page) * s.w;
                    if x >= left + s.w || x + s.w <= left {
                        continue;
                    }
                    // Pages fade a little as they slide away.
                    let po = o * (1.0 - (i as f32 - s.page).abs()).clamp(0.0, 1.0);
                    match (page, &s.music) {
                        (Page::Music, Some(m)) => self.music_page(m, x, s.w, po),
                        (Page::Lyrics, Some(m)) => self.lyrics_page(m, x, s.w, po),
                        (Page::Focus, _) => self.focus_page(&s.focus, x, s.w, po),
                        (Page::Todo, _) => self.todo_page(s.tasks, x, s.w, s.h, po),
                        (Page::Shelf, _) => self.shelf_page(s.shelf, s.drop_hover, x, s.w, s.h, po),
                        _ => self.clock_page(s, x, po),
                    }
                }
                if s.pages.len() > 1 {
                    self.dots(s, left, o);
                }
            }

            self.rt.PopAxisAlignedClip();
            self.rt.EndDraw(None, None)
        }
    }

    fn closed(&self, s: &Scene, pill: D2D_RECT_F, o: f32) {
        if let Some(t) = &s.toast {
            let glyph = rect(pill.left + 8.0, 0.0, pill.left + 8.0 + (s.h - 6.0), s.h);
            self.text(t.glyph, &self.f.icon, glyph, &self.ink, o);
            let after = glyph.right + 8.0;
            match t.level {
                Some(level) => {
                    let (y, bar_r) = (s.h / 2.0, pill.right - 46.0);
                    let track = rect(after, y - 2.5, bar_r, y + 2.5);
                    self.text(t.text, &self.f.tiny_end, rect(bar_r + 4.0, 0.0, pill.right - 14.0, s.h), &self.ink, o);
                    unsafe {
                        self.ink.SetOpacity(0.22 * o);
                        self.round(track, 2.5, &self.ink.cast().unwrap());
                        self.ink.SetOpacity(o);
                        let fill = after + (bar_r - after) * level.clamp(0.0, 1.0);
                        self.round(rect(after, y - 2.5, fill.max(after + 5.0), y + 2.5), 2.5, &self.ink.cast().unwrap());
                    }
                }
                None => self.text(t.text, &self.f.activity, rect(after, 0.0, pill.right - 14.0, s.h), &self.ink, o),
            }
            return;
        }
        match &s.activity {
            Some(act) => {
                let side = (s.h - 10.0).max(0.0);
                let art = rect(pill.left + 6.0, 5.0, pill.left + 6.0 + side, 5.0 + side);
                match act.glyph {
                    Some(g) => self.text(g, &self.f.icon, art, &self.ink, o),
                    None => self.art(art, side * 0.3, o),
                }
                self.text(act.text, &self.f.activity, rect(art.right + 9.0, 0.0, pill.right - 14.0, s.h), &self.ink, o);
            }
            None => self.text(s.time, &self.f.clock, pill, &self.ink, o),
        }
    }

    fn clock_page(&self, s: &Scene, x: f32, o: f32) {
        // With prayer times on, they take a column on the right.
        let right = if s.prayers.is_some() { x + s.w - 168.0 } else { x + s.w - 24.0 };
        let row = |top: f32, h: f32| rect(x + 24.0, top, right, top + h);
        self.text(s.time, &self.f.big, row(18.0, 56.0), &self.ink, o);
        self.text(s.date, &self.f.label, row(74.0, 22.0), &self.ink, 0.6 * o);
        if let Some(p) = &s.prayers {
            self.text(&p.coming, &self.f.body, row(102.0, 20.0), &self.ink, 0.85 * o);
            let col = x + s.w - 150.0;
            for (i, (name, time)) in p.names.iter().zip(&p.times).enumerate() {
                let top = 20.0 + i as f32 * 23.0;
                let a = if i == p.next { 1.0 } else { 0.45 };
                self.text(name, &self.f.body, rect(col, top, col + 80.0, top + 20.0), &self.ink, a * o);
                self.text(time, &self.f.tiny_end, rect(col + 70.0, top, x + s.w - 24.0, top + 20.0), &self.ink, a * o);
            }
        }
        if let Some(note) = s.note {
            self.text(note, &self.f.note, row(s.h - 66.0, 50.0), &self.ink, o);
        }
    }

    fn ring(&self, cx: f32, cy: f32, r: f32, frac: f32, o: f32) {
        unsafe {
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
            self.ink.SetOpacity(0.16 * o);
            self.rt.DrawEllipse(&e, &self.ink, 6.0, None);
            self.accent.SetOpacity(o);
            if frac >= 0.999 {
                self.rt.DrawEllipse(&e, &self.accent, 6.0, None);
                return;
            }
            if frac < 0.002 {
                return;
            }
            // Clockwise from 12 o'clock, shrinking as time runs out.
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
                self.rt.DrawGeometry(&geo, &self.accent, 6.0, &self.round_caps);
            }
        }
    }

    fn focus_page(&self, f: &FocusView, x: f32, w: f32, o: f32) {
        let (cx, cy, r) = (x + 84.0, 80.0, 52.0);
        // Warm for focus, cool for breaks.
        let tint = if f.on_break { color(0.36, 0.84, 0.69, 1.0) } else { color(1.0, 0.55, 0.36, 1.0) };
        unsafe { self.accent.SetColor(&tint) };
        self.ring(cx, cy, r, f.fraction, o);
        self.text(f.left, &self.f.ring, rect(cx - r, cy - 16.0, cx + r, cy + 16.0), &self.ink, o);

        let x0 = x + 164.0;
        self.text(f.label, &self.f.title, rect(x0, 30.0, x + w - 20.0, 54.0), &self.ink, o);
        for i in 0..f.rounds.min(12) {
            let e = D2D1_ELLIPSE { point: Vector2 { X: x0 + 5.0 + i as f32 * 14.0, Y: 68.0 }, radiusX: 4.0, radiusY: 4.0 };
            unsafe {
                let b = if i < f.done { &self.accent } else { &self.ink };
                b.SetOpacity(if i < f.done { o } else { 0.25 * o });
                self.rt.FillEllipse(&e, b);
            }
        }
        let cy = 112.0;
        let play = rect(x0, cy - 21.0, x0 + 42.0, cy + 21.0);
        unsafe {
            self.ink.SetOpacity(o);
            let e = D2D1_ELLIPSE { point: Vector2 { X: x0 + 21.0, Y: cy }, radiusX: 21.0, radiusY: 21.0 };
            self.rt.FillEllipse(&e, &self.ink);
        }
        self.text(if f.running { "\u{E769}" } else { "\u{E768}" }, &self.f.icon, play, &self.fill, o);
        self.add_hit(play, Hit::FocusToggle, o);
        for (i, (glyph, hit)) in [("\u{E893}", Hit::FocusSkip), ("\u{E72C}", Hit::FocusReset)].into_iter().enumerate() {
            let bx = x0 + 58.0 + i as f32 * 48.0;
            let r = rect(bx, cy - 21.0, bx + 42.0, cy + 21.0);
            self.text(glyph, &self.f.icon, r, &self.ink, 0.85 * o);
            self.add_hit(r, hit, o);
        }
    }

    /// The shell's picture for a file, uploaded once. 80 px covers 40 DIP tiles up to 200% scaling.
    fn thumb(&self, path: &str) -> Option<ID2D1Bitmap> {
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

    fn shelf_page(&self, shelf: &[String], hover: bool, x: f32, w: f32, h: f32, o: f32) {
        if hover {
            // Waiting for the drop: a soft outline and an invitation.
            unsafe {
                self.ink.SetOpacity(0.5 * o);
                let r = D2D1_ROUNDED_RECT { rect: rect(x + 10.0, 8.0, x + w - 10.0, h - 18.0), radiusX: 18.0, radiusY: 18.0 };
                self.rt.DrawRoundedRectangle(&r, &self.ink, 1.5, None);
            }
        }
        self.text("Shelf", &self.f.title, rect(x + 24.0, 14.0, x + 140.0, 38.0), &self.ink, o);
        if hover {
            self.text("Release to add", &self.f.tiny_end, rect(x + w - 180.0, 14.0, x + w - 24.0, 38.0), &self.ink, o);
        } else if !shelf.is_empty() {
            let n = shelf.len();
            self.text(
                &format!("{n} item{}", if n == 1 { "" } else { "s" }),
                &self.f.tiny_end,
                rect(x + w - 190.0, 14.0, x + w - 76.0, 38.0),
                &self.ink,
                0.55 * o,
            );
            let clear = rect(x + w - 66.0, 14.0, x + w - 24.0, 38.0);
            self.text("Clear", &self.f.tiny_end, clear, &self.ink, 0.8 * o);
            self.add_hit(clear, Hit::ShelfClear, o);
        }
        if shelf.is_empty() {
            let (cx, cy) = (x + w / 2.0, 76.0);
            self.text("\u{E8B7}", &self.f.icon, rect(cx - 20.0, cy - 20.0, cx + 20.0, cy + 20.0), &self.ink, 0.5 * o);
            let say = if hover { "Drop to keep them here" } else { "Drop files here to keep them close" };
            self.text(say, &self.f.lyric_side, rect(x + 24.0, 100.0, x + w - 24.0, 124.0), &self.ink, 0.55 * o);
            return;
        }
        let shown = 5;
        let tile = (w - 48.0) / shown as f32;
        for (i, path) in shelf.iter().take(shown).enumerate() {
            let tx = x + 24.0 + i as f32 * tile;
            let (cx, top) = (tx + tile / 2.0, 46.0);
            let box_ = rect(cx - 22.0, top, cx + 22.0, top + 44.0);
            match self.thumb(path) {
                Some(b) => unsafe {
                    // Fit inside the box, keeping its shape.
                    let size = b.GetSize();
                    let k = (44.0 / size.width).min(44.0 / size.height);
                    let (bw, bh) = (size.width * k, size.height * k);
                    let dest = rect(cx - bw / 2.0, top + 22.0 - bh / 2.0, cx + bw / 2.0, top + 22.0 + bh / 2.0);
                    self.rt.DrawBitmap(&b, Some(&dest), o, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                },
                None => self.text("\u{E8A5}", &self.f.icon, box_, &self.ink, 0.7 * o),
            }
            self.text(crate::shelf::name(path), &self.f.tiny_center, rect(tx + 2.0, top + 48.0, tx + tile - 2.0, top + 64.0), &self.ink, 0.8 * o);
            self.add_hit(rect(tx, top - 4.0, tx + tile, top + 66.0), Hit::ShelfItem(i), o);
            let x_ = rect(cx + 14.0, top - 6.0, cx + 30.0, top + 10.0);
            unsafe {
                self.ink.SetOpacity(0.2 * o);
                self.round(x_, 8.0, &self.ink.cast().unwrap());
            }
            self.text("\u{E711}", &self.f.icon_small, x_, &self.ink, 0.8 * o);
            self.add_hit(x_, Hit::ShelfRemove(i), o);
        }
        if shelf.len() > shown {
            self.text(&format!("+{} more", shelf.len() - shown), &self.f.tiny, rect(x + 24.0, h - 27.0, x + 160.0, h - 11.0), &self.ink, 0.5 * o);
        }
    }

    fn todo_page(&self, tasks: &[TaskRow], x: f32, w: f32, h: f32, o: f32) {
        let left = tasks.iter().filter(|t| !t.done).count();
        self.text("To-do", &self.f.title, rect(x + 24.0, 14.0, x + 140.0, 38.0), &self.ink, o);
        let count = if left == 0 { String::new() } else { format!("{left} left") };
        self.text(&count, &self.f.tiny_end, rect(x + w - 150.0, 14.0, x + w - 58.0, 38.0), &self.ink, 0.55 * o);
        let add = rect(x + w - 50.0, 12.0, x + w - 22.0, 40.0);
        unsafe {
            self.ink.SetOpacity(0.14 * o);
            self.round(add, 14.0, &self.ink.cast().unwrap());
        }
        self.text("\u{E710}", &self.f.icon, add, &self.ink, o);
        self.add_hit(rect(add.left - 6.0, 6.0, add.right + 6.0, 46.0), Hit::AddTask, o);

        if tasks.is_empty() {
            self.text("Nothing to do. Tap + to add a task.", &self.f.lyric_side, rect(x + 24.0, 60.0, x + w - 24.0, 120.0), &self.ink, 0.55 * o);
            return;
        }
        let shown = 4;
        for (row, t) in tasks.iter().take(shown).enumerate() {
            let top = 44.0 + row as f32 * 24.0;
            let a = if t.done { 0.4 } else { 1.0 };
            let (bx, by) = (x + 32.0, top + 11.0);
            unsafe {
                let e = D2D1_ELLIPSE { point: Vector2 { X: bx, Y: by }, radiusX: 7.0, radiusY: 7.0 };
                self.ink.SetOpacity(a * o);
                if t.done {
                    self.rt.FillEllipse(&e, &self.ink);
                } else {
                    self.rt.DrawEllipse(&e, &self.ink, 1.5, None);
                }
            }
            if t.done {
                self.text("\u{E73E}", &self.f.icon_small, rect(bx - 7.0, by - 7.0, bx + 7.0, by + 7.0), &self.fill, o);
            }
            self.text(&t.text, &self.f.body, rect(x + 48.0, top, x + w - 76.0, top + 22.0), &self.ink, a * o);
            self.text(&t.due, &self.f.tiny_end, rect(x + w - 96.0, top, x + w - 24.0, top + 22.0), &self.ink, 0.55 * a * o);
            self.add_hit(rect(x + 16.0, top, x + w - 20.0, top + 24.0), Hit::Task(t.index), o);
        }
        if tasks.len() > shown {
            self.text(&format!("+{} more", tasks.len() - shown), &self.f.tiny, rect(x + 48.0, h - 27.0, x + 160.0, h - 11.0), &self.ink, 0.5 * o);
        }
        if tasks.iter().any(|t| t.done) {
            let r = rect(x + w - 110.0, h - 28.0, x + w - 24.0, h - 11.0);
            self.text("Clear done", &self.f.tiny_end, r, &self.ink, 0.6 * o);
            self.add_hit(r, Hit::ClearDone, o);
        }
    }

    fn music_page(&self, m: &Music, x: f32, w: f32, o: f32) {
        let pad = 20.0;
        self.art(rect(x + pad, 18.0, x + pad + 64.0, 82.0), 12.0, o);
        let tx = x + pad + 64.0 + 14.0;
        self.text(m.title, &self.f.title, rect(tx, 22.0, x + w - pad, 44.0), &self.ink, o);
        self.text(m.artist, &self.f.body, rect(tx, 44.0, x + w - pad, 64.0), &self.ink, 0.6 * o);

        if m.duration_ms > 0 {
            let y = 100.0;
            let (bl, br) = (x + pad + 44.0, x + w - pad - 44.0);
            let frac = (m.pos_ms as f32 / m.duration_ms as f32).clamp(0.0, 1.0);
            self.text(&clock_text(m.pos_ms), &self.f.tiny, rect(x + pad, y - 9.0, bl - 6.0, y + 9.0), &self.ink, 0.6 * o);
            let left = m.duration_ms.saturating_sub(m.pos_ms);
            self.text(&format!("-{}", clock_text(left)), &self.f.tiny_end, rect(br + 6.0, y - 9.0, x + w - pad, y + 9.0), &self.ink, 0.6 * o);
            unsafe {
                self.ink.SetOpacity(0.22 * o);
                self.round(rect(bl, y - 2.0, br, y + 2.0), 2.0, &self.ink.cast().unwrap());
                self.accent.SetOpacity(o);
                let knob = bl + (br - bl) * frac;
                self.round(rect(bl, y - 2.0, knob, y + 2.0), 2.0, &self.accent.cast().unwrap());
                if m.can_seek {
                    self.ink.SetOpacity(o);
                    let e = D2D1_ELLIPSE { point: Vector2 { X: knob, Y: y }, radiusX: 5.0, radiusY: 5.0 };
                    self.rt.FillEllipse(&e, &self.ink);
                    self.add_hit(rect(bl - 6.0, y - 10.0, br + 6.0, y + 10.0), Hit::Seek { left: bl, right: br }, o);
                }
            }
        }

        let (cx, cy) = (x + w / 2.0, 136.0);
        let button = |c: f32, glyph: &str, hit: Hit| {
            let r = rect(c - 22.0, cy - 22.0, c + 22.0, cy + 22.0);
            self.text(glyph, &self.f.icon, r, &self.ink, o);
            self.add_hit(r, hit, o);
        };
        button(cx - 64.0, "\u{E892}", Hit::Previous);
        button(cx + 64.0, "\u{E893}", Hit::Next);
        unsafe {
            self.ink.SetOpacity(o);
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: 21.0, radiusY: 21.0 };
            self.rt.FillEllipse(&e, &self.ink);
        }
        let r = rect(cx - 22.0, cy - 22.0, cx + 22.0, cy + 22.0);
        self.text(if m.playing { "\u{E769}" } else { "\u{E768}" }, &self.f.icon, r, &self.fill, o);
        self.add_hit(r, Hit::PlayPause, o);
    }

    fn lyrics_page(&self, m: &Music, x: f32, w: f32, o: f32) {
        let row = |top: f32, bottom: f32| rect(x + 24.0, top, x + w - 24.0, bottom);
        let say = |t: &str| self.text(t, &self.f.lyric_side, row(50.0, 110.0), &self.ink, 0.6 * o);
        if !m.lyrics_ready {
            return say("Looking for lyrics…");
        }
        if m.lines.is_empty() {
            return say("No synced lyrics for this song");
        }
        let words = |l: &Line| if l.text.is_empty() { "♪".to_string() } else { l.text.clone() };
        let dy = m.slide * 30.0;
        let side = |i: usize, top: f32| {
            if let Some(l) = m.lines.get(i) {
                let r = row(top + dy, top + 24.0 + dy);
                self.text(&words(l), &self.f.lyric_side, r, &self.ink, 0.42 * o);
                self.add_hit(r, Hit::Line(l.ms), o);
            }
        };
        match m.line {
            Some(i) => {
                let line = &m.lines[i];
                // A translation takes the lower part of the current line's space.
                let (top, bottom) = if line.tr.is_some() { (36.0, 80.0) } else { (46.0, 106.0) };
                if i > 0 {
                    side(i - 1, if line.tr.is_some() { 10.0 } else { 16.0 });
                }
                let r = row(top + dy, bottom + dy);
                self.sung(line, &words(line), m.lit, r, o);
                self.add_hit(r, Hit::Line(line.ms), o);
                if let Some(tr) = &line.tr {
                    self.text(tr, &self.f.lyric_side, row(82.0 + dy, 104.0 + dy), &self.ink, 0.62 * o);
                }
                side(i + 1, 112.0);
            }
            // Before the first line: show it waiting in the middle.
            None => side(0, 64.0),
        }
    }

    /// The line being sung, clipped to its space (an ayah can be long). With word timing, the words sung so far
    /// are bright and the rest dim.
    fn sung(&self, line: &Line, text: &str, lit: usize, r: D2D_RECT_F, o: f32) {
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

    fn dots(&self, s: &Scene, left: f32, o: f32) {
        let n = s.pages.len();
        for i in 0..n {
            let cx = left + s.w / 2.0 + (i as f32 - (n - 1) as f32 / 2.0) * 12.0;
            let near = (1.0 - (i as f32 - s.page).abs()).clamp(0.0, 1.0);
            unsafe {
                self.ink.SetOpacity((0.3 + 0.7 * near) * o);
                let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: s.h - 9.0 }, radiusX: 2.6, radiusY: 2.6 };
                self.rt.FillEllipse(&e, &self.ink);
            }
            self.add_hit(rect(cx - 6.0, s.h - 16.0, cx + 6.0, s.h), Hit::Page(i), o);
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
    use crate::lrc::Word;

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

    /// The last frame as a top-down 32-bit BMP, to look at pages without putting anything on screen.
    fn save_bmp(r: &Renderer, path: &std::path::Path) {
        let (w, h) = (r.px_w, r.px_h);
        let px = unsafe {
            let _ = GdiFlush();
            std::slice::from_raw_parts(r.bits as *const u8, (w * h * 4) as usize)
        };
        let mut f = b"BM".to_vec();
        f.extend((54 + px.len() as u32).to_le_bytes());
        f.extend([0u8; 4]);
        f.extend(54u32.to_le_bytes());
        f.extend(40u32.to_le_bytes());
        f.extend(w.to_le_bytes());
        f.extend((-h).to_le_bytes());
        f.extend(1u16.to_le_bytes());
        f.extend(32u16.to_le_bytes());
        f.extend([0u8; 24]);
        f.extend(px);
        std::fs::write(path, f).unwrap();
    }

    /// Draws every page and a few closed states off-screen into BMPs under %TEMP%\notch-pages, for the README and for
    /// checking the look without touching the desktop: `cargo test render_pages -- --ignored`
    #[test]
    #[ignore]
    fn render_pages() {
        // Shelf thumbnails come from the shell, which needs COM on this thread (the app's UI thread has OLE).
        let _ = unsafe { windows::Win32::System::Ole::OleInitialize(None) };
        let cfg = crate::config::Config::default();
        let mut r = Renderer::new(96, &cfg.theme).unwrap();
        let dir = std::env::temp_dir().join("notch-pages");
        std::fs::create_dir_all(&dir).unwrap();
        // A made-up cover (dusk gradient and a sun), so the art and its accent colour show.
        let n = 96u32;
        let mut px = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
                let sun = (0.16 - (fx - 0.62).hypot(fy - 0.4)).clamp(0.0, 0.02) / 0.02;
                let [rr, gg, bb] = [0.98 - 0.55 * fy, 0.45 + 0.2 * fy, 0.55 + 0.3 * fy].map(|c: f32| c * (1.0 - sun) + sun);
                px.extend([bb, gg, rr].map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8).into_iter().chain([255]));
            }
        }
        let cover = crate::media::Cover { size: n, accent: crate::media::accent(&px), pixels: std::sync::Arc::new(px) };
        r.set_cover("demo", Some(&cover));
        let icon = icon_pixels(64);
        let mut bmp = b"BM".to_vec();
        for v in [54 + icon.len() as u32, 0, 54, 40, 64] {
            bmp.extend(v.to_le_bytes());
        }
        bmp.extend((-64i32).to_le_bytes());
        bmp.extend(1u16.to_le_bytes());
        bmp.extend(32u16.to_le_bytes());
        bmp.extend([0u8; 24]);
        bmp.extend(&icon);
        std::fs::write(dir.join("icon.bmp"), bmp).unwrap();

        let names: Vec<String> = ["Subuh", "Dzuhur", "Ashar", "Maghrib", "Isya"].map(String::from).to_vec();
        let tasks = vec![
            TaskRow { index: 0, text: "Reply to Dina".into(), due: "15:30".into(), done: false },
            TaskRow { index: 1, text: "Finish chapter 3 notes".into(), due: "Tue 09:00".into(), done: false },
            TaskRow { index: 2, text: "Buy kopi susu".into(), due: String::new(), done: true },
        ];
        // Shelf files of our own: the made-up cover as a picture, plus a note and a markdown file.
        let mut pic = b"BM".to_vec();
        for v in [54 + cover.pixels.len() as u32, 0, 54, 40, n] {
            pic.extend(v.to_le_bytes());
        }
        pic.extend((-(n as i32)).to_le_bytes());
        pic.extend(1u16.to_le_bytes());
        pic.extend(32u16.to_le_bytes());
        pic.extend([0u8; 24]);
        pic.extend(cover.pixels.iter());
        let files = [("sunset.bmp", pic), ("notes.txt", b"notch".to_vec()), ("Jot.md", b"- idea".to_vec())];
        let shelf: Vec<String> = files
            .into_iter()
            .map(|(name, bytes)| {
                let p = dir.join(name);
                std::fs::write(&p, bytes).unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect();
        let w = |ms, t: &str| Word { ms, text: t.into() };
        let song = vec![
            Line { ms: 0, text: "we fold the summer light".into(), ..Default::default() },
            Line {
                ms: 4000,
                text: "into paper birds tonight".into(),
                tr: Some("menjadi burung kertas malam ini".into()),
                words: vec![w(4000, "into"), w(4600, "paper"), w(5200, "birds"), w(5800, "tonight")],
            },
            Line { ms: 8000, text: "and send them through the sky".into(), ..Default::default() },
        ];
        let ayah = vec![Line {
            ms: 0,
            text: "بِسْمِ ٱللَّهِ ٱلرَّحْمَٰنِ ٱلرَّحِيمِ".into(),
            tr: Some("Dengan nama Allah Yang Maha Pengasih, Maha Penyayang.".into()),
            words: vec![w(0, "بِسْمِ"), w(700, "ٱللَّهِ"), w(1400, "ٱلرَّحْمَٰنِ"), w(2100, "ٱلرَّحِيمِ")],
        }];
        let pages = [Page::Music, Page::Lyrics, Page::Clock, Page::Focus, Page::Todo, Page::Shelf];
        let lyric = "into paper birds tonight";
        let shots = [
            ("closed-clock", None, None, None),
            ("closed-music", None, Some((None, lyric)), None),
            ("closed-focus", None, Some((Some("\u{E916}"), "Focus · 18 min")), None),
            ("closed-volume", None, None, Some(("\u{E994}", "64", Some(0.64)))),
            ("music", Some(0.0), None, None),
            ("lyrics", Some(1.0), None, None),
            ("lyrics-quran", Some(1.0), None, None),
            ("clock", Some(2.0), None, None),
            ("focus", Some(3.0), None, None),
            ("todo", Some(4.0), None, None),
            ("shelf", Some(5.0), None, None),
        ];
        for (name, page, activity, toast) in shots {
            let quran = name == "lyrics-quran";
            let lines = if quran { &ayah } else { &song };
            let music = Music {
                title: "Paper Sky",
                artist: "Lumen Coast",
                pos_ms: if name == "music" { 84_000 } else { 5300 },
                duration_ms: 207_000,
                playing: true,
                can_seek: true,
                accent: cover.accent,
                lines,
                line: Some(if quran { 0 } else { 1 }),
                lit: 2,
                lyrics_ready: true,
                slide: 0.0,
            };
            let focus = FocusView { label: "Focus · 2 of 4", left: "18:42", fraction: 0.75, running: true, on_break: false, done: 1, rounds: 4 };
            let prayers = Prayers {
                names: &names,
                times: ["04:05", "11:41", "15:05", "17:54", "19:08"].map(String::from),
                next: 2,
                coming: "Ashar in 1 h 8 min".into(),
            };
            // Closed: the pill hugs its content, as App::closed() does.
            let (w, h, radius) = match (page, activity, toast) {
                (Some(_), _, _) => (400.0, 170.0, 32.0),
                (None, _, Some((_, _, Some(_)))) => (TOAST_BAR_W, 30.0, 15.0),
                (None, Some((_, t)), _) => (r.measure(t) + 6.0 + 20.0 + 9.0 + 16.0, 30.0, 15.0),
                _ => (128.0, 30.0, 15.0),
            };
            let scene = Scene {
                w,
                h,
                radius,
                open: page.map_or(0.0, |_| 1.0),
                pages: &pages,
                page: page.unwrap_or(0.0),
                time: "15:04",
                date: "Monday, 23 November",
                note: None,
                music: Some(music),
                focus,
                tasks: &tasks,
                prayers: Some(prayers),
                shelf: &shelf,
                drop_hover: false,
                activity: activity.map(|(glyph, text)| Activity { glyph, text }),
                toast: toast.map(|(glyph, text, level)| Toast { glyph, text, level }),
            };
            r.draw(&scene).unwrap();
            save_bmp(&r, &dir.join(format!("{name}.bmp")));
        }
    }
}
