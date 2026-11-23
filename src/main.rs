#![windows_subsystem = "windows"]
mod config;
mod draw;
mod gesture;
mod input;
mod lrc;
mod media;
mod rhythm;
mod shelf;
mod spring;
mod system;

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use config::Config;
use draw::{Activity, FocusView, Hit, Music, Page, Prayers, Renderer, Scene, TOAST_BAR_W, TaskRow, Toast};
use gesture::{Velocity, lands_open, rubber};
use input::{Ask, WM_ASKED};
use media::{Media, NowPlaying, WM_MEDIA};
use rhythm::{Focus, Phase, Task};
use spring::Spring;
use system::{Audio, Power, WM_AUDIO_DEVICE, WM_CAPS, WM_VOLUME};
use windows::Win32::Foundation::*;
use windows::Win32::Globalization::GetDateFormatEx;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{OleInitialize, RevokeDragDrop};
use windows::Win32::System::Power::RegisterPowerSettingNotification;
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::SystemServices::{GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING};
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

const WM_TRAY: u32 = WM_APP + 1;
const WM_APPBAR: u32 = WM_APP + 2;
const WM_CONFIG: u32 = WM_APP + 3;
// Lives in the Win32_UI_Controls feature, which is a lot of bindings for one constant.
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_DWELL: usize = 1;
const TIMER_LEAVE: usize = 2;
const TIMER_CLOCK: usize = 3;
const TIMER_MEDIA: usize = 4;
const TIMER_TOAST: usize = 5;
const TIMER_RHYTHM: usize = 6;
const HOTKEY_ID: i32 = 1;
const JOT_HOTKEY_ID: i32 = 2;
const MUSIC_PAGES: &[Page] = &[Page::Music, Page::Lyrics, Page::Clock, Page::Focus, Page::Todo, Page::Shelf];
const CLOCK_PAGES: &[Page] = &[Page::Clock, Page::Focus, Page::Todo, Page::Shelf];
// Icon glyphs (Segoe Fluent Icons / MDL2)
const STOPWATCH: &str = "\u{E916}";
const MOON: &str = "\u{E708}";
const ALARM: &str = "\u{E823}";
const CHECK: &str = "\u{E73E}";
const NOTE: &str = "\u{E70F}";
const CMD_EXIT: usize = 100;
const CMD_AUTOSTART: usize = 101;
const CMD_SETTINGS: usize = 102;
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

/// Width, height and corner radius in DIPs.
#[derive(Clone, Copy)]
struct Shape {
    w: f32,
    h: f32,
    r: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Pressed, not moved far enough to tell a click from a drag
    Undecided,
    /// Pulling the notch open or pushing it closed
    Vertical,
    /// Swiping between pages
    Pages,
    /// Scrubbing the seek bar
    Seek { left: f32, right: f32 },
}

#[derive(Clone, Copy)]
struct Drag {
    start_x: f32,
    start_y: f32,
    start_p: f32,
    start_page: f32,
    kind: Kind,
    /// What the press landed on: dragging a shelf tile carries the file out instead of swiping.
    hit: Option<Hit>,
}

struct App {
    hwnd: HWND,
    r: Renderer,
    cfg: Config,
    note: Option<String>,
    dpi: u32,
    x: i32,
    y: i32,
    w: Spring,
    h: Spring,
    radius: Spring,
    mix: Spring,
    last: Instant,
    hovered: bool,
    open: bool,
    /// Opened by click or hotkey: stays open, has keyboard focus, closes on Esc or a click elsewhere.
    pinned: bool,
    prev_focus: HWND,
    drag: Option<Drag>,
    vel: Velocity,
    time: String,
    date: String,
    media: Media,
    np: Option<NowPlaying>,
    page: Spring,
    /// Lyric line slide, 1 → 0 each time a new line starts
    slide: Spring,
    line: Option<usize>,
    /// Touchpad horizontal swipes arrive in small steps; a page turns every 120.
    hwheel: i32,
    /// Where we just seeked to, shown until the player reports back.
    seek_hold: Option<(u64, Instant)>,
    audio: Option<Audio>,
    /// (glyph, text, level) of the pop-up showing in the closed pill
    toast: Option<(String, String, Option<f32>)>,
    plugged: Option<bool>,
    battery: Option<u32>,
    focus: Focus,
    tasks: Vec<Task>,
    /// Today's prayer times, worked out once a day (or when settings change).
    prayer_cache: Option<(NaiveDate, Option<[DateTime<Utc>; 5]>)>,
    /// Up to when prayers have been announced.
    prayer_seen: DateTime<Utc>,
    shelf: Vec<String>,
    drop_hover: bool,
    /// The notch opened because files were dragged onto it, so it closes again after.
    drop_opened: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app unless it's already borrowed (a message sent while we're inside another handler).
fn with_app<T>(f: impl FnOnce(&mut App) -> T) -> Option<T> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut a| a.as_mut().map(f)))
}

fn reduced_motion() -> bool {
    let mut on = BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), Default::default());
    }
    !on.as_bool()
}

impl App {
    /// The closed pill hugs what it shows: the clock, or the cover and lyric line while music plays.
    fn closed(&self) -> Shape {
        let t = &self.cfg.theme;
        let fit = |text: &str, side: f32| (self.r.measure(text) + side + 16.0).clamp(t.closed_width, self.r.box_w - 24.0);
        let w = match (&self.toast, self.activity()) {
            (Some((_, _, Some(_))), _) => TOAST_BAR_W.min(self.r.box_w - 24.0),
            (Some((_, text, None)), _) => fit(text, 8.0 + (t.closed_height - 6.0) + 8.0),
            (None, Some((_, text))) => fit(&text, 6.0 + (t.closed_height - 10.0) + 9.0),
            (None, None) => t.closed_width,
        };
        Shape { w, h: t.closed_height, r: t.closed_height / 2.0 }
    }

    fn opened(&self) -> Shape {
        let t = &self.cfg.theme;
        Shape { w: t.open_width, h: t.open_height, r: t.radius }
    }

    /// The springs that make the notch's shape.
    fn springs(&mut self) -> [&mut Spring; 4] {
        [&mut self.w, &mut self.h, &mut self.radius, &mut self.mix]
    }

    fn animating(&self) -> bool {
        ![self.w, self.h, self.radius, self.mix, self.page, self.slide].iter().all(Spring::is_settled)
    }

