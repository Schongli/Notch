# Changelog

## 0.2.0

A redesign: richer pages, livelier motion, and a closed notch that gets in the way less. Still about 6 MB and no CPU
while idle.

**Less in the way**
- The closed notch is smaller (26 tall, 120 wide) and shows what's going on in three compact slots: cover, time and
  bars for music; a ring, minutes and round for focus; a moon, name and minutes for a prayer coming up.
- The lyric line no longer stretches the closed notch across the top of the screen. It's on the music page now, or
  back in the closed notch with `[music] lyrics_in_pill = true`.
- Hover opens only when the pointer rests (220 ms, and not while it's still moving or a button is held). Before
  opening, the notch peeks: it grows a touch and lightens.
- `[theme] style = "line"`: a thin line while nothing is happening.

**New**
- Resting eyes: after a quiet minute the clock gives way to two eyes that blink, doze off and wake up (`[eyes]`).
- Water: a daily goal, a glass button, the day by the hour, and nudges only when you fall behind (`[water]`).
- Widgets page: two cards of your choice, from week, battery, prayer, water and focus (`[widgets]`).
- Prayer page: the day as an arc from Fajr to Isha with the sun on it, and the five times.
- Focus keeps today's focused time and sessions.
- Two-line pop-ups with a ring around the icon (charger, battery, reminders, prayer, water).

**Redesigned**
- A header on every page; two-column pages with cards, pill buttons and a soft glow in the page's colour.
- The Music and Lyrics pages are one page, with a button to show or hide the lyrics.
- The notch's top corners flare into the screen edge.
- Motion: content comes in block by block, buttons sink when pressed, pop-ups grow out and settle, the page dots
  stretch, side arrows appear near the edges, and the music bars hop on a new track.
- The wheel scrolls the to-do list and the shelf; `↑` `↓` too while pinned.

**Settings**
- New: `[theme] style`, `[music]`, `[water]`, `[widgets]`, `[eyes]`. New size defaults; sizes you never changed in 0.1
  move to them on their own.

## 0.1.0

First release: music from any player with synced lyrics (and Qistara's own), pop-ups, prayer times, a focus timer,
to-do with reminders, Jot, and a file shelf.
