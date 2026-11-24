// The open pages, one function each. They're laid out for the open size (theme open_width × open_height, 540 × 160
// by default): an 18 DIP margin, the header on top (draw.rs), content from y 38 to about 140, the page dots below.
// Each page has a left block (1) and a right block (2) that enter one after the other as the notch opens.
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

use crate::draw::*;

const TOP: f32 = 38.0;
const PAD: f32 = 18.0;
const PLAY: &str = "\u{E768}";
const PAUSE: &str = "\u{E769}";
const PREVIOUS: &str = "\u{E892}";
const NEXT: &str = "\u{E893}";
const LIST: &str = "\u{E8FD}";
const ADD: &str = "\u{E710}";
const UNDO: &str = "\u{E7A7}";
const RESET: &str = "\u{E72C}";
const CHECK: &str = "\u{E73E}";
const CLOSE: &str = "\u{E711}";
const DELETE: &str = "\u{E74D}";
const FILE: &str = "\u{E7C3}";
const SHELF: &str = "\u{E896}";
/// A water drop
pub const DROP: &str = "\u{EB42}";

impl Renderer {
    pub(crate) fn page(&self, s: &Scene, page: Page, x: f32, o: f32) {
        if o <= 0.01 {
            return;
        }
        match page {
            Page::Music => self.music_page(s, x, o),
            Page::Focus => self.focus_page(s, x, o),
            Page::Prayer => self.prayer_page(s, x, o),
            Page::Water => self.water_page(s, x, o),
            Page::Todo => self.todo_page(s, x, o),
            Page::Widgets => self.widgets_page(s, x, o),
            Page::Shelf => self.shelf_page(s, x, o),
        }
    }

    /// Right edge of the left column, and the right column's left and right edges.
    fn columns(&self, x: f32, left: f32) -> (f32, f32, f32) {
        let col = x + PAD + left;
        (col, col + 26.0, x + self.open_w() - PAD)
    }

    /// The big number of a page with a line above and below it.
    fn figure(&self, above: &str, big: &str, below: &str, x0: f32, x1: f32, o: f32) {
        self.caption(above, rect(x0, TOP, x1, TOP + 16.0), o);
        self.text(big, &self.f.big, rect(x0 - 1.0, 52.0, x1, 92.0), &self.ink, o);
        self.caption(below, rect(x0, 92.0, x1, 107.0), o);
    }

    fn music_page(&self, s: &Scene, x: f32, o: f32) {
        let Some(m) = &s.music else { return };
        let right = x + self.open_w() - PAD;
        let has_lyrics = m.lyrics_ready && !m.lines.is_empty();
        let o1 = self.block(1, o);
        if has_lyrics && m.lyrics_on {
            let (col, r0, _) = self.columns(x, 222.0);
            let art = rect(x + PAD, 40.0, x + PAD + 56.0, 96.0);
            self.art(art, 12.0, m.accent, o1);
            self.text(m.title, &self.f.body_semi, rect(art.right + 12.0, 46.0, col, 64.0), &self.ink, o1);
            self.caption(m.byline, rect(art.right + 12.0, 64.0, col, 80.0), o1);
            self.player(m, x + PAD, col, 108.0, 130.0, has_lyrics, o1);
            let o2 = self.block(2, o);
            self.lyrics(m, rect(r0 - 8.0, TOP, right, 140.0), o2);
        } else {
            let art = rect(x + PAD, 40.0, x + PAD + 80.0, 120.0);
            self.art(art, 14.0, m.accent, o1);
            let x0 = art.right + 18.0;
            let o2 = self.block(2, o);
            self.text(m.title, &self.f.title, rect(x0, 42.0, right, 62.0), &self.ink, o2);
            self.caption(m.byline, rect(x0, 62.0, right, 78.0), o2);
            self.player(m, x0, right, 98.0, 126.0, has_lyrics, o2);
        }
    }