    fn pages(&self) -> &'static [Page] {
        if self.np.is_some() { MUSIC_PAGES } else { CLOCK_PAGES }
    }

    fn position(&self) -> u64 {
        match (self.seek_hold, &self.np) {
            (Some((ms, at)), _) if at.elapsed() < Duration::from_millis(1500) => ms,
            (_, Some(np)) => np.position(Instant::now()),
            _ => 0,
        }
    }

    /// What the closed pill shows instead of the clock, most important first: a running focus timer,
    /// music (the line being sung, else title and artist; glyph None = album cover), a prayer within 30 minutes.
    fn activity(&self) -> Option<(Option<&'static str>, String)> {
        if self.focus.running() {
            let left = rhythm::minutes_left(self.focus.remaining(Instant::now()));
            let what = if self.focus.phase == Phase::Focus { "Focus" } else { "Break" };
            return Some((Some(STOPWATCH), format!("{what} · {left}")));
        }
        if let Some(np) = self.np.as_ref().filter(|n| n.playing) {
            let line = self.line.and_then(|i| np.lyrics.get(i)).filter(|l| np.lyrics_ready() && !l.text.is_empty());
            return Some((
                None,
                match line {
                    Some(l) => l.text.clone(),
                    None if np.artist.is_empty() => np.title.clone(),
                    None => format!("{} · {}", np.title, np.artist),
                },
            ));
        }
        let now = Utc::now();
        let (i, t) = rhythm::next_prayer(&self.cfg.prayer, now)?;
        (t - now <= chrono::TimeDelta::minutes(30)).then(|| (Some(MOON), format!("{} in {}", self.cfg.prayer.names[i], rhythm::until(t - now))))
    }

    fn page_now(&self) -> Option<Page> {
        self.pages().get(self.page.target.round() as usize).copied()
    }

    fn go_page(&mut self, page: f32) {
        if self.page.is_settled() {
            self.last = Instant::now();
        }
        self.page.target = page.clamp(0.0, (self.pages().len() - 1) as f32).round();
        self.schedule_rhythm(); // the focus page ticks every second while it shows
    }

    /// Picks up a new snapshot from the media thread.
    fn on_media(&mut self) {
        let showing = self.page_now();
        self.np = self.media.snapshot();
        self.seek_hold = None;
        match &self.np {
            Some(n) => self.r.set_cover(&n.key, n.cover.as_ref()),
            None => self.r.set_cover("", None),
        }
        // Music pages come and go; stay on the page you were looking at.
        if let Some(i) = showing.and_then(|p| self.pages().iter().position(|q| *q == p)) {
            (self.page.value, self.page.target) = (i as f32, i as f32);
        } else if self.page.target as usize >= self.pages().len() {
            self.go_page(0.0);
            self.page.value = self.page.target;
        }
        self.update_line(false);
        self.set_open(self.open); // the closed width follows the activity text
        self.render();
        self.schedule();
    }

    /// Moves to the line being sung now; slides when it's simply the next one.
    fn update_line(&mut self, animate: bool) {
        let pos = self.position();
        let line = self.np.as_ref().filter(|n| n.lyrics_ready()).and_then(|n| lrc::current(&n.lyrics, pos));
        if line != self.line {
            // The slide is only visible on the open lyrics page; closed, the line just swaps.
            if animate && self.open && line.is_some() && line == self.line.map(|i| i + 1) {
                if !self.animating() {
                    self.last = Instant::now();
                }
                self.slide.value = 1.0;
            }
            self.line = line;
        }
    }

    /// One timer, set for the next thing that changes on screen: a lyric line, or the seek bar while open.
    fn schedule(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_MEDIA);
        }
        let Some(np) = self.np.as_ref().filter(|n| n.playing) else { return };
        let pos = self.position();
        let mut wait = if self.open { 500 } else { u64::MAX };
        if let Some(next) = np.lyrics.iter().find(|l| l.ms > pos).filter(|_| np.lyrics_ready()) {
            wait = wait.min(next.ms - pos + 15);
        }
        // Word by word, but only where it shows: the open lyrics page.
        if self.open
            && self.page_now() == Some(Page::Lyrics)
            && let Some(word) = self.line.and_then(|i| np.lyrics.get(i)).and_then(|l| l.words.iter().find(|w| w.ms > pos))
        {
            wait = wait.min(word.ms - pos + 15);
        }
        if wait != u64::MAX {
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_MEDIA, wait.clamp(16, 60_000) as u32, None);
            }
        }
    }

    /// A pop-up in the closed pill for `ms`; the pill springs to fit it and back.
    fn show_toast(&mut self, glyph: &str, text: String, level: Option<f32>, ms: u32) {
        self.toast = Some((glyph.to_string(), text, level));
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_TOAST, ms, None);
        }
        self.set_open(self.open);
        self.render();
    }

    /// Today's five prayer times, recalculated when the date changes.
    fn today_prayers(&mut self) -> Option<[DateTime<Utc>; 5]> {
        let today = Local::now().date_naive();
        if self.prayer_cache.as_ref().is_none_or(|(d, _)| *d != today) {
            self.prayer_cache = Some((today, rhythm::prayer_day(&self.cfg.prayer, today)));
        }
        self.prayer_cache.as_ref().and_then(|(_, t)| *t)
    }

    fn task_rows(&self) -> Vec<TaskRow> {
        let today = Local::now().date_naive();
        let mut rows: Vec<TaskRow> =
            (self.tasks.iter().enumerate())
                .map(|(index, t)| {
                    let due = t.due.and_then(|s| Local.timestamp_opt(s, 0).single());
                    let due = due.map_or(String::new(), |d| {
                        if d.date_naive() == today { d.format("%H:%M").to_string() } else { d.format("%a %H:%M").to_string() }
                    });
                    TaskRow { index, text: t.text.clone(), due, done: t.done }
                })
                .collect();
        // Open tasks first, soonest due first; then the done ones.
        rows.sort_by_key(|r| (r.done, self.tasks[r.index].due.unwrap_or(i64::MAX)));
        rows
    }

    /// The box under the notch for a new task or a Jot.
    fn ask(&self, kind: Ask) {
        let scale = self.dpi as f32 / 96.0;
        // Below the notch as it is, so an open to-do list stays visible above the box.
        let h = if self.open { self.cfg.theme.open_height } else { self.cfg.theme.closed_height };
        let top = self.y + ((h + 10.0) * scale) as i32;
        input::ask(self.hwnd, kind, self.x + self.r.px_w / 2, top, self.dpi);
    }

    /// Words typed into the box: a task (a time in it sets a reminder) or a Jot line.
    fn on_asked(&mut self, kind: Ask) {
        let text = input::take();
        match kind {
            Ask::Task => {
                let (words, at) = rhythm::split_time(&text);
                let due = at.map(|t| rhythm::next_at(t, Local::now()));
                let note = due.map_or(String::new(), |d| format!(" · {}", d.format("%H:%M")));
                self.tasks.push(Task { text: words.clone(), due: due.map(|d| d.timestamp()), done: false, reminded: false });
                rhythm::save_tasks(&self.tasks);
                self.show_toast(CHECK, format!("Added: {words}{note}"), None, 2500);
                self.schedule_rhythm();
            }
            Ask::Jot => {
                let file = expand_env(&self.cfg.jot.file);
                match rhythm::jot(&file, &text, Local::now()) {
                    Ok(()) => self.show_toast(NOTE, "Jotted".into(), None, 1600),
                    Err(e) => self.show_toast(NOTE, format!("Couldn't jot: {e}"), None, 5000),
                }
            }
        }
    }

    /// Files dragged over the notch: open on the shelf, take them on drop, close again if the drag opened it.
    fn on_shelf(&mut self, what: usize) {
        match what {
            shelf::ENTER => {
                if !self.open {
                    self.drop_opened = true;
                    self.set_open(true);
                }
                if let Some(i) = self.pages().iter().position(|p| *p == Page::Shelf) {
                    self.go_page(i as f32);
                }
                self.drop_hover = true;
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_LEAVE);
                }
            }
            shelf::LEAVE => {
                self.drop_hover = false;
                if std::mem::take(&mut self.drop_opened) && !self.pinned {
                    self.set_open(false);
                }
            }
            _ => {
                self.drop_hover = false;
                shelf::add(&mut self.shelf, shelf::take_dropped());
                shelf::save(&self.shelf);
                // Show what landed for a moment, then tuck away.
                if std::mem::take(&mut self.drop_opened) {
                    unsafe {
                        SetTimer(Some(self.hwnd), TIMER_LEAVE, 1500, None);
                    }
                }
            }
        }
        self.render();
    }

    /// Wakes for whatever is due: a focus phase ending, a task reminder, a prayer.
    fn tick_rhythm(&mut self) {
        let (now_i, now) = (Instant::now(), Utc::now());
        if self.focus.running() && self.focus.remaining(now_i).is_zero() {
            let text = match self.focus.advance(&self.cfg.focus, now_i) {
                Some(Phase::Focus) => format!("Back to focus · round {}", self.focus.round),
                Some(Phase::Short) => format!("Short break · {} min", self.cfg.focus.short_break),
                Some(Phase::Long) => format!("Long break · {} min", self.cfg.focus.long_break),
                None => "Focus cycle done. Well done!".into(),
            };
            self.show_toast(STOPWATCH, text, None, 5000);
        }
        let mut changed = false;
        for i in 0..self.tasks.len() {
            let t = &self.tasks[i];
            if !t.done && !t.reminded && t.due.is_some_and(|d| d <= now.timestamp()) {
                let text = t.text.clone();
                self.tasks[i].reminded = true;
                changed = true;
                self.show_toast(ALARM, text, None, 10_000);
            }
        }
        if changed {
            rhythm::save_tasks(&self.tasks);
        }
        // A prayer time passed since the last look. Not if it was long ago (the PC slept through it).
        if let Some((i, t)) = rhythm::next_prayer(&self.cfg.prayer, self.prayer_seen)
            && t <= now
        {
            if now - t < chrono::TimeDelta::minutes(10) {
                let name = self.cfg.prayer.names[i].clone();
                self.show_toast(MOON, format!("{name} · {}", t.with_timezone(&Local).format("%H:%M")), None, 10_000);
                if self.cfg.prayer.pause_music && self.np.as_ref().is_some_and(|n| n.playing) {
                    self.media.pause();
                }
            }
            self.prayer_seen = t;
        } else {
            self.prayer_seen = now;
        }
        self.set_open(self.open);
        self.render();
        self.schedule_rhythm();
    }

    /// One timer for the next rhythm event, plus the minute ticks of countdowns on screen. None when nothing's on.
    fn schedule_rhythm(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_RHYTHM);
        }
        let (now_i, now) = (Instant::now(), Utc::now());
        let mut wait = u64::MAX;
        if self.focus.running() {
            let left = self.focus.remaining(now_i).as_millis() as u64;
            let seconds = (self.open && self.page_now() == Some(Page::Focus)) || left <= 60_000;
            wait = left.min(if seconds { 1000 } else { left % 60_000 + 1 });
        }
        for t in self.tasks.iter().filter(|t| !t.done && !t.reminded) {
            if let Some(d) = t.due {
                wait = wait.min((d * 1000 - now.timestamp_millis()).max(0) as u64);
            }
        }
        if let Some((_, t)) = rhythm::next_prayer(&self.cfg.prayer, now) {
            let ms = (t - now).num_milliseconds().max(0) as u64;
            // Outside the last 30 minutes, wake when they start; inside, every minute for "Asr in 12 min".
            wait = wait.min(if ms > 30 * 60_000 { ms - 30 * 60_000 } else { ms.min(ms % 60_000 + 1) });
            // Long sleeps drift (the PC may sleep in between); check back at least every 10 minutes.
            wait = wait.min(10 * 60_000);
        }
        if wait != u64::MAX {
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_RHYTHM, wait.clamp(50, 10 * 60_000) as u32, None);
            }
        }
    }

    fn on_power(&mut self, p: Power) {
        match p {
            Power::Plugged(on) => {
                // The first report is just the state at startup, not a change.
                if self.plugged.is_some_and(|was| was != on) {
                    let pct = self.battery.map_or(String::new(), |b| format!(" · {b}%"));
                    let glyph = system::battery_glyph(self.battery.unwrap_or(100), on);
                    self.show_toast(&glyph, format!("{}{pct}", if on { "Charging" } else { "On battery" }), None, 2500);
                }
                self.plugged = Some(on);
            }
            Power::Percent(now) => {
                if let (Some(old), Some(false)) = (self.battery, self.plugged)
                    && system::crossed_low(old, now)
                {
                    self.show_toast(&system::battery_glyph(now, false), format!("Battery low · {now}%"), None, 5000);
                }
                self.battery = Some(now);
            }
        }
    }

    fn act(&mut self, hit: Hit, x: f32) {
        match hit {
            Hit::Previous => self.media.previous(),
            Hit::Next => self.media.next(),
            Hit::PlayPause => self.media.toggle(),
            Hit::Seek { left, right } => self.seek_to_x(x, left, right),
            Hit::Line(ms) => self.seek(ms),
            Hit::Page(i) => self.go_page(i as f32),
            Hit::FocusToggle => self.focus.toggle(Instant::now()),
            Hit::FocusSkip => {
                if self.focus.started {
                    self.focus.advance(&self.cfg.focus, Instant::now());
                } else {
                    self.focus.toggle(Instant::now());
                }
            }
            Hit::FocusReset => self.focus = Focus::new(&self.cfg.focus),
            Hit::AddTask => self.ask(Ask::Task),
            Hit::Task(i) => {
                if let Some(t) = self.tasks.get_mut(i) {
                    t.done = !t.done;
                }
                rhythm::save_tasks(&self.tasks);
            }
            Hit::ClearDone => {
                self.tasks.retain(|t| !t.done);
                rhythm::save_tasks(&self.tasks);
            }
            Hit::ShelfItem(i) => {
                let Some(path) = self.shelf.get(i).cloned() else { return };
                let ok =
                    unsafe { ShellExecuteW(Some(self.hwnd), w!("open"), &HSTRING::from(path.as_str()), None, None, SW_SHOWNORMAL).0 as usize > 32 };
                if !ok {
                    // Moved or deleted since it was shelved.
                    self.shelf.remove(i);
                    shelf::save(&self.shelf);
                    self.show_toast("\u{E783}", format!("Can't open {}", shelf::name(&path)), None, 3000);
                }
            }
            Hit::ShelfRemove(i) => {
                if i < self.shelf.len() {
                    self.shelf.remove(i);
                    shelf::save(&self.shelf);
                }
            }
            Hit::ShelfClear => {
                self.shelf.clear();
                shelf::save(&self.shelf);
            }
        }
        if matches!(hit, Hit::ShelfRemove(_) | Hit::ShelfClear) {
            self.render();
        }
        if matches!(hit, Hit::FocusToggle | Hit::FocusSkip | Hit::FocusReset | Hit::Task(_) | Hit::ClearDone) {
            self.schedule_rhythm();
            self.render();
        }
    }

    fn seek(&mut self, ms: u64) {
        self.media.seek(ms);
        self.seek_hold = Some((ms, Instant::now()));
        self.update_line(false);
        self.render();
    }

    fn seek_x_ms(&self, x: f32, left: f32, right: f32) -> u64 {
        let d = self.np.as_ref().map_or(0, |n| n.duration_ms);
        (((x - left) / (right - left)).clamp(0.0, 1.0) * d as f32) as u64
    }

    fn seek_to_x(&mut self, x: f32, left: f32, right: f32) {
        let ms = self.seek_x_ms(x, left, right);
        self.seek(ms);
    }

    /// Springy normally; when Windows animations are off, a quick critically damped move with no bounce.
    fn tune_springs(&mut self) {
        let (k, d) = if reduced_motion() { (600.0, 49.0) } else { (380.0, 26.0) };
        for s in [&mut self.w, &mut self.h, &mut self.radius] {
            (s.stiffness, s.damping) = (k, d);
        }
    }

    fn set_open(&mut self, open: bool) {
        if !self.animating() {
            self.last = Instant::now();
        }
        let changed = self.open != open;
        self.open = open;
        let s = if open { self.opened() } else { self.closed() };
        (self.w.target, self.h.target, self.radius.target) = (s.w, s.h, s.r);
        self.mix.target = if open { 1.0 } else { 0.0 };
        if changed {
            // The seek bar and the focus seconds only tick while open.
            self.schedule();
            self.schedule_rhythm();
        }
    }

    /// Hover never takes focus; a pinned notch needs it for the keyboard and to notice clicks elsewhere.
    fn activatable(&self, on: bool) {
        unsafe {
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let no = WS_EX_NOACTIVATE.0 as isize;
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, if on { ex & !no } else { ex | no });
        }
    }

    fn pin(&mut self) {
        unsafe {
            if !self.pinned {
                self.prev_focus = GetForegroundWindow();
            }
            self.pinned = true;
            self.set_open(true);
            self.activatable(true);
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    /// Closes a pinned notch; `restore` hands focus back to the window you were in (Esc, hotkey).
    fn unpin(&mut self, restore: bool) {
        let was = std::mem::take(&mut self.pinned);
        self.activatable(false);
        self.set_open(false);
        if was && restore && !self.prev_focus.is_invalid() {
            unsafe {
                let _ = SetForegroundWindow(self.prev_focus);
            }
        }
    }

    /// Shape for a drag position on the closed (0) .. open (1) axis.
    fn follow(&mut self, p: f32) {
        let (c, o) = (self.closed(), self.opened());
        let lerp = |a: f32, b: f32| a + (b - a) * p;
        let values = [lerp(c.w, o.w), lerp(c.h, o.h), lerp(c.r, o.r), p.clamp(0.0, 1.0)];
        for (s, v) in self.springs().into_iter().zip(values) {
            (s.value, s.target, s.velocity) = (v, v, 0.0);
        }
    }

    /// Where the hand is on the closed (0) .. open (1) axis, before any edge resistance.
    fn hand(&self, d: Drag, y: f32) -> f32 {
        d.start_p + (y - d.start_y) / (self.opened().h - self.closed().h)
    }

    /// Past either end the notch follows the hand less and less.
    fn resist(p: f32) -> f32 {
        if p > 1.0 {
            1.0 + rubber(p - 1.0, 0.08)
        } else if p < 0.0 {
            rubber(p, 0.05)
        } else {
            p
        }
    }

    /// Let go at progress `p`: land open or closed, carrying the hand's speed into the springs.
    fn release(&mut self, p: f32) {
        self.drag = None;
        let now = Instant::now();
        let pv = self.vel.per_second(now);
        let open = lands_open(p, pv);
        let (c, o) = (self.closed(), self.opened());
        let deltas = [o.w - c.w, o.h - c.h, o.r - c.r, 1.0];
        for (s, delta) in self.springs().into_iter().zip(deltas) {
            s.velocity = pv * delta;
        }
        self.last = now;
        if open {
            self.pin();
        } else {
            self.unpin(true);
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32();
        self.last = now;
        for s in [&mut self.w, &mut self.h, &mut self.radius, &mut self.mix, &mut self.page, &mut self.slide] {
            s.step(dt);
        }
    }

    fn render(&mut self) {
        let activity = self.activity();
        let pos = self.position();
        let now_i = Instant::now();
        let f = &self.focus;
        let left = f.remaining(now_i);
        let focus_left = format!("{}:{:02}", left.as_secs() / 60, left.as_secs() % 60);
        let focus_label = f.label(&self.cfg.focus);
        let focus = FocusView {
            label: &focus_label,
            left: &focus_left,
            fraction: left.as_secs_f32() / rhythm::length(&self.cfg.focus, f.phase).as_secs_f32(),
            running: f.running(),
            on_break: f.phase != Phase::Focus,
            // A break comes after its focus round, so that round counts as done.
            done: if f.phase == Phase::Focus { f.round - 1 } else { f.round },
            rounds: self.cfg.focus.rounds,
        };
        let tasks = self.task_rows();
        let now = Utc::now();
        let prayers = self.today_prayers().map(|times| {
            let (next, coming) = match rhythm::next_prayer(&self.cfg.prayer, now) {
                Some((i, t)) => {
                    (times.iter().position(|x| *x == t).unwrap_or(5), format!("{} in {}", self.cfg.prayer.names[i], rhythm::until(t - now)))
                }
                None => (5, String::new()),
            };
            Prayers { names: &self.cfg.prayer.names, times: times.map(|t| t.with_timezone(&Local).format("%H:%M").to_string()), next, coming }
        });
        let music = self.np.as_ref().map(|n| Music {
            title: &n.title,
            artist: &n.artist,
            pos_ms: pos,
            duration_ms: n.duration_ms,
            playing: n.playing,
            can_seek: n.can_seek,
            accent: n.cover.as_ref().map_or([0.92, 0.92, 0.92], |c| c.accent),
            lines: &n.lyrics,
            line: self.line,
            lit: self.line.and_then(|i| n.lyrics.get(i)).map_or(0, |l| l.words.iter().filter(|w| w.ms <= pos).count()),
            lyrics_ready: n.lyrics_ready(),
            slide: self.slide.value,
        });
        let scene = Scene {
            w: self.w.value,
            h: self.h.value,
            radius: self.radius.value.max(0.0),
            open: self.mix.value,
            pages: self.pages(),
            page: self.page.value,
            time: &self.time,
            date: &self.date,
            note: self.note.as_deref(),
            music,
            focus,
            tasks: &tasks,
            prayers,
            shelf: &self.shelf,
            drop_hover: self.drop_hover,
            activity: activity.as_ref().map(|(glyph, text)| Activity { glyph: *glyph, text }),
            toast: self.toast.as_ref().map(|(glyph, text, level)| Toast { glyph, text, level: *level }),
        };
        if self.r.draw(&scene).is_ok() {
            let _ = self.r.present(self.hwnd, self.x, self.y);
        }
    }

    /// Top-center of the primary monitor. Called at start and whenever displays or DPI change.
    fn place(&mut self) {
        unsafe {
            let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let m = mi.rcMonitor;
            (self.x, self.y) = (m.left + (m.right - m.left - self.r.px_w) / 2, m.top);
        }
    }

    fn rebuild(&mut self) {
        if let Ok(r) = Renderer::new(self.dpi, &self.cfg.theme) {
            self.r = r;
            if let Some(n) = &self.np {
                self.r.set_cover(&n.key, n.cover.as_ref());
            }
        }
        self.place();
        self.set_open(self.open);
        self.render();
    }

    fn reload_config(&mut self) {
        let (cfg, note) = Config::load();
        self.note = note;
        if cfg != self.cfg {
            self.cfg = cfg;
            register_hotkey(self.hwnd, &self.cfg);
            self.prayer_cache = None;
            self.schedule_rhythm();
        }
        self.rebuild();
    }

    /// Client pixels from a mouse message → box DIPs.
    fn to_dip(&self, lp: LPARAM) -> (f32, f32) {
        let k = 96.0 / self.dpi as f32;
        ((lp.0 & 0xFFFF) as i16 as f32 * k, ((lp.0 >> 16) & 0xFFFF) as i16 as f32 * k)
    }

    fn press(&mut self, x: f32, y: f32) {
        let hit = self.open.then(|| self.r.hit(x, y)).flatten();
        let seek = match hit {
            Some(Hit::Seek { left, right }) => Some(Kind::Seek { left, right }),
            _ => None,
        };
        let start_p = if self.open { 1.0 } else { 0.0 };
        self.drag = Some(Drag { start_x: x, start_y: y, start_p, start_page: self.page.target, kind: seek.unwrap_or(Kind::Undecided), hit });
        if let Some(Kind::Seek { left, right }) = seek {
            self.seek_hold = Some((self.seek_x_ms(x, left, right), Instant::now() + Duration::from_secs(3600)));
            self.render();
        }
        self.vel.reset();
    }

    fn drag_to(&mut self, x: f32, y: f32) {
        let Some(mut d) = self.drag else { return };
        let now = Instant::now();
        let (dx, dy) = (x - d.start_x, y - d.start_y);
        if d.kind == Kind::Undecided && dx.hypot(dy) > 4.0 {
            if let Some(Hit::ShelfItem(i)) = d.hit {
                // OLE runs its own drag loop; start it from the message loop, not from inside this handler.
                self.drag = None;
                unsafe {
                    let _ = PostMessageW(Some(self.hwnd), shelf::WM_SHELF_OUT, WPARAM(i), LPARAM(0));
                }
                return;
            }
            let swipe = self.open && self.pages().len() > 1 && dx.abs() > dy.abs();
            d.kind = if swipe { Kind::Pages } else { Kind::Vertical };
            self.vel.push(now, if swipe { d.start_page } else { d.start_p });
        }
        self.drag = Some(d);
        match d.kind {
            Kind::Undecided => return,
            Kind::Vertical => {
                let p = self.hand(d, y);
                self.vel.push(now, p);
                self.follow(App::resist(p));
            }
            Kind::Pages => {
                let last = (self.pages().len() - 1) as f32;
                let p = d.start_page - dx / self.opened().w;
                self.vel.push(now, p);
                let shown = if p < 0.0 {
                    rubber(p, 0.15)
                } else if p > last {
                    last + rubber(p - last, 0.15)
                } else {
                    p
                };
                (self.page.value, self.page.target, self.page.velocity) = (shown, shown, 0.0);
            }
            Kind::Seek { left, right } => {
                self.seek_hold = Some((self.seek_x_ms(x, left, right), now + Duration::from_secs(3600)));
            }
        }
        self.render();
    }

    /// Mouse up: a click, or the end of a drag.
    fn let_go(&mut self, x: f32, y: f32) {
        let Some(d) = self.drag.take() else { return };
        let now = Instant::now();
        match d.kind {
            Kind::Undecided => match self.open.then(|| self.r.hit(x, y)).flatten() {
                Some(hit) => self.act(hit, x),
                None if self.pinned => self.unpin(true),
                None => self.pin(),
            },
            Kind::Vertical => {
                self.drag = Some(d);
                self.release(App::resist(self.hand(d, y)));
            }
            Kind::Pages => {
                // Project the flick, then land on a neighbouring page at most.
                let v = self.vel.per_second(now);
                let target = (self.page.value + v * 0.12).round().clamp(d.start_page - 1.0, d.start_page + 1.0);
                self.page.velocity = v;
                self.last = now;
                self.go_page(target);
            }
            Kind::Seek { left, right } => self.seek_to_x(x, left, right),
        }
    }

    /// Refreshes the clock strings; returns milliseconds until the next minute.
    fn update_clock(&mut self) -> u32 {
        let t = unsafe { GetLocalTime() };
        self.time = format!("{:02}:{:02}", t.wHour, t.wMinute);
        let mut buf = [0u16; 64];
        let n = unsafe { GetDateFormatEx(PCWSTR::null(), Default::default(), Some(&t), w!("dddd, d MMMM"), Some(&mut buf), PCWSTR::null()) };
        self.date = String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize]);
        (60 - t.wSecond as u32) * 1000 - t.wMilliseconds as u32
    }
}

