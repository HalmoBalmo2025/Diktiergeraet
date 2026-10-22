//! Verlauf der letzten Diktate (nur im Speicher, weg nach einem Neustart).
//!
//! Im Reiter „Diktat“ steht je Diktat der Anfang; liegt die Maus darauf,
//! zeigt das Panel den ganzen Text daneben. Ein Klick legt ihn in die
//! Zwischenablage – nützlich, wenn das Einfügen danebenging.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::note::UserText;

/// So viele Diktate werden behalten.
const CAPACITY: usize = 10;

/// So viele Zeichen eines Diktats werden angezeigt.
const LABEL_CHARS: usize = 60;

/// Fortlaufende Nummer eines Verlaufseintrags; mit ihr fordert das Panel
/// ein Diktat an. Auf der Leitung eine Zahl; die erste ist 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct HistorySerial(u64);

impl std::fmt::Display for HistorySerial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Ein angezeigter Verlaufseintrag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct HistoryItem {
    /// Fortlaufende Nummer, mit der die Oberfläche das Diktat anfordert.
    pub serial: HistorySerial,
    /// Angezeigter Anfang des Diktats.
    pub label: String,
    /// Der ganze Text, angezeigt, solange die Maus auf dem Eintrag liegt.
    pub text: UserText,
}

/// Ein Diktat mit fortlaufender Nummer.
#[derive(Debug)]
struct Entry {
    /// Fortlaufende Nummer; bleibt gleich, wenn neue Diktate dazukommen.
    serial: HistorySerial,
    /// Der Text, wie das Modell ihn geliefert hat (vor dem Anpassen an die
    /// Schreibmarke).
    text: String,
}

/// Die letzten Diktate, neuestes zuerst.
#[derive(Debug, Default)]
pub(crate) struct History {
    entries: VecDeque<Entry>,
    next_serial: HistorySerial,
}

impl History {
    /// Merkt sich ein Diktat; das älteste fällt bei Bedarf heraus.
    pub(crate) fn push(&mut self, text: &str) {
        self.entries.push_front(Entry {
            serial: self.next_serial,
            text: text.to_owned(),
        });
        self.entries.truncate(CAPACITY);
        // Nach `u64::MAX` Diktaten von vorn; so lange läuft kein Diktator.
        self.next_serial = HistorySerial(self.next_serial.0.wrapping_add(1));
    }

    /// Text des Diktats mit dieser Nummer, falls noch vorhanden.
    #[must_use]
    pub(crate) fn get(&self, serial: HistorySerial) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.serial == serial)
            .map(|entry| entry.text.as_str())
    }

    /// Die angezeigten Einträge, neuestes zuerst.
    #[must_use]
    pub(crate) fn items(&self) -> Vec<HistoryItem> {
        self.entries
            .iter()
            .map(|entry| HistoryItem {
                serial: entry.serial,
                label: label(&entry.text),
                text: UserText::new(entry.text.as_str()),
            })
            .collect()
    }
}

/// Anfang des Textes in einer Zeile, gekürzt mit „…“.
fn label(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= LABEL_CHARS {
        return line;
    }
    let mut short: String = line.chars().take(LABEL_CHARS).collect();
    short.truncate(short.trim_end().len());
    short.push_str(" …");
    short
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_and_limited() {
        let mut history = History::default();
        for number in 0..12 {
            history.push(&format!("Diktat {number}"));
        }
        let items = history.items();
        assert_eq!(items.len(), CAPACITY);
        assert_eq!(items[0].label, "Diktat 11");
        assert_eq!(items[0].text.shown(), "Diktat 11");
        assert_eq!(items[9].label, "Diktat 2");
        assert_eq!(history.get(HistorySerial(0)), None);
    }

    #[test]
    fn finds_entry_by_serial() {
        let mut history = History::default();
        history.push("Erstes");
        history.push("Zweites");
        let items = history.items();
        assert_eq!(history.get(items[1].serial), Some("Erstes"));
        assert_eq!(history.get(HistorySerial(99)), None);
    }

    #[test]
    fn labels_are_one_short_line() {
        assert_eq!(label("Hallo\n\nWelt"), "Hallo Welt");
        let long = "Ein sehr langer Satz, der nicht ganz ins Panel passt und deshalb gekürzt wird.";
        assert_eq!(
            label(long),
            "Ein sehr langer Satz, der nicht ganz ins Panel passt und des …"
        );
    }
}
