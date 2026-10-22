//! Was der Nutzer liest, auf Deutsch und Englisch: Meldungen im Panel,
//! Mitteilungen und Fehlermeldungen.
//!
//! Diktator schickt jede Meldung in beiden Sprachen; die Oberfläche zeigt
//! die, die im Reiter „Info“ gewählt ist. So wechselt auch eine Meldung,
//! die schon dasteht, sofort mit. Fehlertypen haben ihre deutsche Meldung
//! in `Display` (sie geht auch ins Protokoll) und die englische über
//! [`Explain`]. Was fremde Bibliotheken und Programme melden (`io::Error`,
//! ffmpeg, yt-dlp …), steht in beiden Fassungen so, wie es kommt – meist
//! englisch.
//!
//! Text des Nutzers (Titel, Dateinamen, Pfade, Eingaben) ist ein
//! [`UserText`]: ohne `Display`, damit er nicht versehentlich in eine
//! Protokollzeile gerät. Eine Meldung, die ihn zeigt, entsteht mit
//! [`Note::showing`]; ihre Zeile fürs Protokoll ([`Note::log_line`]) hat an
//! seiner Stelle „…“.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Sprache der Oberfläche (Panel und Mitteilungen).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum UiLanguage {
    /// Deutsch.
    #[default]
    #[serde(rename = "de")]
    German,
    /// Englisch.
    #[serde(rename = "en")]
    English,
}

impl UiLanguage {
    /// Wert in `settings.toml`.
    #[must_use]
    pub(crate) const fn file_value(self) -> &'static str {
        match self {
            Self::German => "de",
            Self::English => "en",
        }
    }
}

/// Ein Fehler, den der Nutzer zu sehen bekommt: `Display` ist die deutsche
/// Meldung, [`Explain::english`] die englische.
pub(crate) trait Explain: fmt::Display {
    /// Die Meldung auf Englisch.
    fn english(&self) -> String;

    /// Beide Fassungen als Meldung.
    fn note(&self) -> Note {
        Note::new(self.to_string(), self.english())
    }
}

/// Text des Nutzers: ein Titel, ein Dateiname, ein Pfad, eine Eingabe. Er
/// wird angezeigt, aber nie protokolliert: kein `Display`, und `Debug`
/// nennt nur seine Länge.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct UserText(String);

impl UserText {
    /// Hält `text` als Text des Nutzers.
    #[must_use]
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// Der Text, nur zum Anzeigen und Kopieren (nie fürs Protokoll).
    #[must_use]
    pub(crate) fn shown(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for UserText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UserText({} Zeichen)", self.0.chars().count())
    }
}

/// Wie [`Note::showing`] einen [`UserText`] einsetzt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shown {
    /// Den Text selbst (für Panel und Mitteilung).
    Text,
    /// „…“ (für das Protokoll).
    Redacted,
}

impl Shown {
    /// Was für `user` eingesetzt wird.
    #[must_use]
    pub(crate) fn text(self, user: &UserText) -> &str {
        match self {
            Self::Text => &user.0,
            Self::Redacted => "…",
        }
    }
}

/// Eine Meldung in beiden Sprachen der Oberfläche. Die Swift-Hilfsprogramme
/// melden Fehler in derselben Form (`{"de": …, "en": …}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Note {
    /// Deutsche Fassung (auch für Mitteilungen).
    de: String,
    /// Englische Fassung.
    en: String,
    /// Die Zeile fürs Protokoll, wenn die Meldung Text des Nutzers zeigt
    /// (an seiner Stelle „…“); `None`: die deutsche Fassung.
    #[serde(skip)]
    log: Option<String>,
}

impl Note {
    /// Eine Meldung mit deutscher und englischer Fassung, ohne Text des
    /// Nutzers.
    #[must_use]
    pub(crate) fn new(de: impl Into<String>, en: impl Into<String>) -> Self {
        Self {
            de: de.into(),
            en: en.into(),
            log: None,
        }
    }

    /// Eine Meldung, die Text des Nutzers zeigt: `make` baut deutsche und
    /// englische Fassung und setzt jeden [`UserText`] über [`Shown::text`]
    /// ein. Es läuft zweimal, einmal mit dem Text, einmal mit „…“ für das
    /// Protokoll.
    #[must_use]
    pub(crate) fn showing(make: impl Fn(Shown) -> (String, String)) -> Self {
        let (de, en) = make(Shown::Text);
        let (log, _) = make(Shown::Redacted);
        Self {
            log: (log != de).then_some(log),
            de,
            en,
        }
    }