fn register_hotkey(hwnd: HWND, cfg: &Config) {
    for (id, keys) in [(HOTKEY_ID, &cfg.hotkey), (JOT_HOTKEY_ID, &cfg.jot.hotkey)] {
        unsafe {
            let _ = UnregisterHotKey(Some(hwnd), id);
            if let Some((mods, vk)) = config::parse_hotkey(keys) {
                let _ = RegisterHotKey(Some(hwnd), id, HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT, vk);
            }
        }
    }
}

/// "%USERPROFILE%\Documents\Jot.md" → the real path.
fn expand_env(s: &str) -> PathBuf {
    let src = HSTRING::from(s);
    let mut buf = vec![0u16; 1024];
    let n = unsafe { windows::Win32::System::Environment::ExpandEnvironmentStringsW(&src, Some(&mut buf)) } as usize;
    match n {
        1..=1024 => PathBuf::from(String::from_utf16_lossy(&buf[..n - 1])),
        _ => PathBuf::from(s),
    }
}

fn autostart() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, w!("notch"), RRF_RT_REG_SZ, None, None, None).is_ok() }
}

fn set_autostart(on: bool) {
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return };
            let value: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("notch"), REG_SZ.0, Some(value.as_ptr().cast()), (value.len() * 2) as u32);
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("notch"));
        }
    }
}

