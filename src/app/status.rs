//! Was die Oberfläche (Menüleisten-Symbol und Panel) anzeigt.
//!
//! Jede Änderung ist ein [`UiUpdate`]; [`super::ui`] schickt sie als
//! JSON-Zeile an das Swift-Hilfsprogramm, das Symbol und Panel zeichnet.
//! Für jede Art liegt ein Beispiel unter `fixtures/ui/`, Byte für Byte so,
//! wie Rust es schreibt (Test `every_update_matches_its_fixture`).

use crate::audio::Level;
use crate::launch::Autostart;
use serde::Serialize;

use super::history::HistoryItem;
use super::source::SourceKind;
use crate::hotkey::{Mode, TriggerKey};
use crate::language::Language;
use crate::note::{Note, UserText};
use crate::polish::llama::{self, Installed};
use crate::settings::{Restart, SettingsEntries};
use crate::text::dictionary::DictionaryEntries;

/// Zustand, wie ihn das Symbol in der Menüleiste zeigt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Status {
    /// Das Modell wird geladen.
    Loading,
    /// Bereit für fn.
    Ready,
    /// Aufnahme läuft.
    Recording {
        /// Gewählte Sprache.
        language: Language,
        /// Roh oder überarbeitet.
        mode: Mode,
    },
    /// Die Aufnahme wird transkribiert.
    Processing,
    /// Das Sprachmodell überarbeitet das Diktat.
    Polishing,
    /// Die Tonspur eines Videos oder einer Datei wird geladen.
    SourceLoading,
    /// Ein Video oder eine Datei wird transkribiert.
    Transcribing {
        /// Fortschritt.
        percent: Percent,
    },
    /// Ein Transkript wird übersetzt.
    Translating {
        /// Fortschritt.
        percent: Percent,
    },
    /// Dauerhaftes Problem (Modell fehlt, Rechte fehlen); Details stehen in
    /// der Notiz und in der Mitteilung.
    Problem,
}

/// Ein Fortschritt in Prozent, 0 bis 100. Zum Panel geht er als Zahl.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct Percent(u8);

impl Percent {
    /// Noch nichts geschafft.
    pub(crate) const ZERO: Self = Self(0);

    /// `done` von `total`, abgerundet; mehr als `total` zählt als 100. Von
    /// nichts ist alles geschafft (`total` 0 = 100).
    #[must_use]
    pub(crate) fn of(done: usize, total: usize) -> Self {
        // In u128 läuft `done × 100` für kein usize über.
        let wide = |count: usize| u128::try_from(count).unwrap_or(u128::MAX);
        let share = (wide(done.min(total)) * 100)
            .checked_div(wide(total))
            .unwrap_or(100);
        Self(u8::try_from(share).unwrap_or(100))
    }

    /// Der Wert, 0 bis 100.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn value(self) -> u8 {
        self.0
    }
}

/// Stand des langen Transkripts (Reiter `YouTube` und Datei).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum TranscriptView {
    /// Nichts in Arbeit.
    Idle,
    /// Die Tonspur wird geladen.
    Loading {
        /// Art der Quelle.
        kind: SourceKind,
        /// Titel, falls schon bekannt (Dateiname).
        title: Option<UserText>,
        /// So viele warten noch danach.
        queued: usize,
    },
    /// Das Modell transkribiert.
    Transcribing {
        /// Titel des Videos bzw. Dateiname.
        title: UserText,
        /// Fortschritt.
        percent: Percent,
        /// So viele warten noch danach.
        queued: usize,
    },
    /// Das Sprachmodell übersetzt das Transkript.
    Translating {
        /// Titel des Videos.
        title: UserText,
        /// Zielsprache.
        to: Language,
        /// Fortschritt.
        percent: Percent,
        /// So viele warten noch danach.
        queued: usize,
    },
}

/// Version des Formats der Nachrichten an die Oberfläche. Steigt mit jeder
/// Änderung, die eine ältere Oberfläche nicht mehr lesen kann; die
/// Swift-Seite kennt ihre (`wireProtocolVersion` in
/// `helpers/ui/Messages.swift`) und protokolliert eine Abweichung.
pub(crate) const PROTOCOL_VERSION: u32 = 2;

/// Feste Angaben für den Reiter „Info“, einmal beim Start; die erste
/// Nachricht.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Setup {
    /// Immer [`PROTOCOL_VERSION`].
    pub protocol_version: u32,
    /// Taste zum Diktieren (als Wert aus `settings.toml`; das Panel
    /// beschriftet sie in seiner Sprache).
    pub key: TriggerKey,
    /// Dateiname des Modells.
    pub model: String,
    /// Version von Diktator.
    pub version: &'static str,
    /// Ob Diktator beim Anmelden startet.
    pub autostart: Autostart,
    /// Pfad des Protokolls; `None`, wenn macOS keinen Benutzerordner nennt.
    pub log: Option<String>,
}

