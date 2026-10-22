//! Verbindung zur Oberfläche: dem Swift-Hilfsprogramm `diktator-ui`
//! (Quelltext in `helpers/ui/`), das Menüleisten-Symbol und Panel
//! zeichnet.
//!
//! Ein `NSPopover` mit `SwiftUI` gibt es nur in Swift; so braucht Diktator
//! selbst kein `unsafe`. Diktator schickt jede [`UiUpdate`] als JSON-Zeile
//! an die Standardeingabe des Hilfsprogramms und liest dessen Befehle
//! ([`UiCommand`]) zeilenweise aus der Standardausgabe. Endet eine Seite,
//! endet die andere: Schließt Diktator die Eingabe, beendet sich das
//! Hilfsprogramm; endet dessen Ausgabe, beendet sich Diktator.

use crate::note::Explain;
use std::io::{BufRead, BufReader, Lines, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, ChildStdout, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::app::history::HistorySerial;
use crate::launch::Autostart;
use serde::Deserialize;

use super::source::SourceKind;
use super::status::UiUpdate;
use crate::audio::range::RangeInput;
use crate::helper::Helper;
use crate::language::Language;
use crate::process::{self, Running};
use crate::settings::SettingsEntries;
use crate::text::dictionary::DictionaryEntries;

/// Fehler beim Starten der Oberfläche.
#[derive(Debug, thiserror::Error)]
pub(crate) enum UiError {
    /// Das Hilfsprogramm liegt nicht neben dem Programm (etwa bei
    /// `cargo run`; es wird von `scripts/bundle.sh` gebaut).
    #[error("Oberfläche {} fehlt – Diktator läuft ohne Menüleisten-Symbol", Helper::Ui.file_name())]
    MissingHelper,
    /// Das Hilfsprogramm ließ sich nicht starten.
    #[error("Oberfläche nicht startbar: {0}")]
    Spawn(#[source] std::io::Error),
    /// Das Hilfsprogramm lief ohne Pipes zu Ein- und Ausgabe an.
    #[error("Oberfläche nicht startbar: keine Verbindung zum Hilfsprogramm")]
    NoPipes,
    /// Der Schreib-Thread ließ sich nicht starten.
    #[error("Oberfläche: Thread nicht startbar: {0}")]
    Thread(#[source] std::io::Error),
}

impl Explain for UiError {
    fn english(&self) -> String {
        match self {
            Self::MissingHelper => format!(
                "interface {} missing – Diktator runs without a menu bar icon",
                Helper::Ui.file_name()
            ),
            Self::Spawn(source) => format!("cannot start the interface: {source}"),
            Self::NoPipes => "cannot start the interface: no connection to the helper".to_owned(),
            Self::Thread(source) => format!("interface: cannot start thread: {source}"),
        }
    }
}

/// Ein Befehl aus dem Panel.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum UiCommand {
    /// Video aus der Zwischenablage; ohne Sprache automatisch.
    Youtube {
        /// Feste Sprache oder automatisch.
        language: Option<Language>,
        /// Nur dieser Bereich (leere Felder = alles).
        #[serde(default)]
        range: RangeInput,
        /// Danach in diese Sprache übersetzen.
        #[serde(default)]
        translate: Option<Language>,
    },
    /// Dateien: aufs Panel gezogen (`paths`) oder im Finder kopiert.
    File {
        /// Sprache der Aufnahmen.
        language: Language,
        /// Die aufs Panel gezogenen Dateien; ohne sie gilt die Zwischenablage.
        #[serde(default)]
        paths: Vec<PathBuf>,
        /// Nur dieser Bereich (leere Felder = alles; nur bei einer Datei).
        #[serde(default)]
        range: RangeInput,
    },
    /// Laufendes Transkript abbrechen.
    CancelTranscript,
    /// Letztes Transkript dieser Art erneut kopieren.
    CopyLastTranscript {
        /// `YouTube` oder Datei.
        source: SourceKind,
    },
    /// Wörterbuch im Editor öffnen.
    OpenDictionary,
    /// Prüfen, ob die Zwischenablage einen YouTube-Link hält (die Maus liegt
    /// auf einem YouTube-Knopf).
    CheckLink,
    /// Diktat aus dem Verlauf kopieren.
    CopyHistory {
        /// Fortlaufende Nummer des Diktats.
        serial: HistorySerial,
    },
    /// Autostart ein- oder ausschalten.
    Autostart {
        /// Gewünschter Zustand (`true` = ein).
        on: Autostart,
    },
    /// Geänderte Einstellungen speichern.
    SetSettings {
        /// Alle Werte des Reiters.
        settings: SettingsEntries,
    },
    /// Geändertes Wörterbuch speichern.
    SetDictionary {
        /// Alle Werte des Reiters.
        dictionary: DictionaryEntries,
    },
    /// Protokoll leeren.
    ClearLog,
    /// Das Panel wurde geöffnet oder geschlossen.
    Panel {
        /// Ob es jetzt offen ist.
        visible: bool,
    },
    /// Diktator neu starten.
    Restart,
    /// Diktator beenden.
    Quit,
    /// Die Oberfläche meldet ein Problem fürs Protokoll (etwa eine
    /// Nachricht, die sie nicht lesen konnte; ohne deren Inhalt).
    Log {
        /// Was passiert ist.
        message: String,
    },
}

/// Die laufende Oberfläche. Fällt sie weg, werden das Hilfsprogramm und
/// der Schreib-Thread beendet; auf beide wird gewartet.
pub(crate) struct Ui {
    /// Das Hilfsprogramm.
    child: Running,
    /// Zeilen aus seiner Standardausgabe.
    lines: Lines<BufReader<ChildStdout>>,
    /// Ein Eingang zum Schreib-Thread, um ihn zu beenden.
    updates: Sender<UiUpdate>,
    /// Der Thread, der Änderungen an das Hilfsprogramm schreibt; `None`,
    /// sobald beendet.
    writer: Option<JoinHandle<()>>,
}

impl Ui {
    /// Startet das Hilfsprogramm. Änderungen für die Oberfläche kommen über
    /// den Kanal aus [`channel`]: `sender` ist ein Eingang, `updates` sein
    /// Ausgang.
    ///
    /// # Errors
    /// [`UiError`], wenn das Hilfsprogramm fehlt oder nicht startet.
    pub(crate) fn spawn(
        sender: Sender<UiUpdate>,
        updates: Receiver<UiUpdate>,
    ) -> Result<Self, UiError> {
        let program = Helper::Ui.path().ok_or(UiError::MissingHelper)?;
        Self::spawn_program(&program, sender, updates)
    }

    /// Wie [`Ui::spawn`], mit dem Hilfsprogramm unter `program`.
    fn spawn_program(
        program: &Path,
        sender: Sender<UiUpdate>,
        updates: Receiver<UiUpdate>,
    ) -> Result<Self, UiError> {
        let mut child = process::spawn(
            process::command(program)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit()),
        )
        .map_err(UiError::Spawn)?;
        // Auf den Fehlerwegen beendet `child` beim Wegfallen das
        // Hilfsprogramm.
        let (Some(stdin), Some(stdout)) = (child.take_stdin(), child.take_stdout()) else {
            return Err(UiError::NoPipes);
        };
        let writer = thread::Builder::new()
            .name("ui-writer".to_owned())
            .spawn(move || write_updates(stdin, &updates));
        let writer = writer.map_err(UiError::Thread)?;
        Ok(Self {
            child,
            lines: BufReader::new(stdout).lines(),
            updates: sender,
            writer: Some(writer),
        })
    }

    /// Der nächste Befehl aus dem Panel. `None`, wenn das Hilfsprogramm
    /// beendet ist oder seine Ausgabe sich nicht lesen lässt (gemeldet mit
    /// `on_garbage`). Unbekannte Befehle werden gemeldet und übersprungen;
    /// ein bekannter mit ungültigen Werten kommt als [`Incoming::Rejected`],
    /// damit das Panel eine Antwort bekommt.
    pub(crate) fn next_command(&mut self, on_garbage: impl Fn(String)) -> Option<Incoming> {
        loop {
            let line = match self.lines.next()? {
                Ok(line) => line,
                Err(error) => {
                    on_garbage(format!("Oberfläche nicht lesbar: {error}"));
                    return None;
                }
            };
            match classify(&line) {
                Ok(incoming) => return Some(incoming),
                // Ohne die Zeile selbst: Sie kann Wörterbuch-Einträge und
                // Pfade enthalten.
                Err(error) => on_garbage(format!(
                    "Unbekannter Befehl der Oberfläche ({} Zeichen): {:?}-Fehler bei Spalte {}",
                    line.chars().count(),
                    error.classify(),
                    error.column()
                )),
            }
        }
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        // Erst das Hilfsprogramm beenden: Ein Schreiben, das an seiner
        // vollen Pipe hängt, scheitert dann.
        self.child.stop();
        // Weckt den Schreib-Thread, falls er wartet; er endet mit `Quit`.
        // Scheitert nur, wenn er schon beendet ist.
        match self.updates.send(UiUpdate::Quit) {
            Ok(()) | Err(mpsc::SendError(_)) => {}
        }
        if let Some(writer) = self.writer.take()
            && writer.join().is_err()
        {
            eprintln!("Schreib-Thread der Oberfläche ist abgestürzt");
        }
    }
}

/// Was eine Zeile aus dem Panel ist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Incoming {
    /// Ein gültiger Befehl.
    Command(UiCommand),
    /// Ein bekannter Befehl mit Werten, die nicht gelten. Das Panel zeigt sie
    /// womöglich schon an; es bekommt eine Meldung und den gespeicherten
    /// Stand zurück.
    Rejected(Rejected),
}