fn open_settings() {
    let Some(path) = config::path() else { return };
    if !path.exists() {
        Config::load(); // writes the commented defaults
    }
    let path = HSTRING::from(path.as_os_str());
    unsafe {
        // Opens in your .toml editor; Notepad if nothing is associated.
        if ShellExecuteW(None, w!("open"), &path, None, None, SW_SHOWNORMAL).0 as usize <= 32 {
            ShellExecuteW(None, w!("open"), w!("notepad.exe"), &path, None, SW_SHOWNORMAL);
        }
    }
}

/// Posts WM_CONFIG whenever something in the config folder changes. Blocks in the kernel, so it costs nothing while idle.
fn watch_config(hwnd: HWND) {
    let Some(dir) = config::path().and_then(|p| p.parent().map(PathBuf::from)) else { return };
    let _ = std::fs::create_dir_all(&dir);
    let hwnd = hwnd.0 as isize; // HWND isn't Send; the handle value is
    std::thread::spawn(move || unsafe {
        let dir = HSTRING::from(dir.as_os_str());
        let Ok(h) = FindFirstChangeNotificationW(&dir, false, FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME) else { return };
        while WaitForSingleObject(h, INFINITE) == WAIT_OBJECT_0 {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_CONFIG, WPARAM(0), LPARAM(0));
            if FindNextChangeNotification(h).is_err() {
                break;
            }
        }
    });
}