/// Der Reiter „Einstellungen“.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SettingsView {
    /// Was in `settings.toml` steht.
    pub entries: SettingsEntries,
    /// Die Modelldateien zur Auswahl.
    pub models: Vec<String>,
    /// Alle Sprachen des Spracherkennungsmodells (zur Wahl für fn + ⇧,
    /// Dateien, `YouTube` und Übersetzungen).
    pub languages: Vec<LanguageOption>,
    /// Die lokalen Sprachmodelle in `models/llm/` zur Auswahl.
    pub polish_models: Vec<PolishModelOption>,
    /// Ist llama.cpp installiert (nötig für ein lokales Modell)?
    pub llama_installed: Installed,
    /// Weicht die Datei von dem ab, womit Diktator gestartet ist?
    pub restart_needed: Restart,
}

/// Ein lokales Sprachmodell zur Auswahl.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PolishModelOption {
    /// Dateiname in `models/llm/`.
    pub file: String,
    /// Lesbarer Name („Gemma 4 12B“).
    pub name: String,
    /// Größe der Datei („6,7 GB“, englisch „6.7 GB“), etwa so viel Speicher
    /// belegt es geladen.
    pub size: Note,
}

impl PolishModelOption {
    /// Die Modelle in `models/llm/`.
    #[must_use]
    pub(crate) fn available() -> Vec<Self> {
        llama::model_files()
            .into_iter()
            .map(|model| Self {
                size: gigabytes(model.bytes),
                name: model.name,
                file: model.file,
            })
            .collect()
    }
}

/// „6,7 GB“, englisch „6.7 GB“ (auf ein Zehntel gerundet).
fn gigabytes(bytes: u64) -> Note {
    // Dateigrößen erreichen `u64::MAX` nicht; die Sättigung ändert nichts.
    let tenths = bytes.saturating_add(50_000_000) / 100_000_000;
    let (whole, tenth) = (tenths / 10, tenths % 10);
    Note::new(format!("{whole},{tenth} GB"), format!("{whole}.{tenth} GB"))
}

/// Eine Sprache zur Auswahl.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LanguageOption {
    /// Sprachcode.
    pub code: Language,
    /// Deutscher Name.
    pub name: &'static str,
    /// Englischer Name (für die englische Oberfläche).
    pub english: &'static str,
}

impl LanguageOption {
    /// Alle Sprachen des Spracherkennungsmodells.
    #[must_use]
    pub(crate) fn all() -> Vec<Self> {
        Language::ALL
            .into_iter()
            .map(|code| Self {
                code,
                name: code.name(),
                english: code.english_name(),
            })
            .collect()
    }
}

/// Eine Änderung an der Oberfläche.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub(crate) enum UiUpdate {
    /// Feste Angaben beim Start.
    Setup(Setup),
    /// Neuer Zustand (Symbol in der Menüleiste).
    Status(Status),
    /// Neuer Stand des langen Transkripts.
    Transcript(TranscriptView),
    /// Was zuletzt passiert ist – steht oben im Panel, damit Ergebnisse und
    /// Fehler auch ohne Mitteilung sichtbar sind.
    Note(Note),
    /// Die Meldung oben im Panel gilt nicht mehr: Zeile leeren.
    ClearNote,
    /// Neuer Verlauf, neuestes Diktat zuerst.
    History(Vec<HistoryItem>),
    /// Autostart wurde ein- oder ausgeschaltet.
    Autostart(Autostart),
    /// Pegel der laufenden Aufnahme (für die Anzeige an der Schreibmarke).
    Level(Level),
    /// Diktator endet (der Wächter ist weg): Oberfläche schließen.
    Quit,
    /// Inhalt des Reiters „Einstellungen“.
    Settings(SettingsView),
    /// Inhalt des Reiters „Wörterbuch“.
    Dictionary(DictionaryEntries),
    /// Was in der Zwischenablage liegt (Antwort auf `check_link`).
    ClipboardLink(ClipboardLink),
}