    /// The seek bar with the times at its ends, and under it the controls (plus the lyrics toggle when there are lyrics).
    fn player(&self, m: &Music, x0: f32, x1: f32, bar_y: f32, ctl_y: f32, has_lyrics: bool, o: f32) {
        self.caption(&clock_text(m.pos_ms), rect(x0, bar_y - 8.0, x0 + 40.0, bar_y + 8.0), o);
        self.text(&clock_text(m.duration_ms), &self.f.caption_end, rect(x1 - 40.0, bar_y - 8.0, x1, bar_y + 8.0), &self.ink, 0.5 * o);
        let (b0, b1) = (x0 + 38.0, x1 - 38.0);
        let frac = if m.duration_ms > 0 { m.pos_ms as f32 / m.duration_ms as f32 } else { 0.0 };
        self.progress(b0, b1, bar_y, frac, m.accent, o);
        if m.can_seek {
            self.add_hit(rect(b0 - 4.0, bar_y - 9.0, b1 + 4.0, bar_y + 9.0), Hit::Seek { left: b0, right: b1 }, o);
        }
        let cx = (x0 + x1) / 2.0;
        self.icon_button(cx - 42.0, ctl_y, 14.0, PREVIOUS, Hit::Previous, false, o);
        self.icon_button(cx, ctl_y, 15.0, if m.playing { PAUSE } else { PLAY }, Hit::PlayPause, true, o);
        self.icon_button(cx + 42.0, ctl_y, 14.0, NEXT, Hit::Next, false, o);
        if has_lyrics {
            let tx = x1 - 12.0;
            if m.lyrics_on {
                self.dot(tx, ctl_y, 12.0, rgb(m.accent, 0.22 * o));
            }
            let c = if m.lyrics_on { rgb(m.accent, o) } else { color(1.0, 1.0, 1.0, 0.5 * o) };
            self.text_in(LIST, &self.f.icon_small, rect(tx - 12.0, ctl_y - 12.0, tx + 12.0, ctl_y + 12.0), c);
            self.add_hit(rect(tx - 14.0, ctl_y - 14.0, tx + 14.0, ctl_y + 14.0), Hit::LyricsToggle, o);
        }
    }

    /// Three lines: the one before (dim), the one being sung (word by word when timed) and the one after. A new line
    /// slides up into place. A translation, when there is one, takes the place of the line after.
    fn lyrics(&self, m: &Music, r: D2D_RECT_F, o: f32) {
        let up = m.slide * 24.0;
        let side = |line: &crate::lrc::Line, top: f32, alpha: f32| {
            let lr = rect(r.left, top + up, r.right, top + up + 20.0);
            self.text(&line.text, &self.f.lyric_side, lr, &self.ink, alpha * o);
            self.add_hit(lr, Hit::Line(line.ms), o);
        };
        let Some(i) = m.line else {
            // Before the first line: a note where the words will be, the first line coming up.
            self.text_in("\u{E8D6}", &self.f.icon, rect(r.left, 64.0, r.right, 104.0), rgb(m.accent, 0.8 * o));
            side(&m.lines[0], 112.0, 0.45);
            return;
        };
        if let Some(prev) = i.checked_sub(1).and_then(|p| m.lines.get(p)) {
            side(prev, 40.0, 0.32 * (1.0 - m.slide));
        }
        let cur = &m.lines[i];
        let main = rect(r.left, 60.0 + up, r.right, 112.0 + up);
        self.sung(cur, &cur.text, m.lit, main, o * (1.0 - 0.5 * m.slide));
        self.add_hit(main, Hit::Line(cur.ms), o);
        match &cur.tr {
            Some(tr) => self.text(tr, &self.f.lyric_side, rect(r.left, 114.0 + up, r.right, 134.0 + up), &self.ink, 0.55 * o),
            None => {
                if let Some(next) = m.lines.get(i + 1) {
                    side(next, 114.0, 0.45);
                }
            }
        }
    }

    fn focus_page(&self, s: &Scene, x: f32, o: f32) {
        let f = &s.focus;
        let c = if f.on_break { TEAL } else { CORAL };
        let (col, r0, r1) = self.columns(x, 196.0);
        let o1 = self.block(1, o);
        self.figure(f.label, f.left, f.until, x + PAD, col, o1);
        self.progress(x + PAD, col, 121.0, 1.0 - f.fraction, c, o1);

        let o2 = self.block(2, o);
        self.caption("Rounds", rect(r0, TOP, r1, TOP + 16.0), o2);
        // One segment per focus round: done, the one under way (filling as it goes), still to come.
        let n = f.rounds.max(1);
        let seg = (r1 - r0 - 5.0 * (n - 1) as f32) / n as f32;
        for k in 0..n {
            let sx = r0 + k as f32 * (seg + 5.0);
            self.round_in(rect(sx, 58.0, sx + seg, 64.0), 3.0, color(1.0, 1.0, 1.0, 0.12 * o2));
            let fill = match k.cmp(&f.done) {
                std::cmp::Ordering::Less => 1.0,
                std::cmp::Ordering::Equal if !f.on_break => 1.0 - f.fraction,
                _ => 0.0,
            };
            if fill > 0.01 {
                self.round_in(rect(sx, 58.0, sx + (seg * fill).max(6.0), 64.0), 3.0, rgb(CORAL, o2));
            }
        }
        let (glyph, label) = match (f.running, f.fraction >= 0.999) {
            (true, _) => (PAUSE, "Pause"),
            (false, true) => (PLAY, "Start"),
            (false, false) => (PLAY, "Resume"),
        };
        let mut px = r0;
        px += self.pill_button(px, 76.0, glyph, label, Hit::FocusToggle, Some(c), o2) + 8.0;
        px += self.pill_button(px, 76.0, NEXT, "Skip", Hit::FocusSkip, None, o2) + 8.0;
        self.pill_button(px, 76.0, RESET, "Reset", Hit::FocusReset, None, o2);
        self.caption(f.today, rect(r0, 112.0, r1, 128.0), o2);
    }