/// Welcher Befehl mit ungültigen Werten kam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rejected {
    /// `set_settings`.
    Settings,
    /// `set_dictionary`.
    Dictionary,
}

/// Nur der Name eines Befehls.
#[derive(Deserialize)]
struct Named {
    /// Wert von `command`.
    command: String,
}

/// Ordnet eine Zeile aus dem Panel ein.
///
/// # Errors
/// Der Fehler beim Lesen als [`UiCommand`], wenn es weder ein gültiger noch
/// ein abzulehnender bekannter Befehl ist.
fn classify(line: &str) -> Result<Incoming, serde_json::Error> {
    let error = match serde_json::from_str::<UiCommand>(line) {
        Ok(command) => return Ok(Incoming::Command(command)),
        Err(error) => error,
    };
    // Die Namen sind die von `UiCommand` (`rename_all = "snake_case"`); der
    // Test `known_commands_with_bad_values_are_rejected` hält sie gleich.
    match serde_json::from_str::<Named>(line) {
        Ok(Named { command }) if command == "set_settings" => {
            Ok(Incoming::Rejected(Rejected::Settings))
        }
        Ok(Named { command }) if command == "set_dictionary" => {
            Ok(Incoming::Rejected(Rejected::Dictionary))
        }
        Ok(Named { command: _ }) | Err(_) => Err(error),
    }
}