/// Registers as an app bar without taking any space, only to hear ABN_FULLSCREENAPP when a full-screen app comes and goes.
fn appbar(hwnd: HWND, add: bool) {
    let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: hwnd, uCallbackMessage: WM_APPBAR, ..Default::default() };
    unsafe {
        SHAppBarMessage(if add { ABM_NEW } else { ABM_REMOVE }, &mut abd);
    }
}

fn tray(hwnd: HWND, add: bool) {
    unsafe {
        let icon = if add { draw::tray_icon(GetDpiForWindow(hwnd)) } else { None };
        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: icon.or_else(|| LoadIconW(None, IDI_APPLICATION).ok()).unwrap_or_default(),
            ..Default::default()
        };
        let tip: Vec<u16> = "notch".encode_utf16().collect();
        nid.szTip[..tip.len()].copy_from_slice(&tip);
        let _ = Shell_NotifyIconW(if add { NIM_ADD } else { NIM_DELETE }, &nid);
        // The notification area keeps its own copy.
        if let Some(i) = icon {
            let _ = DestroyIcon(i);
        }
    }
}

fn tray_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let check = if autostart() { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING | check, CMD_AUTOSTART, w!("Start with Windows"));
        let _ = AppendMenuW(menu, MF_STRING, CMD_SETTINGS, w!("Edit settings…"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, CMD_EXIT, w!("Exit"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Without this the menu doesn't close when you click elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY, pt.x, pt.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
        match cmd.0 as usize {
            CMD_EXIT => {
                let _ = DestroyWindow(hwnd);
            }
            CMD_AUTOSTART => set_autostart(!autostart()),
            CMD_SETTINGS => open_settings(),
            _ => {}
        }
    }
}

fn taskbar_created() -> u32 {
    static ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *ID.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEMOVE => {
                with_app(|a| {
                    if a.drag.is_some() {
                        let (x, y) = a.to_dip(lp);
                        return a.drag_to(x, y);
                    }
                    if !a.hovered {
                        a.hovered = true;
                        let mut tme =
                            TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                        let _ = TrackMouseEvent(&mut tme);
                        let _ = KillTimer(Some(hwnd), TIMER_LEAVE);
                        if !a.open {
                            SetTimer(Some(hwnd), TIMER_DWELL, 120, None);
                        }
                    }
                });
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                with_app(|a| {
                    a.hovered = false;
                    let _ = KillTimer(Some(hwnd), TIMER_DWELL);
                    if a.open && !a.pinned && a.drag.is_none() {
                        SetTimer(Some(hwnd), TIMER_LEAVE, 300, None);
                    }
                });
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                with_app(|a| {
                    let (x, y) = a.to_dip(lp);
                    a.press(x, y);
                    SetCapture(hwnd);
                });
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                with_app(|a| {
                    let (x, y) = a.to_dip(lp);
                    a.let_go(x, y);
                });
                // After the drag is settled, so the WM_CAPTURECHANGED this sends finds nothing to do.
                let _ = ReleaseCapture();
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                // Capture taken away mid-drag (e.g. a window popped up): settle from wherever things are.
                with_app(|a| match a.drag.map(|d| d.kind) {
                    Some(Kind::Vertical) => a.release(a.mix.value),
                    Some(Kind::Pages) => {
                        a.drag = None;
                        a.go_page(a.page.value);
                    }
                    Some(Kind::Seek { .. }) => {
                        a.drag = None;
                        a.seek_hold = None;
                        a.render();
                    }
                    _ => a.drag = None,
                });
                LRESULT(0)
            }
            WM_MOUSEHWHEEL => {
                // Two-finger horizontal swipe on a touchpad, or a tilt wheel: turn pages.
                with_app(|a| {
                    if !a.open {
                        return;
                    }
                    a.hwheel += (wp.0 >> 16) as i16 as i32;
                    if a.hwheel.abs() >= 120 {
                        let step = a.hwheel.signum() as f32;
                        a.hwheel = 0;
                        a.go_page(a.page.target + step);
                    }
                });
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(if with_app(|a| a.pinned).unwrap_or(false) { MA_ACTIVATE } else { MA_NOACTIVATE } as isize),
            WM_ACTIVATE => {
                // Clicked somewhere else while pinned: let go (focus already moved, nothing to restore).
                if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE {
                    with_app(|a| {
                        if a.pinned {
                            a.unpin(false);
                        }
                    });
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                with_app(|a| match VIRTUAL_KEY(wp.0 as u16) {
                    VK_ESCAPE => a.unpin(true),
                    VK_LEFT => a.go_page(a.page.target - 1.0),
                    VK_RIGHT => a.go_page(a.page.target + 1.0),
                    VK_SPACE => a.media.toggle(),
                    _ => {}
                });
                LRESULT(0)
            }
            WM_MEDIA => {
                with_app(|a| a.on_media());
                LRESULT(0)
            }
            WM_VOLUME => {
                let (level, muted) = ((wp.0 & 0xFFFF) as f32 / 1000.0, wp.0 >> 16 & 1 == 1);
                with_app(|a| {
                    a.show_toast(
                        system::volume_glyph(level, muted),
                        format!("{}", (level * 100.0).round()),
                        Some(if muted { 0.0 } else { level }),
                        1600,
                    )
                });
                LRESULT(0)
            }
            WM_AUDIO_DEVICE => {
                with_app(|a| {
                    if let Some(name) = a.audio.as_mut().and_then(|au| au.follow_default()) {
                        let l = name.to_lowercase();
                        let ears = ["headphone", "headset", "earbud", "buds", "airpods"].iter().any(|w| l.contains(w));
                        a.show_toast(if ears { "\u{E7F6}" } else { "\u{E7F5}" }, name, None, 2500);
                    }
                });
                LRESULT(0)
            }
            WM_CAPS => {
                // The hook fires on key up, after Windows has flipped the toggle.
                let on = GetKeyState(VK_CAPITAL.0 as i32) & 1 == 1;
                with_app(|a| a.show_toast("\u{E765}", format!("Caps Lock {}", if on { "on" } else { "off" }), None, 1600));
                LRESULT(0)
            }
            WM_POWERBROADCAST => {
                if wp.0 as u32 == PBT_POWERSETTINGCHANGE
                    && let Some(p) = system::power_setting(lp)
                {
                    with_app(|a| a.on_power(p));
                }
                LRESULT(1)
            }
            WM_MOUSEWHEEL => {
                // Scroll on the closed pill: volume, 2% a notch. Scrolling means "not peeking", so no hover-open.
                with_app(|a| {
                    if !a.open {
                        let _ = KillTimer(Some(hwnd), TIMER_DWELL);
                        let notches = (wp.0 >> 16) as i16 as f32 / 120.0;
                        if let Some(au) = &a.audio {
                            au.nudge(notches * 0.02);
                        }
                    }
                });
                LRESULT(0)
            }
            WM_HOTKEY => {
                match wp.0 as i32 {
                    JOT_HOTKEY_ID => with_app(|a| a.ask(Ask::Jot)),
                    _ => with_app(|a| if a.pinned { a.unpin(true) } else { a.pin() }),
                };
                LRESULT(0)
            }
            shelf::WM_SHELF => {
                with_app(|a| a.on_shelf(wp.0));
                LRESULT(0)
            }
            shelf::WM_SHELF_OUT => {
                // Outside any borrow: SHDoDragDrop runs a modal loop that keeps dispatching our messages.
                let path = with_app(|a| a.shelf.get(wp.0).cloned()).flatten();
                let _ = ReleaseCapture();
                if let Some(p) = path {
                    let _ = shelf::drag_out(hwnd, &p);
                }
                LRESULT(0)
            }
            WM_ASKED => {
                let kind = if wp.0 == Ask::Jot as usize { Ask::Jot } else { Ask::Task };
                with_app(|a| a.on_asked(kind));
                LRESULT(0)
            }
            WM_TIMER => {
                match wp.0 {
                    TIMER_DWELL => {
                        let _ = KillTimer(Some(hwnd), TIMER_DWELL);
                        with_app(|a| {
                            if a.hovered && !a.open {
                                a.set_open(true)
                            }
                        });
                    }
                    TIMER_LEAVE => {
                        let _ = KillTimer(Some(hwnd), TIMER_LEAVE);
                        with_app(|a| {
                            if !a.hovered && !a.pinned {
                                a.set_open(false)
                            }
                        });
                    }
                    TIMER_CLOCK => {
                        with_app(|a| {
                            let ms = a.update_clock();
                            SetTimer(Some(hwnd), TIMER_CLOCK, ms, None);
                            a.render();
                        });
                    }
                    TIMER_RHYTHM => {
                        with_app(|a| a.tick_rhythm());
                    }
                    TIMER_TOAST => {
                        let _ = KillTimer(Some(hwnd), TIMER_TOAST);
                        with_app(|a| {
                            a.toast = None;
                            a.set_open(a.open);
                            a.render();
                        });
                    }
                    TIMER_MEDIA => {
                        with_app(|a| {
                            a.update_line(true);
                            a.set_open(a.open); // a new lyric line resizes the closed pill
                            a.render();
                            a.schedule();
                        });
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_APPBAR => {
                if wp.0 as u32 == ABN_FULLSCREENAPP {
                    let _ = ShowWindow(hwnd, if lp.0 != 0 { SW_HIDE } else { SW_SHOWNOACTIVATE });
                }
                LRESULT(0)
            }
            WM_CONFIG => {
                with_app(|a| a.reload_config());
                LRESULT(0)
            }
            WM_DPICHANGED => {
                with_app(|a| {
                    a.dpi = (wp.0 >> 16) as u32 & 0xFFFF;
                    a.rebuild();
                });
                LRESULT(0)
            }
            WM_DISPLAYCHANGE => {
                with_app(|a| {
                    a.dpi = GetDpiForWindow(hwnd);
                    a.rebuild();
                });
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                with_app(|a| a.tune_springs());
                LRESULT(0)
            }
            WM_TRAY => {
                let event = (lp.0 & 0xFFFF) as u32;
                if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                    tray_menu(hwnd);
                } else if event == WM_LBUTTONUP {
                    with_app(|a| a.pin());
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                tray(hwnd, false);
                appbar(hwnd, false);
                let _ = RevokeDragDrop(hwnd);
                PostQuitMessage(0);
                LRESULT(0)
            }
            m if m == taskbar_created() => {
                // Explorer restarted: our tray icon and app bar registration went with it.
                tray(hwnd, true);
                appbar(hwnd, true);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn log_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("notch"))
}

/// One line per crash in %LOCALAPPDATA%\notch\crash.log. Never sent anywhere.
fn log_crash(what: &str) {
    use std::io::Write;
    let Some(dir) = log_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(f, "{secs} v{} {what}", env!("CARGO_PKG_VERSION"));
    }
}

fn main() {
    std::panic::set_hook(Box::new(|info| log_crash(&info.to_string())));
    if let Err(e) = run() {
        log_crash(&e.to_string());
    }
}

fn run() -> Result<()> {
    unsafe {
        // One notch at a time; a second launch just exits.
        let _single = CreateMutexW(None, true, w!("Local\\notch-single-instance"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // An STA with OLE: drag and drop needs it, and WinRT media controls are happy in it.
        let _ = OleInitialize(None);
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("NotchWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("notch"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )?;

        let (cfg, note) = Config::load();
        let dpi = GetDpiForWindow(hwnd);
        let r = Renderer::new(dpi, &cfg.theme)?;
        let shape = |v| Spring::new(v, 380.0, 26.0);
        let t = &cfg.theme;
        let cfg_focus = cfg.focus.clone();
        let mut app = App {
            hwnd,
            r,
            dpi,
            x: 0,
            y: 0,
            w: shape(t.closed_width),
            h: shape(t.closed_height),
            radius: shape(t.closed_height / 2.0),
            mix: Spring { eps: 0.004, ..Spring::new(0.0, 300.0, 30.0) },
            cfg,
            note,
            last: Instant::now(),
            hovered: false,
            open: false,
            pinned: false,
            prev_focus: HWND::default(),
            drag: None,
            vel: Velocity::default(),
            time: String::new(),
            date: String::new(),
            media: Media::start(hwnd),
            np: None,
            page: Spring { eps: 0.002, ..Spring::new(0.0, 300.0, 30.0) },
            slide: Spring { eps: 0.004, ..Spring::new(0.0, 220.0, 26.0) },
            line: None,
            hwheel: 0,
            seek_hold: None,
            audio: Audio::start(hwnd).ok(),
            toast: None,
            plugged: None,
            battery: None,
            focus: Focus::new(&cfg_focus),
            tasks: rhythm::load_tasks(),
            prayer_cache: None,
            prayer_seen: Utc::now(),
            shelf: shelf::load(),
            drop_hover: false,
            drop_opened: false,
        };
        let _ = shelf::accept_drops(hwnd);
        app.schedule_rhythm();
        app.tune_springs();
        app.place();
        let ms = app.update_clock();
        SetTimer(Some(hwnd), TIMER_CLOCK, ms, None);
        app.render();
        register_hotkey(hwnd, &app.cfg);
        APP.with(|a| *a.borrow_mut() = Some(app));
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        taskbar_created();
        tray(hwnd, true);
        appbar(hwnd, true);
        watch_config(hwnd);
        // Each sends its current value right away (which we take as the starting state), then changes.
        for setting in [GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING] {
            let _ = RegisterPowerSettingNotification(HANDLE(hwnd.0), &setting, DEVICE_NOTIFY_WINDOW_HANDLE);
        }
        system::watch_caps_lock(hwnd, instance);

        // Frames only while a spring moves; otherwise block in GetMessage (0% CPU).
        let mut msg = MSG::default();
        loop {
            if with_app(|a| a.animating()).unwrap_or(false) {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        return Ok(());
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                with_app(|a| {
                    a.tick();
                    a.render();
                });
                if DwmFlush().is_err() {
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
            } else {
                if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    return Ok(());
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