    fn prayer_page(&self, s: &Scene, x: f32, o: f32) {
        let Some(p) = &s.prayer else { return };
        let (col, r0, r1) = self.columns(x, 176.0);
        let o1 = self.block(1, o);
        self.figure(&format!("Next · {}", p.next_name), &p.next_time, &p.until, x + PAD, col, o1);
        self.progress(x + PAD, col, 121.0, p.progress, AMBER, o1);

        // The day as an arc from Fajr to Isha, the sun where we are on it, the five times underneath.
        let o2 = self.block(2, o);
        let (base, lift) = (90.0, 42.0);
        let at = |t: f32| (lerp(r0 + 8.0, r1 - 8.0, t), base - (t * std::f32::consts::PI).sin() * lift);
        let all: Vec<(f32, f32)> = (0..=40).map(|k| at(k as f32 / 40.0)).collect();
        self.round_in(rect(r0, base - 0.5, r1, base + 0.5), 0.0, color(1.0, 1.0, 1.0, 0.10 * o2));
        self.curve(&all, 1.6, color(1.0, 1.0, 1.0, 0.18 * o2));
        let lit: Vec<(f32, f32)> = (0..=40).map(|k| at(p.now * k as f32 / 40.0)).collect();
        self.curve(&lit, 2.2, rgb(AMBER, o2));
        let col_w = (r1 - r0) / 5.0;
        for (i, name) in p.names.iter().take(5).enumerate() {
            let (mx, my) = at(p.marks[i]);
            let next = i == p.next;
            self.dot(mx, my, if next { 3.5 } else { 2.5 }, if next { rgb(AMBER, o2) } else { color(1.0, 1.0, 1.0, 0.55 * o2) });
            let cx = r0 + col_w * (i as f32 + 0.5);
            let c = if next { rgb(AMBER, o2) } else { color(1.0, 1.0, 1.0, 0.55 * o2) };
            self.text_in(name, &self.f.caption_center, rect(cx - col_w / 2.0, 100.0, cx + col_w / 2.0, 114.0), c);
            let c = if next { rgb(AMBER, o2) } else { color(1.0, 1.0, 1.0, 0.9 * o2) };
            self.text_in(&p.times[i], &self.f.label, rect(cx - col_w / 2.0, 114.0, cx + col_w / 2.0, 130.0), c);
        }
        if p.now > 0.0 && p.now < 1.0 {
            let (sx, sy) = at(p.now);
            self.dot(sx, sy, 9.0, rgb(AMBER, 0.18 * o2));
            self.dot(sx, sy, 5.0, rgb(AMBER, o2));
        }
    }

    fn water_page(&self, s: &Scene, x: f32, o: f32) {
        let Some(wv) = &s.water else { return };
        let (col, r0, r1) = self.columns(x, 176.0);
        let o1 = self.block(1, o);
        self.caption("Today", rect(x + PAD, TOP, col, TOP + 16.0), o1);
        self.text(&wv.drunk, &self.f.big, rect(x + PAD - 1.0, 52.0, col, 92.0), &self.ink, o1);
        let bw = self.measure_in(&self.f.big, &wv.drunk);
        self.text(&format!("/ {} L", wv.goal), &self.f.body_semi, rect(x + PAD + bw + 6.0, 64.0, col, 88.0), &self.ink, 0.5 * o1);
        let pct = (wv.frac * 100.0).round() as u32;
        let say = if pct >= 100 { "Goal reached. Nice.".to_string() } else { format!("{pct}% of today's goal") };
        self.caption(&say, rect(x + PAD, 92.0, col, 107.0), o1);
        self.progress(x + PAD, col, 121.0, wv.frac, AQUA, o1);

        let o2 = self.block(2, o);
        let px = r0 + self.pill_button(r0, 40.0, ADD, &format!("Glass · {}", wv.glass), Hit::Drink, Some(AQUA), o2) + 8.0;
        self.pill_button(px, 40.0, UNDO, "Undo", Hit::Undrink, None, o2);
        // Each hour from 6 to 22: how much you drank.
        let hours: Vec<u32> = (6..=22).collect();
        let slot = (r1 - r0) / hours.len() as f32;
        let top = (wv.by_hour.iter().copied().max().unwrap_or(0)).max(500) as f32;
        let (y0, y1) = (78.0, 120.0);
        for (k, h) in hours.iter().enumerate() {
            let bx = r0 + slot * k as f32 + slot * 0.2;
            let ml = wv.by_hour[*h as usize] as f32;
            let now = *h == wv.hour_now;
            let bh = (ml / top * (y1 - y0)).max(3.0);
            let c =
                if ml > 0.0 { rgb(AQUA, (if now { 1.0 } else { 0.75 }) * o2) } else { color(1.0, 1.0, 1.0, (if now { 0.35 } else { 0.10 }) * o2) };
            self.round_in(rect(bx, y1 - bh, bx + slot * 0.6, y1), 2.0, c);
            if h % 6 == 0 || *h == 22 {
                self.text_in(
                    &h.to_string(),
                    &self.f.caption_center,
                    rect(bx - 10.0, 122.0, bx + slot * 0.6 + 10.0, 136.0),
                    color(1.0, 1.0, 1.0, 0.45 * o2),
                );
            }
        }
    }