/// Erzeugt den Kanal für Änderungen an der Oberfläche.
#[must_use]
pub(crate) fn channel() -> (Sender<UiUpdate>, Receiver<UiUpdate>) {
    mpsc::channel()
}

/// Schreibt Änderungen als JSON-Zeilen, bis der Kanal oder das
/// Hilfsprogramm endet oder `Quit` geschrieben ist (danach liest das
/// Hilfsprogramm nichts mehr).
fn write_updates(mut stdin: ChildStdin, updates: &Receiver<UiUpdate>) {
    for update in updates {
        let last = matches!(update, UiUpdate::Quit);
        let mut line = match serde_json::to_string(&update) {
            Ok(line) => line,
            // Die Typen von `UiUpdate` lassen sich immer schreiben (Test
            // `serializes_for_the_helper`); fällt doch eine weg, steht es
            // wenigstens in der Konsole.
            Err(error) => {
                debug_assert!(false, "invariant: UiUpdate ist schreibbar: {error}");
                eprintln!("Änderung für die Oberfläche nicht schreibbar: {error}");
                continue;
            }
        };
        line.push('\n');
        if let Err(error) = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
        {
            // Das Hilfsprogramm ist weg; sein Ende meldet `next_command`.
            eprintln!("Oberfläche nimmt keine Änderungen mehr an: {error}");
            return;
        }
        if last {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_the_ui_stops_its_helper_and_writer() {
        let (sender, updates) = channel();
        let ui = Ui::spawn_program(Path::new("/bin/cat"), sender.clone(), updates).unwrap();
        send_note(&sender);
        // `main.rs` hält Sender auf diesen Kanal bis zum Programmende: Das
        // Ende des Kanals beendet den Schreib-Thread also nicht.
        let (dropped, done) = mpsc::channel();
        thread::Builder::new()
            .spawn(move || {
                drop(ui);
                dropped.send(()).unwrap();
            })
            .unwrap();
        done.recv_timeout(std::time::Duration::from_secs(5))
            .expect("Wegfallen der Oberfläche wartet auf einen Schreib-Thread, der nie endet");
        assert!(
            sender
                .send(UiUpdate::Note(crate::note::Note::same("spät")))
                .is_err(),
            "der Schreib-Thread läuft nach dem Ende der Oberfläche weiter"
        );
    }

    /// Schickt eine Meldung an die Oberfläche.
    fn send_note(sender: &Sender<UiUpdate>) {
        sender
            .send(UiUpdate::Note(crate::note::Note::same("Test")))
            .unwrap();
    }

    #[test]
    fn parses_log_messages() {
        assert_eq!(
            serde_json::from_str::<UiCommand>(r#"{"command":"log","message":"unlesbar"}"#).unwrap(),
            UiCommand::Log {
                message: "unlesbar".to_owned()
            }
        );
    }

    #[test]
    fn parses_commands() {
        let parse = |line: &str| serde_json::from_str::<UiCommand>(line).unwrap();
        assert_eq!(
            parse(r#"{"command":"autostart","on":true}"#),
            UiCommand::Autostart { on: Autostart::On }
        );
        assert_eq!(
            parse(r#"{"command":"autostart","on":false}"#),
            UiCommand::Autostart { on: Autostart::Off }
        );
        assert_eq!(
            parse(r#"{"command":"copy_history","serial":3}"#),
            UiCommand::CopyHistory {
                serial: serde_json::from_str("3").unwrap()
            }
        );
        assert_eq!(parse(r#"{"command":"quit"}"#), UiCommand::Quit);
        assert_eq!(
            parse(r#"{"command":"copy_last_transcript","source":"file"}"#),
            UiCommand::CopyLastTranscript {
                source: SourceKind::File
            }
        );
        serde_json::from_str::<UiCommand>(r#"{"command":"format_disk"}"#).unwrap_err();
    }

    #[test]
    fn parses_transcript_commands() {
        let parse = |line: &str| serde_json::from_str::<UiCommand>(line).unwrap();
        assert_eq!(
            parse(r#"{"command":"youtube","language":null}"#),
            UiCommand::Youtube {
                language: None,
                range: RangeInput::default(),
                translate: None
            }
        );
        assert_eq!(
            parse(r#"{"command":"youtube","language":"de","range":{"from":"12:30","to":""}}"#),
            UiCommand::Youtube {
                language: Some(Language::German),
                range: RangeInput {
                    from: "12:30".to_owned(),
                    to: String::new()
                },
                translate: None
            }
        );
        assert_eq!(
            parse(r#"{"command":"youtube","language":null,"translate":"de"}"#),
            UiCommand::Youtube {
                language: None,
                range: RangeInput::default(),
                translate: Some(Language::German)
            }
        );
        assert_eq!(
            parse(r#"{"command":"file","language":"en"}"#),
            UiCommand::File {
                language: Language::English,
                paths: Vec::new(),
                range: RangeInput::default(),
            }
        );
        assert_eq!(
            parse(
                r#"{"command":"file","language":"de","paths":["/Users/a/1.m4a","/Users/a/2.m4a"]}"#
            ),
            UiCommand::File {
                language: Language::German,
                paths: vec![
                    PathBuf::from("/Users/a/1.m4a"),
                    PathBuf::from("/Users/a/2.m4a")
                ],
                range: RangeInput::default(),
            }
        );
    }

    #[test]
    fn parses_settings_and_dictionary() {
        let parse = |line: &str| serde_json::from_str::<UiCommand>(line).unwrap();
        assert_eq!(
            parse(
                r#"{"command":"set_settings","settings":{"key":"rechte_wahltaste","sounds":true,"volume":40,"model_file":"m.gguf","unload_minutes":15,"secondary_language":"fr","parts":15,"log":true,"polish_backend":"lokal","polish_model":"gemma.gguf","polish_unload_minutes":5,"timestamps":true,"chapters":false,"ui_language":"en"}}"#
            ),
            UiCommand::SetSettings {
                settings: SettingsEntries {
                    key: crate::hotkey::TriggerKey::RightOption,
                    sounds: crate::settings::SoundSwitch::On,
                    volume: crate::feedback::Volume::try_from(40).unwrap(),
                    model_file: "m.gguf".to_owned(),
                    unload_minutes: 15,
                    secondary_language: crate::language::SecondaryLanguage::try_from(
                        Language::French
                    )
                    .unwrap(),
                    parts: crate::audio::chunk::PartTiming::try_from(15).unwrap(),
                    log: crate::feedback::Logging::On,
                    polish_backend: crate::settings::PolishBackend::Local,
                    polish_model: "gemma.gguf".to_owned(),
                    polish_unload_minutes: 5,
                    timestamps: crate::text::Timestamps::On,
                    chapters: crate::text::ChapterHeadings::Off,
                    ui_language: crate::note::UiLanguage::English,
                }
            }
        );
        assert_eq!(
            parse(
                r#"{"command":"set_dictionary","dictionary":{"replacements":[{"from":"a","to":"b"}],"legal":true,"style":"abgekürzt","statutes":["KAGB"]}}"#
            ),
            UiCommand::SetDictionary {
                dictionary: DictionaryEntries {
                    replacements: vec![crate::text::dictionary::ReplacementEntry {
                        from: "a".to_owned(),
                        to: "b".to_owned(),
                    }],
                    legal: true,
                    style: crate::text::CitationStyle::Abbreviated,
                    statutes: vec!["KAGB".to_owned()],
                    terms: Vec::new(),
                }
            }
        );
    }

    #[test]
    fn rejects_settings_out_of_range() {
        // Was die Datei beim Lesen ablehnen würde, kommt auch vom Panel
        // nicht hinein (sonst wäre die Datei nach dem Speichern unlesbar).
        let settings_with = |volume: u16, parts: u16, secondary: &str| {
            format!(
                r#"{{"command":"set_settings","settings":{{"key":"fn","sounds":true,"volume":{volume},"model_file":"m.gguf","unload_minutes":30,"secondary_language":"{secondary}","parts":{parts},"log":true,"polish_backend":"apple","polish_model":"","polish_unload_minutes":10,"timestamps":false,"chapters":true,"ui_language":"de"}}}}"#
            )
        };
        let settings = |volume: u16, parts: u16| settings_with(volume, parts, "en");
        serde_json::from_str::<UiCommand>(&settings(100, 20)).unwrap();
        serde_json::from_str::<UiCommand>(&settings(150, 5)).unwrap_err();
        serde_json::from_str::<UiCommand>(&settings(25, 21)).unwrap_err();
        serde_json::from_str::<UiCommand>(&settings_with(25, 5, "de")).unwrap_err();
    }

    #[test]
    fn known_commands_with_bad_values_are_rejected() {
        let settings = |volume: u16| {
            format!(
                r#"{{"command":"set_settings","settings":{{"key":"fn","sounds":true,"volume":{volume},"model_file":"m.gguf","unload_minutes":30,"secondary_language":"en","parts":5,"log":true,"polish_backend":"apple","polish_model":"","polish_unload_minutes":10,"timestamps":false,"chapters":true,"ui_language":"de"}}}}"#
            )
        };
        assert!(matches!(
            classify(&settings(100)),
            Ok(Incoming::Command(UiCommand::SetSettings { settings: _ }))
        ));
        assert_eq!(
            classify(&settings(150)).unwrap(),
            Incoming::Rejected(Rejected::Settings)
        );
        assert!(matches!(
            classify(
                r#"{"command":"set_dictionary","dictionary":{"replacements":[],"legal":true,"style":"abgekürzt","statutes":[]}}"#
            ),
            Ok(Incoming::Command(UiCommand::SetDictionary {
                dictionary: _
            }))
        ));
        assert_eq!(
            classify(r#"{"command":"set_dictionary","dictionary":{"replacements":7}}"#).unwrap(),
            Incoming::Rejected(Rejected::Dictionary)
        );
        classify(r#"{"command":"format_disk"}"#).unwrap_err();
        classify("kein JSON").unwrap_err();
    }
}
