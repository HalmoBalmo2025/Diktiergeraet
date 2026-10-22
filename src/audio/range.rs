//! Ein Bereich einer langen Aufnahme („von 12:30 bis 20:00“): Nur er wird
//! geladen und transkribiert.
//!
//! Die Zeiten kommen als Text aus dem Panel: `12:30`, `1:02:05` oder nur
//! Minuten (`12`); statt `:` geht auch `.`. Ein leeres „von“ heißt „ab dem
//! Anfang“, ein leeres „bis“ „bis zum Ende“; sind beide leer, gibt es keinen
//! Bereich.

use crate::note::{Explain, Note, UserText};
use std::time::Duration;

use serde::Deserialize;

use crate::text;

/// Sekunden pro Minute bzw. Minuten pro Stunde.
const SIXTY: u64 = 60;

/// Die beiden Felder „von“ und „bis“, wie sie im Panel stehen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RangeInput {
    /// Anfang, z. B. `12:30`; leer = ab dem Anfang.
    pub from: String,
    /// Ende, z. B. `20:00`; leer = bis zum Ende.
    pub to: String,
}

impl RangeInput {
    /// Der Bereich; `None`, wenn beide Felder leer sind.
    ///
    /// # Errors
    /// [`RangeError`] wie bei [`TimeRange::parse`].
    pub(crate) fn parse(&self) -> Result<Option<TimeRange>, RangeError> {
        TimeRange::parse(&self.from, &self.to)
    }
}

/// Ein Bereich mit Anfang und, falls angegeben, Ende.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimeRange {
    start: Duration,
    end: Option<Duration>,
}

/// Eine Eingabe, die keinen Bereich ergibt.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RangeError {
    /// Keine Zeit wie `12:30` oder `1:02:05`; die Eingabe zeigt nur die
    /// Meldung im Panel ([`Explain::note`]), nicht das Protokoll.
    #[error("„…“ ist keine Zeit – so schreiben: 12:30 oder 1:02:05")]
    NotATime(UserText),
    /// „bis“ liegt nicht hinter „von“.
    #[error("„bis“ muss nach „von“ liegen")]
    EndNotAfterStart,
}

impl Explain for RangeError {
    fn english(&self) -> String {
        match self {
            Self::NotATime(_) => "“…” is not a time – write it like 12:30 or 1:02:05".to_owned(),
            Self::EndNotAfterStart => "“to” must come after “from”".to_owned(),
        }
    }

    fn note(&self) -> Note {
        match self {
            Self::NotATime(input) => Note::showing(|shown| {
                (
                    format!(
                        "„{}“ ist keine Zeit – so schreiben: 12:30 oder 1:02:05",
                        shown.text(input)
                    ),
                    format!(
                        "“{}” is not a time – write it like 12:30 or 1:02:05",
                        shown.text(input)
                    ),
                )
            }),
            Self::EndNotAfterStart => Note::new(self.to_string(), self.english()),
        }
    }
}

impl TimeRange {
    /// Liest „von“ und „bis“; `None`, wenn beide leer sind.
    ///
    /// # Errors
    /// [`RangeError`], wenn eine Zeit unlesbar ist oder „bis“ nicht hinter
    /// „von“ liegt.
    pub(crate) fn parse(from: &str, to: &str) -> Result<Option<Self>, RangeError> {
        let (start, end) = (parse_time(from)?, parse_time(to)?);
        if start.is_none() && end.is_none() {
            return Ok(None);
        }
        let start = start.unwrap_or_default();
        if end.is_some_and(|end| end <= start) {
            return Err(RangeError::EndNotAfterStart);
        }
        Ok(Some(Self { start, end }))
    }

    /// Wo der Bereich beginnt.
    #[must_use]
    pub(crate) const fn start(&self) -> Duration {
        self.start
    }

    /// Ob `at` im Bereich liegt.
    #[must_use]
    pub(crate) fn contains(&self, at: Duration) -> bool {
        at >= self.start && self.end.is_none_or(|end| at < end)
    }

    /// Für Titel und Meldungen: „12:00–20:00“ oder „ab 12:00“.
    #[must_use]
    pub(crate) fn label(&self) -> String {
        let start = text::clock(self.start);
        self.end.map_or_else(
            || format!("ab {start}"),
            |end| format!("{start}–{}", text::clock(end)),
        )
    }

    /// Eingabe-Optionen für ffmpeg (vor `-i`): genau ab `start`, bis `end`.
    #[must_use]
    pub(crate) fn ffmpeg_args(&self) -> Vec<String> {
        let mut args = vec!["-ss".to_owned(), seconds(self.start)];
        if let Some(end) = self.end {
            args.extend(["-to".to_owned(), seconds(end)]);
        }
        args
    }

    /// Abschnitt für `yt-dlp --download-sections`: `*754-1200`, offen
    /// `*754-inf`.
    #[must_use]
    pub(crate) fn download_section(&self) -> String {
        let end = self.end.map_or_else(|| "inf".to_owned(), seconds);
        format!("*{}-{end}", seconds(self.start))
    }
}