    fn todo_page(&self, s: &Scene, x: f32, o: f32) {
        let (col, r0, r1) = self.columns(x, 132.0);
        let total = s.tasks.len();
        let done = s.tasks.iter().filter(|t| t.done).count();
        let o1 = self.block(1, o);
        self.caption("Done", rect(x + PAD, TOP, col, TOP + 16.0), o1);
        self.text(&format!("{done}/{total}"), &self.f.big, rect(x + PAD - 1.0, 52.0, col, 92.0), &self.ink, o1);
        let px = x + PAD + self.pill_button(x + PAD, 100.0, ADD, "Add", Hit::AddTask, Some(MINT), o1) + 6.0;
        if done > 0 {
            self.pill_button(px, 100.0, DELETE, "Clear", Hit::ClearDone, None, o1);
        }

        let o2 = self.block(2, o);
        if total == 0 {
            self.caption("Nothing to do. Add a task; a time in it, like 15:30, sets a reminder.", rect(r0, 70.0, r1, 100.0), o2);
            return;
        }
        const ROWS: usize = 4;
        for (k, t) in s.tasks.iter().skip(s.task_scroll).take(ROWS).enumerate() {
            let y = TOP + 1.0 + k as f32 * 26.0;
            let row = rect(r0, y, r1, y + 23.0);
            let hovered = s.hover == Some(Hit::Task(t.index));
            self.round_in(row, 8.0, color(1.0, 1.0, 1.0, (if hovered { 0.10 } else { 0.05 }) * o2));
            let (cx, cy) = (r0 + 13.0, y + 11.5);
            if t.done {
                self.dot(cx, cy, 7.0, rgb(MINT, o2));
                self.text(CHECK, &self.f.icon_small, rect(cx - 7.0, cy - 7.0, cx + 7.0, cy + 7.0), &self.fill, o2);
            } else {
                self.ring(cx, cy, 6.5, 1.5, 0.0, MINT, o2 * 2.5);
            }
            let due_w = if t.due.is_empty() { 0.0 } else { self.measure_in(&self.f.caption, &t.due) + 10.0 };
            self.text(&t.text, &self.f.body, rect(r0 + 27.0, y, r1 - 10.0 - due_w, y + 23.0), &self.ink, (if t.done { 0.45 } else { 0.95 }) * o2);
            if !t.due.is_empty() {
                self.text(&t.due, &self.f.caption_end, rect(r1 - 10.0 - due_w, y, r1 - 10.0, y + 23.0), &self.ink, 0.5 * o2);
            }
            self.add_hit(row, Hit::Task(t.index), o2);
        }
        if total > ROWS {
            // A thin scroll thumb in the margin.
            let (t0, t1) = (TOP + 1.0, TOP + 1.0 + ROWS as f32 * 26.0 - 3.0);
            let len = (t1 - t0) * ROWS as f32 / total as f32;
            let at = t0 + (t1 - t0 - len) * s.task_scroll as f32 / (total - ROWS) as f32;
            self.round_in(rect(r1 + 6.0, at, r1 + 8.0, at + len), 1.0, color(1.0, 1.0, 1.0, 0.3 * o2));
        }
    }

    fn widgets_page(&self, s: &Scene, x: f32, o: f32) {
        let w = self.open_w();
        let half = (w - 2.0 * PAD - 12.0) / 2.0;
        for (k, widget) in s.widgets.iter().enumerate() {
            let o = self.block(1 + k as u32, o);
            let x0 = x + PAD + k as f32 * (half + 12.0);
            let r = rect(x0, TOP, x0 + half, 140.0);
            self.card(r, o);
            self.widget(widget, r, o);
        }
    }