    /// Ein Text, der in beiden Fassungen gleich ist: was ein anderes
    /// Programm meldet.
    #[must_use]
    pub(crate) fn same(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            en: text.clone(),
            de: text,
            log: None,
        }
    }

    /// Die deutsche Fassung (auch für Mitteilungen).
    #[must_use]
    pub(crate) fn de(&self) -> &str {
        &self.de
    }

    /// Die englische Fassung.
    #[must_use]
    pub(crate) fn en(&self) -> &str {
        &self.en
    }

    /// Die Zeile fürs Protokoll: die deutsche Fassung, Text des Nutzers
    /// darin als „…“.
    #[must_use]
    pub(crate) fn log_line(&self) -> &str {
        self.log.as_deref().unwrap_or(&self.de)
    }

    /// Die deutsche Fassung wie in [`Note::showing`] gewünscht: zum Zeigen
    /// oder fürs Protokoll. So lässt sich eine Meldung in eine größere
    /// einbauen.
    #[must_use]
    pub(crate) fn de_for(&self, shown: Shown) -> &str {
        match shown {
            Shown::Text => &self.de,
            Shown::Redacted => self.log_line(),
        }
    }

    /// Diese Meldung eingebettet: `de` baut die deutsche Fassung (und die
    /// Zeile fürs Protokoll) um die deutsche, `en` die englische um die
    /// englische.
    #[must_use]
    pub(crate) fn around(&self, de: impl Fn(&str) -> String, en: impl Fn(&str) -> String) -> Self {
        Self {
            de: de(&self.de),
            en: en(&self.en),
            log: self.log.as_deref().map(de),
        }
    }

    /// Mehrere Meldungen hintereinander, getrennt durch ein Leerzeichen.
    #[must_use]
    pub(crate) fn joined<'a>(notes: impl IntoIterator<Item = &'a Self>) -> Self {
        let notes: Vec<&Self> = notes.into_iter().collect();
        let join = |part: fn(&Self) -> &str| -> String {
            notes
                .iter()
                .map(|note| part(note))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let de = join(|note| &note.de);
        let log = join(Self::log_line);
        Self {
            en: join(|note| &note.en),
            log: (log != de).then_some(log),
            de,
        }
    }

    /// Die Fassung in `language`.
    #[must_use]
    pub(crate) fn text(&self, language: UiLanguage) -> &str {
        match language {
            UiLanguage::German => &self.de,
            UiLanguage::English => &self.en,
        }
    }
}

impl fmt::Display for Note {
    /// Die Zeile fürs Protokoll ([`Note::log_line`]).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.log_line())
    }
}

/// Anzahl der Wörter eines Textes zum Anzeigen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Words(usize);

impl Words {
    /// Die Wörter von `text`.
    #[must_use]
    pub(crate) fn of(text: &str) -> Self {
        Self(crate::text::word_count(text))
    }

    /// „1 Wort“, „12 Wörter“.
    #[must_use]
    pub(crate) fn de(self) -> String {
        match self.0 {
            1 => "1 Wort".to_owned(),
            count => format!("{count} Wörter"),
        }
    }

    /// „1 word“, „12 words“.
    #[must_use]
    pub(crate) fn en(self) -> String {
        match self.0 {
            1 => "1 word".to_owned(),
            count => format!("{count} words"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words_in_both_languages() {
        assert_eq!(Words::of("Hallo").de(), "1 Wort");
        assert_eq!(Words::of("Hallo Welt").de(), "2 Wörter");
        assert_eq!(Words::of("").de(), "0 Wörter");
        assert_eq!(Words::of("hello").en(), "1 word");
        assert_eq!(Words::of("").en(), "0 words");
    }

    #[test]
    fn serializes_both_languages() {
        assert_eq!(
            serde_json::to_string(&Note::new("Bereit.", "Ready.")).unwrap(),
            r#"{"de":"Bereit.","en":"Ready."}"#
        );
        assert_eq!(Note::same("ffmpeg: x").en, "ffmpeg: x");
        assert_eq!(
            serde_json::from_str::<Note>(r#"{"de":"kein Text","en":"no text"}"#).unwrap(),
            Note::new("kein Text", "no text")
        );
        assert_eq!(Note::new("a", "b").text(UiLanguage::English), "b");
    }

    #[test]
    fn user_text_is_shown_but_not_logged() {
        let title = UserText::new("Mandant Müller");
        let note = Note::showing(|shown| {
            (
                format!("„{}“ wird gelesen …", shown.text(&title)),
                format!("Reading “{}” …", shown.text(&title)),
            )
        });
        assert_eq!(note.de, "„Mandant Müller“ wird gelesen …");
        assert_eq!(note.en, "Reading “Mandant Müller” …");
        assert_eq!(note.log_line(), "„…“ wird gelesen …");
        assert_eq!(note.to_string(), "„…“ wird gelesen …");
        assert_eq!(format!("{title:?}"), "UserText(14 Zeichen)");
        assert_eq!(
            serde_json::to_string(&note).unwrap(),
            r#"{"de":"„Mandant Müller“ wird gelesen …","en":"Reading “Mandant Müller” …"}"#
        );
        let wrapped = note.around(|de| format!("Datei: {de}"), |en| format!("File: {en}"));
        assert_eq!(wrapped.de, "Datei: „Mandant Müller“ wird gelesen …");
        assert_eq!(wrapped.log_line(), "Datei: „…“ wird gelesen …");
        let both = Note::joined([&Note::new("Fehler.", "Error."), &wrapped]);
        assert_eq!(both.en, "Error. File: Reading “Mandant Müller” …");
        assert_eq!(both.log_line(), "Fehler. Datei: „…“ wird gelesen …");
        assert_eq!(Note::new("Bereit.", "Ready.").log_line(), "Bereit.");
    }
}