/// Ob die Zwischenablage einen YouTube-Link hält. Geprüft wird nur die Form
/// des Links, nicht, ob das Video abrufbar ist; das Panel färbt danach die
/// YouTube-Knöpfe, solange die Maus darauf liegt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClipboardLink {
    /// Ein YouTube-Link.
    Youtube,
    /// Kein Text oder kein YouTube-Link.
    Missing,
    /// Die Zwischenablage ließ sich nicht lesen.
    Unreadable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::history::History;

    /// Name der Beispieldatei je Art; der Compiler verlangt jede.
    const fn fixture_name(update: &UiUpdate) -> &'static str {
        match update {
            UiUpdate::Setup(_) => "setup",
            UiUpdate::Status(_) => "status",
            UiUpdate::Transcript(_) => "transcript",
            UiUpdate::Note(_) => "note",
            UiUpdate::ClearNote => "clear_note",
            UiUpdate::History(_) => "history",
            UiUpdate::Autostart(_) => "autostart",
            UiUpdate::Level(_) => "level",
            UiUpdate::Quit => "quit",
            UiUpdate::Settings(_) => "settings",
            UiUpdate::Dictionary(_) => "dictionary",
            UiUpdate::ClipboardLink(_) => "clipboard_link",
        }
    }

    /// Ein Beispiel jeder Art.
    fn samples() -> Vec<UiUpdate> {
        let mut history = History::default();
        history.push("Ein Diktat.");
        vec![
            UiUpdate::Setup(Setup {
                protocol_version: PROTOCOL_VERSION,
                key: TriggerKey::RightOption,
                model: "modell.gguf".to_owned(),
                version: "1.0.0",
                autostart: Autostart::On,
                log: Some("~/Library/Logs/Diktator.log".to_owned()),
            }),
            UiUpdate::Status(Status::Recording {
                language: Language::English,
                mode: Mode::Polish,
            }),
            UiUpdate::Transcript(TranscriptView::Translating {
                title: UserText::new("Vorlesung"),
                to: Language::German,
                percent: Percent::of(1, 4),
                queued: 1,
            }),
            UiUpdate::Note(Note::new("Bereit.", "Ready.")),
            UiUpdate::ClearNote,
            UiUpdate::History(history.items()),
            UiUpdate::Autostart(Autostart::Off),
            UiUpdate::Level(crate::audio::meter(0.01)),
            UiUpdate::Quit,
            UiUpdate::Settings(SettingsView {
                entries: SettingsEntries::default(),
                models: vec!["modell.gguf".to_owned()],
                languages: LanguageOption::all().into_iter().take(2).collect(),
                polish_models: vec![PolishModelOption {
                    file: "gemma.gguf".to_owned(),
                    name: "Gemma".to_owned(),
                    size: gigabytes(6_700_000_000),
                }],
                llama_installed: Installed::Yes,
                restart_needed: Restart::NotNeeded,
            }),
            UiUpdate::Dictionary(DictionaryEntries::default()),
            UiUpdate::ClipboardLink(ClipboardLink::Youtube),
        ]
    }

    #[test]
    fn every_update_matches_its_fixture() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/ui");
        let samples = samples();
        let mut names: Vec<_> = samples.iter().map(fixture_name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), samples.len(), "je Art genau ein Beispiel");
        assert_eq!(names.len(), 12, "jede Art von UiUpdate hat ein Beispiel");
        for update in &samples {
            let path = dir.join(format!("{}.json", fixture_name(update)));
            let written = format!("{}\n", serde_json::to_string(update).unwrap());
            let Ok(fixture) = std::fs::read_to_string(&path) else {
                crate::textfile::write_new(&path, &written).unwrap();
                panic!("{} neu geschrieben: prüfen und einchecken", path.display());
            };
            assert_eq!(written, fixture, "{} weicht ab", path.display());
        }
    }

    #[test]
    fn model_sizes_are_written_per_language() {
        let size = gigabytes(6_700_000_000);
        assert_eq!((size.de(), size.en()), ("6,7 GB", "6.7 GB"));
        let small = gigabytes(949_999_999);
        assert_eq!((small.de(), small.en()), ("0,9 GB", "0.9 GB"));
    }

    #[test]
    fn percent_stays_within_zero_and_hundred() {
        assert_eq!(Percent::of(0, 3).value(), 0);
        assert_eq!(Percent::of(1, 3).value(), 33);
        assert_eq!(Percent::of(3, 3).value(), 100);
        assert_eq!(Percent::of(5, 3).value(), 100, "mehr als alles ist alles");
        assert_eq!(
            Percent::of(0, 0).value(),
            100,
            "von nichts ist alles geschafft"
        );
        assert_eq!(Percent::of(usize::MAX, usize::MAX).value(), 100);
    }

    #[test]
    fn serializes_for_the_helper() {
        let json = |update: &UiUpdate| serde_json::to_string(update).unwrap();
        assert_eq!(
            json(&UiUpdate::Status(Status::Recording {
                language: Language::English,
                mode: Mode::Polish,
            })),
            r#"{"type":"status","data":{"state":"recording","language":"en","mode":"polish"}}"#
        );
        assert_eq!(
            json(&UiUpdate::Note(Note::new("Bereit.", "Ready."))),
            r#"{"type":"note","data":{"de":"Bereit.","en":"Ready."}}"#
        );
        assert_eq!(
            json(&UiUpdate::Transcript(TranscriptView::Transcribing {
                title: UserText::new("Vorlesung"),
                percent: Percent::of(34, 100),
                queued: 2,
            })),
            r#"{"type":"transcript","data":{"state":"transcribing","title":"Vorlesung","percent":34,"queued":2}}"#
        );
        assert_eq!(
            json(&UiUpdate::Autostart(Autostart::On)),
            r#"{"type":"autostart","data":true}"#
        );
        assert_eq!(
            json(&UiUpdate::Autostart(Autostart::Off)),
            r#"{"type":"autostart","data":false}"#
        );
        assert_eq!(json(&UiUpdate::ClearNote), r#"{"type":"clear_note"}"#);
        // Die Swift-Seite liest beide Felder des Reiters als Bool.
        assert_eq!(serde_json::to_string(&Restart::Needed).unwrap(), "true");
        assert_eq!(serde_json::to_string(&Restart::NotNeeded).unwrap(), "false");
        assert_eq!(serde_json::to_string(&Installed::Yes).unwrap(), "true");
        assert_eq!(serde_json::to_string(&Installed::No).unwrap(), "false");
    }
}
