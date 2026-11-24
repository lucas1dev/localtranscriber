use crate::transcribe::Segment;

use super::CHUNK_SECONDS;

fn parse_ts(ts: &str) -> Option<i64> {
    let (hms, ms) = ts.split_once(',')?;
    let mut parts = hms.split(':');
    let h: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let s: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || ms.len() != 3 {
        return None;
    }
    let ms: i64 = ms.parse().ok()?;
    Some(((h * 60 + m) * 60 + s) * 1000 + ms)
}

/// Soma `offset_ms` a um carimbo `HH:MM:SS,mmm`; entrada inválida volta igual.
pub fn shift_ts(ts: &str, offset_ms: i64) -> String {
    match parse_ts(ts) {
        Some(total) => {
            let ms = (total + offset_ms).max(0);
            format!(
                "{:02}:{:02}:{:02},{:03}",
                ms / 3_600_000,
                ms / 60_000 % 60,
                ms / 1000 % 60,
                ms % 1000
            )
        }
        None => ts.to_string(),
    }
}

/// Desloca os segmentos para a posição do bloco e define o falante.
pub fn shift_segments(segs: Vec<Segment>, chunk_index: u32, speaker: Option<&str>) -> Vec<Segment> {
    let offset = chunk_index as i64 * CHUNK_SECONDS as i64 * 1000;
    segs.into_iter()
        .map(|s| Segment {
            start: shift_ts(&s.start, offset),
            end: shift_ts(&s.end, offset),
            text: s.text,
            speaker: speaker.map(str::to_string),
        })
        .collect()
}

/// Ordena por `start`; em empate, `a` (Você) vem antes de `b`.
pub fn interleave(a: Vec<Segment>, b: Vec<Segment>) -> Vec<Segment> {
    let mut all: Vec<Segment> = a.into_iter().chain(b).collect();
    // `sort_by` é estável: no empate, `a` (que vem primeiro) permanece antes.
    all.sort_by(|x, y| x.start.cmp(&y.start));
    all
}

fn display_name(speaker: Option<&str>) -> Option<&'static str> {
    match speaker {
        Some("voce") => Some("Você"),
        Some("reuniao") => Some("Reunião"),
        _ => None,
    }
}

fn clock(ts: &str) -> String {
    let secs = parse_ts(ts).unwrap_or(0) / 1000;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h >= 1 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Uma linha por grupo de falas consecutivas do mesmo falante: `[mm:ss] Você: texto`.
pub fn labeled_text(segs: &[Segment]) -> String {
    let mut groups: Vec<(&Segment, Vec<&str>)> = Vec::new();
    for s in segs {
        let text = s.text.trim();
        if text.is_empty() {
            continue;
        }
        match groups.last_mut() {
            Some((first, texts)) if first.speaker == s.speaker => texts.push(text),
            _ => groups.push((s, vec![text])),
        }
    }
    groups
        .into_iter()
        .map(|(first, texts)| {
            let body = texts.join(" ");
            match display_name(first.speaker.as_deref()) {
                Some(name) => format!("[{}] {name}: {body}", clock(&first.start)),
                None => format!("[{}] {body}", clock(&first.start)),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: &str, text: &str, speaker: Option<&str>) -> Segment {
        Segment {
            start: start.into(),
            end: start.into(),
            text: text.into(),
            speaker: speaker.map(str::to_string),
        }
    }

    #[test]
    fn shift_ts_adds_offset_and_rolls_over() {
        assert_eq!(shift_ts("00:00:05,000", 12_000), "00:00:17,000");
        assert_eq!(shift_ts("00:59:59,500", 1_000), "01:00:00,500");
        assert_eq!(shift_ts("lixo", 1_000), "lixo");
    }

    #[test]
    fn shift_segments_applies_chunk_offset_and_speaker() {
        let out = shift_segments(vec![seg("00:00:01,000", "oi", None)], 2, Some("voce"));
        assert_eq!(out[0].start, "00:00:25,000");
        assert_eq!(out[0].end, "00:00:25,000");
        assert_eq!(out[0].speaker.as_deref(), Some("voce"));
    }

    #[test]
    fn interleave_orders_by_time_with_a_first_on_tie() {
        let a = vec![seg("00:00:12,000", "a1", Some("voce")), seg("00:00:20,000", "a2", Some("voce"))];
        let b = vec![seg("00:00:12,000", "b1", Some("reuniao")), seg("00:00:15,000", "b2", Some("reuniao"))];
        let texts: Vec<_> = interleave(a, b).into_iter().map(|s| s.text).collect();
        assert_eq!(texts, ["a1", "b1", "b2", "a2"]);
    }

    #[test]
    fn labeled_text_groups_consecutive_speakers() {
        let segs = vec![
            seg("00:00:12,000", "Bom dia.", Some("voce")),
            seg("00:00:13,500", "", Some("voce")),
            seg("00:00:14,000", "Vamos.", Some("voce")),
            seg("00:00:15,000", "Oi.", Some("reuniao")),
        ];
        assert_eq!(labeled_text(&segs), "[00:12] Você: Bom dia. Vamos.\n[00:15] Reunião: Oi.");
    }

    #[test]
    fn labeled_text_uses_hours_after_one_hour() {
        let segs = vec![seg("01:02:03,000", "Fala.", Some("voce"))];
        assert_eq!(labeled_text(&segs), "[1:02:03] Você: Fala.");
    }
}