    fn widget(&self, wdg: &Widget, r: D2D_RECT_F, o: f32) {
        let (l, rr) = (r.left + 14.0, r.right - 14.0);
        match wdg {
            Widget::Week { days, line } => {
                self.caption("This week", rect(l, r.top + 8.0, rr, r.top + 24.0), o);
                let cw = (r.right - r.left - 16.0) / 7.0;
                for (i, (day, due, today)) in days.iter().enumerate() {
                    let cx = r.left + 8.0 + cw * (i as f32 + 0.5);
                    let letter = ["M", "T", "W", "T", "F", "S", "S"][i];
                    self.text(letter, &self.f.caption_center, rect(cx - 12.0, r.top + 28.0, cx + 12.0, r.top + 42.0), &self.ink, 0.45 * o);
                    let nr = rect(cx - 12.0, r.top + 44.0, cx + 12.0, r.top + 66.0);
                    if *today {
                        self.dot(cx, r.top + 55.0, 11.0, color(1.0, 1.0, 1.0, o));
                        self.text(&day.to_string(), &self.f.label, nr, &self.fill, o);
                    } else {
                        self.text(&day.to_string(), &self.f.label, nr, &self.ink, 0.85 * o);
                    }
                    if *due {
                        self.dot(cx, r.top + 72.0, 2.2, rgb(MINT, o));
                    }
                }
                self.caption(line, rect(l, r.bottom - 26.0, rr, r.bottom - 10.0), o);
            }
            Widget::Battery { percent, charging } => {
                let c = if *charging {
                    MINT
                } else if *percent <= 20 {
                    AMBER
                } else {
                    NEUTRAL
                };
                let (cx, cy) = (l + 30.0, (r.top + r.bottom) / 2.0);
                self.ring(cx, cy, 28.0, 4.0, *percent as f32 / 100.0, c, o);
                self.text(&format!("{percent}%"), &self.f.label, rect(cx - 26.0, cy - 10.0, cx + 26.0, cy + 10.0), &self.ink, o);
                let tx = cx + 44.0;
                self.caption("Battery", rect(tx, cy - 24.0, rr, cy - 8.0), o);
                self.text(if *charging { "Charging" } else { "On battery" }, &self.f.body_semi, rect(tx, cy - 8.0, rr, cy + 12.0), &self.ink, o);
            }
            Widget::Prayer { name, time, until } => {
                self.caption("Next prayer", rect(l, r.top + 8.0, rr, r.top + 24.0), o);
                self.text(name, &self.f.mid, rect(l - 1.0, r.top + 28.0, rr, r.top + 58.0), &self.ink, o);
                self.text_in(time, &self.f.body_semi, rect(l, r.top + 60.0, rr, r.top + 80.0), rgb(AMBER, o));
                self.caption(until, rect(l, r.bottom - 26.0, rr, r.bottom - 10.0), o);
            }
            Widget::Water { drunk, goal, frac } => {
                let (cx, cy) = (l + 30.0, (r.top + r.bottom) / 2.0);
                self.ring(cx, cy, 28.0, 4.0, *frac, AQUA, o);
                self.text_in(DROP, &self.f.icon, rect(cx - 12.0, cy - 12.0, cx + 12.0, cy + 12.0), rgb(AQUA, o));
                let tx = cx + 44.0;
                self.caption("Water today", rect(tx, cy - 24.0, rr, cy - 8.0), o);
                self.text(&format!("{drunk} / {goal} L"), &self.f.body_semi, rect(tx, cy - 8.0, rr, cy + 12.0), &self.ink, o);
            }
            Widget::Focus { today, running } => {
                self.caption("Focus", rect(l, r.top + 8.0, rr, r.top + 24.0), o);
                self.text(running, &self.f.mid, rect(l - 1.0, r.top + 28.0, rr, r.top + 58.0), &self.ink, o);
                self.caption(today, rect(l, r.bottom - 26.0, rr, r.bottom - 10.0), o);
            }
            Widget::Empty { title, say } => {
                self.caption(title, rect(l, r.top + 8.0, rr, r.top + 24.0), o);
                self.text(say, &self.f.body, rect(l, r.top + 30.0, rr, r.top + 50.0), &self.ink, 0.8 * o);
            }
        }
    }