/// Sekunden als Text für ffmpeg und yt-dlp.
fn seconds(at: Duration) -> String {
    at.as_secs().to_string()
}

/// Eine Zeit: `12:30`, `1:02:05`, nur Minuten (`12`); leer → `None`.
fn parse_time(input: &str) -> Result<Option<Duration>, RangeError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let not_a_time = || RangeError::NotATime(UserText::new(trimmed));
    let numbers = trimmed
        .split([':', '.'])
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            part.parse::<u64>().ok()
        })
        .collect::<Option<Vec<u64>>>()
        .ok_or_else(not_a_time)?;
    let total = match numbers.as_slice() {
        [minutes] => minutes.checked_mul(SIXTY),
        [minutes, seconds] if *seconds < SIXTY => minutes
            .checked_mul(SIXTY)
            .and_then(|total| total.checked_add(*seconds)),
        [hours, minutes, seconds] if *minutes < SIXTY && *seconds < SIXTY => hours
            .checked_mul(SIXTY)
            .and_then(|total| total.checked_add(*minutes))
            .and_then(|total| total.checked_mul(SIXTY))
            .and_then(|total| total.checked_add(*seconds)),
        _ => None,
    };
    total
        .map(|total| Some(Duration::from_secs(total)))
        .ok_or_else(not_a_time)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    #[test]
    fn reads_common_ways_to_write_a_time() {
        assert_eq!(parse_time("12:30"), Ok(Some(secs(750))));
        assert_eq!(parse_time(" 1:02:05 "), Ok(Some(secs(3_725))));
        assert_eq!(parse_time("12"), Ok(Some(secs(720))));
        assert_eq!(parse_time("12.30"), Ok(Some(secs(750))));
        assert_eq!(parse_time("0:05"), Ok(Some(secs(5))));
        assert_eq!(parse_time("90:00"), Ok(Some(secs(5_400))));
        assert_eq!(parse_time(""), Ok(None));
        assert_eq!(parse_time("  "), Ok(None));
    }

    #[test]
    fn rejects_what_is_not_a_time() {
        for input in [
            "12:60", "1:60:00", "abc", "12:", ":30", "1:2:3:4", "-5", "12 30", "1,5",
        ] {
            assert_eq!(
                parse_time(input),
                Err(RangeError::NotATime(UserText::new(input))),
                "{input}"
            );
        }
    }

    #[test]
    fn empty_fields_mean_no_range() {
        assert_eq!(TimeRange::parse("", " "), Ok(None));
    }

    #[test]
    fn open_ends_reach_to_start_or_end() {
        let from_start = TimeRange::parse("", "20:00").unwrap().unwrap();
        assert_eq!(from_start.start(), Duration::ZERO);
        assert_eq!(from_start.label(), "0:00–20:00");
        assert_eq!(from_start.download_section(), "*0-1200");

        let to_end = TimeRange::parse("12:00", "").unwrap().unwrap();
        assert_eq!(to_end.label(), "ab 12:00");
        assert_eq!(to_end.download_section(), "*720-inf");
        assert_eq!(to_end.ffmpeg_args(), ["-ss", "720"]);
    }

    #[test]
    fn closed_range_for_ffmpeg_and_titles() {
        let range = TimeRange::parse("12:30", "1:02:05").unwrap().unwrap();
        assert_eq!(range.label(), "12:30–1:02:05");
        assert_eq!(range.ffmpeg_args(), ["-ss", "750", "-to", "3725"]);
        assert!(range.contains(secs(750)), "der Anfang gehört zum Bereich");
        assert!(
            range.contains(secs(3_724)),
            "die letzte Sekunde vor dem Ende gehört zum Bereich"
        );
        assert!(
            !range.contains(secs(3_725)),
            "das Ende gehört nicht mehr zum Bereich"
        );
        assert!(
            !range.contains(secs(749)),
            "vor dem Anfang liegt außerhalb des Bereichs"
        );
    }

    #[test]
    fn end_must_lie_after_start() {
        assert_eq!(
            TimeRange::parse("20:00", "12:00"),
            Err(RangeError::EndNotAfterStart)
        );
        assert_eq!(
            TimeRange::parse("12:00", "12:00"),
            Err(RangeError::EndNotAfterStart)
        );
        assert_eq!(
            TimeRange::parse("12:x", ""),
            Err(RangeError::NotATime(UserText::new("12:x")))
        );
    }

    #[test]
    fn the_typed_text_is_shown_but_not_logged() {
        let error = TimeRange::parse("Mandant", "").unwrap_err();
        assert!(!error.to_string().contains("Mandant"), "{error}");
        let note = error.note();
        assert!(
            note.de().starts_with("„Mandant“ ist keine Zeit"),
            "{}",
            note.de()
        );
        assert!(
            note.en().starts_with("“Mandant” is not a time"),
            "{}",
            note.en()
        );
        assert!(!note.log_line().contains("Mandant"), "{}", note.log_line());
    }
}
