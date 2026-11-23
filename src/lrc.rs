// LRC lyrics: "[mm:ss.xx]text" lines, several stamps per line allowed, plus the [offset:±ms] tag.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub ms: u64,
    pub text: String,
    /// A translation shown under the line (from Qistara)
    pub tr: Option<String>,
    /// Word timing, for the word-by-word highlight (from Qistara)
    pub words: Vec<Word>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Word {
    pub ms: u64,
    pub text: String,
}

/// Where each word sits in its line, in UTF-16 units (what DirectWrite counts), found left to right.
/// A word that can't be found gets None, and the highlight just skips it.
pub fn word_spans(text: &str, words: &[Word]) -> Vec<Option<(u32, u32)>> {
    let mut from = 0; // byte offset
    words
        .iter()
        .map(|w| {
            let needle = w.text.trim();
            let at = (!needle.is_empty()).then(|| text[from..].find(needle)).flatten()? + from;
            from = at + needle.len();
            let utf16 = |s: &str| s.encode_utf16().count() as u32;
            Some((utf16(&text[..at]), utf16(needle)))
        })
        .collect()
}

pub fn parse(lrc: &str) -> Vec<Line> {
    let mut offset: i64 = 0;
    let mut lines = Vec::new();
    for raw in lrc.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(body) = rest.strip_prefix('[') {
            let Some(end) = body.find(']') else { break };
            let tag = &body[..end];
            rest = &body[end + 1..];
            if let Some(ms) = stamp(tag) {
                stamps.push(ms);
            } else if let Some(v) = tag.strip_prefix("offset:") {
                offset = v.trim().parse().unwrap_or(0);
            }
        }
        // Enhanced LRC puts <mm:ss.xx> before words; keep the words.
        let text = strip_word_stamps(rest);
        for ms in stamps {
            // A positive offset shows lyrics earlier.
            lines.push(Line { ms: (ms - offset).max(0) as u64, text: text.clone(), ..Default::default() });
        }
    }
    lines.sort_by_key(|l| l.ms);
    lines
}

/// "mm:ss", "mm:ss.x", "mm:ss.xx", "mm:ss.xxx" or "mm:ss:xx" → milliseconds.
fn stamp(tag: &str) -> Option<i64> {
    let (m, s) = tag.split_once(':')?;
    let m: i64 = m.trim().parse().ok()?;
    let (s, frac) = match s.split_once(['.', ':']) {
        Some((s, f)) => (s, f),
        None => (s, ""),
    };
    let s: i64 = s.parse().ok()?;
    if s >= 60 || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let frac_ms = match frac.len() {
        0 => 0,
        1 => frac.parse::<i64>().ok()? * 100,
        2 => frac.parse::<i64>().ok()? * 10,
        _ => frac[..3].parse::<i64>().ok()?,
    };
    Some(m * 60_000 + s * 1000 + frac_ms)
}

fn strip_word_stamps(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        match rest[i..].find('>') {
            Some(j) if stamp(&rest[i + 1..i + j]).is_some() => {
                out.push_str(&rest[..i]);
                rest = &rest[i + j + 1..];
            }
            _ => {
                out.push_str(&rest[..=i]);
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Index of the line being sung at `ms`, if any has started.
pub fn current(lines: &[Line], ms: u64) -> Option<usize> {
    lines.partition_point(|l| l.ms <= ms).checked_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_and_sorted() {
        let l = parse("[ti:Song]\n[00:12.50]second\n[00:01.00]first\n");
        let line = |ms, t: &str| Line { ms, text: t.into(), ..Default::default() };
        assert_eq!(l, vec![line(1000, "first"), line(12500, "second")]);
    }

    #[test]
    fn spans_of_words() {
        let w = |t: &str| Word { ms: 0, text: t.into() };
        assert_eq!(word_spans("we fold light", &[w("we "), w("fold"), w(" light")]), [Some((0, 2)), Some((3, 4)), Some((8, 5))]);
        // the same word twice is found in order; a missing word doesn't stop the rest
        assert_eq!(word_spans("la la land", &[w("la"), w("xyz"), w("la"), w("land")]), [Some((0, 2)), None, Some((3, 2)), Some((6, 4))]);
        // UTF-16 counts: Arabic and emoji
        assert_eq!(word_spans("بِسْمِ ٱللَّهِ", &[w("بِسْمِ"), w("ٱللَّهِ")]), [Some((0, 6)), Some((7, 7))]);
        assert_eq!(word_spans("🎵 hi", &[w("hi")]), [Some((3, 2))]);
    }

    #[test]
    fn several_stamps_one_line() {
        let l = parse("[00:10.00][01:10.00]chorus");
        assert_eq!(l.iter().map(|l| l.ms).collect::<Vec<_>>(), [10_000, 70_000]);
        assert!(l.iter().all(|l| l.text == "chorus"));
    }

    #[test]
    fn fraction_forms() {
        assert_eq!(stamp("01:02"), Some(62_000));
        assert_eq!(stamp("01:02.5"), Some(62_500));
        assert_eq!(stamp("01:02.05"), Some(62_050));
        assert_eq!(stamp("01:02.123"), Some(62_123));
        assert_eq!(stamp("01:02:30"), Some(62_300));
        assert_eq!(stamp("ar:Someone"), None);
        assert_eq!(stamp("01:75.00"), None);
    }

    #[test]
    fn offset_shifts_lines() {
        let l = parse("[offset:+500]\n[00:02.00]a\n[00:00.20]b");
        assert_eq!(l.iter().map(|l| l.ms).collect::<Vec<_>>(), [0, 1500]);
    }

    #[test]
    fn enhanced_word_stamps_are_dropped() {
        let l = parse("[00:01.00]<00:01.00>we <00:01.40>fold <00:01.90>light <3");
        assert_eq!(l[0].text, "we fold light <3");
    }

    #[test]
    fn current_line() {
        let l = parse("[00:01.00]a\n[00:05.00]b\n[00:09.00]c");
        assert_eq!(current(&l, 500), None);
        assert_eq!(current(&l, 1000), Some(0));
        assert_eq!(current(&l, 8999), Some(1));
        assert_eq!(current(&l, 60_000), Some(2));
        assert_eq!(current(&[], 1000), None);
    }
}