    fn shelf_page(&self, s: &Scene, x: f32, o: f32) {
        let right = x + self.open_w() - PAD;
        let o1 = self.block(1, o);
        if s.shelf.is_empty() || s.drop_hover {
            // A dashed-looking drop zone: a soft card that lights up while files hover over it.
            let zone = rect(x + PAD, TOP + 2.0, right, 136.0);
            let a = if s.drop_hover { 0.16 } else { 0.06 };
            self.round_in(zone, 14.0, color(1.0, 1.0, 1.0, a * o1));
            let say = if s.drop_hover { "Let go to keep them here" } else { "Drop files here to keep them handy" };
            self.text_in(SHELF, &self.f.icon_big, rect(zone.left, 56.0, zone.right, 84.0), color(1.0, 1.0, 1.0, 0.7 * o1));
            self.text(say, &self.f.lyric_side, rect(zone.left, 88.0, zone.right, 108.0), &self.ink, 0.7 * o1);
            return;
        }
        const TILES: usize = 5;
        let (tw, gap) = (80.0, 9.0);
        for (k, item) in s.shelf.iter().enumerate().skip(s.shelf_scroll).take(TILES) {
            let tx = x + PAD + (k - s.shelf_scroll) as f32 * (tw + gap);
            let tile = rect(tx, TOP, tx + tw, 138.0);
            let hovered = matches!(s.hover, Some(Hit::ShelfItem(i) | Hit::ShelfRemove(i)) if i == k);
            self.round_in(tile, 12.0, color(1.0, 1.0, 1.0, (if hovered { 0.10 } else { 0.05 }) * o1));
            let pic = rect(tx + 18.0, TOP + 8.0, tx + tw - 18.0, TOP + 52.0);
            match self.thumb(&item.path) {
                Some(b) => unsafe {
                    let sz = b.GetSize();
                    // Fit the picture in its square, keeping its shape.
                    let k = ((pic.right - pic.left) / sz.width).min((pic.bottom - pic.top) / sz.height);
                    let (w, h) = (sz.width * k, sz.height * k);
                    let (cx, cy) = ((pic.left + pic.right) / 2.0, (pic.top + pic.bottom) / 2.0);
                    let dst = rect(cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0);
                    self.rt.DrawBitmap(&b, Some(&dst), o1, windows::Win32::Graphics::Direct2D::D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                },
                None => self.text_in(FILE, &self.f.icon_big, pic, color(1.0, 1.0, 1.0, 0.7 * o1)),
            }
            self.text(&item.name, &self.f.caption_center, rect(tx + 6.0, TOP + 60.0, tx + tw - 6.0, TOP + 76.0), &self.ink, 0.9 * o1);
            self.text(&item.size, &self.f.caption_center, rect(tx + 6.0, TOP + 76.0, tx + tw - 6.0, TOP + 90.0), &self.ink, 0.45 * o1);
            self.add_hit(tile, Hit::ShelfItem(k), o1);
            if hovered {
                let (cx, cy) = (tile.right - 10.0, tile.top + 10.0);
                self.dot(cx, cy, 8.0, color(0.25, 0.25, 0.28, o1));
                self.text(CLOSE, &self.f.icon_small, rect(cx - 8.0, cy - 8.0, cx + 8.0, cy + 8.0), &self.ink, o1);
                self.add_hit(rect(cx - 9.0, cy - 9.0, cx + 9.0, cy + 9.0), Hit::ShelfRemove(k), o1);
            }
        }
        let o2 = self.block(2, o);
        let more = s.shelf.len().saturating_sub(s.shelf_scroll + TILES);
        let cx = right - 20.0;
        self.icon_button(cx, 70.0, 14.0, DELETE, Hit::ShelfClear, false, o2);
        self.text("Clear", &self.f.caption_center, rect(cx - 24.0, 86.0, cx + 24.0, 100.0), &self.ink, 0.5 * o2);
        if more > 0 || s.shelf_scroll > 0 {
            let say = format!("{} of {}", (s.shelf_scroll + TILES).min(s.shelf.len()), s.shelf.len());
            self.text(&say, &self.f.caption_center, rect(cx - 24.0, 112.0, cx + 24.0, 126.0), &self.ink, 0.4 * o2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lrc::{Line, Word};

    /// The last frame as a top-down 32-bit BMP, to look at pages without putting anything on screen.
    fn save_bmp(r: &Renderer, path: &std::path::Path) {
        let (w, h) = (r.px_w, r.px_h);
        let px = unsafe {
            let _ = windows::Win32::Graphics::Gdi::GdiFlush();
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

    fn bmp(w: u32, h: u32, px: &[u8]) -> Vec<u8> {
        let mut f = b"BM".to_vec();
        for v in [54 + px.len() as u32, 0, 54, 40, w] {
            f.extend(v.to_le_bytes());
        }
        f.extend((-(h as i32)).to_le_bytes());
        f.extend(1u16.to_le_bytes());
        f.extend(32u16.to_le_bytes());
        f.extend([0u8; 24]);
        f.extend(px);
        f
    }

    /// Draws every page and the closed states off-screen into BMPs under %TEMP%\notch-pages, for the README and for
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
        std::fs::write(dir.join("icon.bmp"), bmp(64, 64, &icon_pixels(64))).unwrap();

        let names: Vec<String> = ["Subuh", "Dzuhur", "Ashar", "Maghrib", "Isya"].map(String::from).to_vec();
        let tasks = vec![
            TaskRow { index: 0, text: "Reply to Dina".into(), due: "15:30".into(), done: false },
            TaskRow { index: 1, text: "Finish chapter 3 notes".into(), due: "Tue 09:00".into(), done: false },
            TaskRow { index: 2, text: "Water the plants".into(), due: String::new(), done: false },
            TaskRow { index: 3, text: "Buy kopi susu".into(), due: String::new(), done: true },
            TaskRow { index: 4, text: "Call the bank".into(), due: String::new(), done: true },
        ];
        let files = [("sunset.bmp", bmp(n, n, &cover.pixels)), ("notes.txt", b"notch".to_vec()), ("Jot.md", b"- idea".to_vec())];
        let shelf: Vec<ShelfItem> = files
            .into_iter()
            .map(|(name, bytes)| {
                let p = dir.join(name);
                let size = format!("{:.1} KB", bytes.len() as f32 / 1024.0);
                std::fs::write(&p, bytes).unwrap();
                ShelfItem { path: p.to_string_lossy().into_owned(), name: name.into(), size }
            })
            .collect();
        let w = |ms, t: &str| Word { ms, text: t.into() };
        let song = vec![
            Line { ms: 0, text: "we fold the summer light".into(), ..Default::default() },
            Line {
                ms: 4000,
                text: "into paper birds tonight".into(),
                words: vec![w(4000, "into"), w(4600, "paper"), w(5200, "birds"), w(5800, "tonight")],
                ..Default::default()
            },
            Line { ms: 8000, text: "and send them through the sky".into(), ..Default::default() },
        ];
        let ayah = vec![Line {
            ms: 0,
            text: "بِسْمِ ٱللَّهِ ٱلرَّحْمَٰنِ ٱلرَّحِيمِ".into(),
            tr: Some("Dengan nama Allah Yang Maha Pengasih, Maha Penyayang.".into()),
            words: vec![w(0, "بِسْمِ"), w(700, "ٱللَّهِ"), w(1400, "ٱلرَّحْمَٰنِ"), w(2100, "ٱلرَّحِيمِ")],
        }];
        let pages = [Page::Music, Page::Focus, Page::Prayer, Page::Water, Page::Todo, Page::Widgets, Page::Shelf];
        let mut by_hour = [0; 24];
        (by_hour[8], by_hour[9], by_hour[11], by_hour[13], by_hour[14]) = (250, 250, 500, 250, 0);
        let days: [(u32, bool, bool); 7] = std::array::from_fn(|i| (23 + i as u32, i == 1 || i == 3, i == 1));

        // (file, open page, closed state, toast)
        type Shot<'a> = (&'a str, Option<f32>, Closed<'a>, Option<Toast<'a>>);
        let toast = |glyph, title, sub, value, level, tint, ring| Some(Toast { glyph, title, sub, value, level, tint, ring });
        let shots: Vec<Shot> = vec![
            ("closed-clock", None, Closed::Clock("15:04"), None),
            ("closed-eyes", None, Closed::Eyes { lid: 0.0, dozing: false, bounce: 0.0 }, None),
            ("closed-eyes-blink", None, Closed::Eyes { lid: 0.85, dozing: false, bounce: 0.0 }, None),
            ("closed-eyes-doze", None, Closed::Eyes { lid: 1.0, dozing: true, bounce: 0.0 }, None),
            ("closed-music", None, Closed::Live { left: Slot::Cover, center: "15:04", right: Slot::Bars(cover.accent, 0.0) }, None),
            ("closed-focus", None, Closed::Live { left: Slot::Ring(0.75, CORAL), center: "18 min", right: Slot::Text("2/4", CORAL) }, None),
            ("closed-prayer", None, Closed::Live { left: Slot::Glyph("\u{E708}", AMBER), center: "Ashar", right: Slot::Text("12m", AMBER) }, None),
            ("closed-lyric", None, Closed::Lyric("into paper birds tonight"), None),
            ("closed-line", None, Closed::Clock("15:04"), None),
            ("closed-volume", None, Closed::Clock(""), toast("\u{E994}", "", None, Some("64"), Some(0.64), NEUTRAL, None)),
            ("closed-battery", None, Closed::Clock(""), toast("\u{E83E}", "Charger connected", Some("Charging · 70%"), None, None, MINT, Some(0.7))),
            ("closed-water", None, Closed::Clock(""), toast(DROP, "Time for some water", Some("0.75 of 2 L so far"), None, None, AQUA, None)),
            ("music", Some(0.0), Closed::Clock(""), None),
            ("lyrics", Some(0.0), Closed::Clock(""), None),
            ("lyrics-quran", Some(0.0), Closed::Clock(""), None),
            ("focus", Some(1.0), Closed::Clock(""), None),
            ("prayer", Some(2.0), Closed::Clock(""), None),
            ("water", Some(3.0), Closed::Clock(""), None),
            ("todo", Some(4.0), Closed::Clock(""), None),
            ("widgets", Some(5.0), Closed::Clock(""), None),
            ("shelf", Some(6.0), Closed::Clock(""), None),
            ("shelf-empty", Some(6.0), Closed::Clock(""), None),
        ];
        for (name, page, closed, toast) in shots {
            let quran = name == "lyrics-quran";
            let music = Music {
                title: "Paper Sky",
                byline: "Lumen Coast · Postcards",
                pos_ms: if name == "music" { 84_000 } else { 5300 },
                duration_ms: 207_000,
                playing: true,
                can_seek: true,
                accent: cover.accent,
                lines: if quran { &ayah } else { &song },
                line: Some(if quran { 0 } else { 1 }),
                lit: 2,
                lyrics_ready: true,
                lyrics_on: name != "music",
                slide: 0.0,
            };
            let focus = FocusView {
                label: "Focus · 2 of 4",
                left: "18:42",
                until: "until 15:23",
                fraction: 0.75,
                running: true,
                on_break: false,
                done: 1,
                rounds: 4,
                today: "Today · 1 h 15 min · 3 sessions",
            };
            let prayer = PrayerView {
                names: &names,
                times: ["04:05", "11:41", "15:05", "17:54", "19:08"].map(String::from),
                next: 2,
                next_name: "Ashar",
                next_time: "15:05".into(),
                until: "in 1 min".into(),
                progress: 0.96,
                marks: [0.0, 0.507, 0.73, 0.91, 1.0],
                now: 0.728,
            };
            let water = WaterView { drunk: "1.25".into(), goal: "2".into(), frac: 0.625, by_hour, hour_now: 15, glass: "250 ml".into() };
            // Closed: the pill hugs its content, as App::closed() does.
            let (w, h, radius) = match (&page, &closed, &toast) {
                (Some(_), _, _) => (cfg.theme.open_width, cfg.theme.open_height, cfg.theme.radius),
                (None, _, Some(t)) if t.level.is_some() => (TOAST_BAR_W, 26.0, 13.0),
                (None, _, Some(t)) => (r.measure_toast(t.title, t.sub, t.value) + 59.0, TOAST_TALL, 18.0),
                (None, Closed::Live { center, .. }, _) => (r.measure(center) + 2.0 * (26.0 + 6.0) + 12.0, 26.0, 13.0),
                (None, Closed::Lyric(t), _) => (r.measure(t) + 6.0 + 16.0 + 9.0 + 14.0, 26.0, 13.0),
                (None, Closed::Eyes { .. }, _) => (96.0, 26.0, 13.0),
                _ => (cfg.theme.closed_width, 26.0, 13.0),
            };
            let line = name == "closed-line";
            let scene = Scene {
                w,
                h: if line { 4.0 } else { h },
                radius,
                open: page.map_or(0.0, |_| 1.0),
                peek: 0.0,
                line,
                pages: &pages,
                page: page.unwrap_or(0.0),
                hover: if name == "shelf" { Some(Hit::ShelfItem(0)) } else { None },
                pressed: None,
                press: 1.0,
                arrows: (0.0, if name == "focus" { 1.0 } else { 0.0 }),
                date: "Tue 24 Nov",
                time: "15:04",
                note: None,
                focus_ring: Some(0.75),
                playing: true,
                music: Some(music),
                focus,
                prayer: Some(prayer),
                water: Some(water),
                tasks: &tasks,
                task_scroll: 0,
                widgets: [Widget::Week { days, line: "1 task today · next: Reply to Dina 15:30" }, Widget::Battery { percent: 70, charging: true }],
                shelf: if name == "shelf-empty" { &[] } else { &shelf },
                shelf_scroll: 0,
                drop_hover: false,
                closed,
                toast,
                pop: 1.0,
            };
            r.draw(&scene).unwrap();
            save_bmp(&r, &dir.join(format!("{name}.bmp")));
        }
    }
}
