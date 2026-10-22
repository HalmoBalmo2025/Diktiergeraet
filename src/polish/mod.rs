//! Diktat überarbeiten lassen (fn + ⌥) – mit Apples Sprachmodell auf dem
//! Gerät oder einem lokalen Modell über llama.cpp ([`llama`]) – und lange
//! Transkripte übersetzen.
//!
//! Das Überarbeiten läuft auf einem eigenen Thread ([`spawn`]). Er fragt
//! das Modell, prüft währenddessen mit der Rechtschreibprüfung von macOS
//! (`spelling.rs`), welche Wörter schon richtig sind, und übernimmt von der
//! Antwort nur Korrekturen (Sicherheitsnetz, `guard.rs`). Übersetzt wird
//! auf demselben Thread mit demselben Modell, Absatz für Absatz
//! (`translate.rs`); wartende Diktate gehen zwischendurch vor.

mod apple;
mod guard;
pub(crate) mod llama;
mod local;
mod spelling;
mod translate;
mod worker;

use crate::note::{Explain, Note};
use std::path::PathBuf;
use std::time::Duration;

pub(crate) use worker::{
    PolishDone, PolishRequest, PolishThread, Translated, TranslationEvent, TranslationOutcome,
    TranslationRequest, Untranslated, spawn,
};

use crate::helper::HelperError;
use crate::settings::Unload;
use llama::LlamaError;

/// Länger wird auf eine Antwort nicht gewartet, das Ausweichen von einem
/// lokalen Modell auf Apple Intelligence eingeschlossen; dann bleibt der
/// Text, wie er ist. (Das Laden eines lokalen Modells zählt nicht mit.)
const TIMEOUT: Duration = Duration::from_mins(2);

/// Womit überarbeitet wird.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Backend {
    /// Apple Intelligence über `diktator-llm`.
    Apple,
    /// Ein lokales Modell über llama.cpp.
    Local {
        /// Die GGUF-Datei.
        model: PathBuf,
        /// Wann es wieder entladen wird.
        unload: Unload,
    },
}

/// Anweisung für „Überarbeiten“ in der Sprache `language` (englischer
/// Name): nur Fehler korrigieren, nichts umformulieren, nicht übersetzen.
///
/// Auf Englisch und mit zwei Beispielen, weil Apples kleines Modell damit
/// am zuverlässigsten arbeitet; mit deutscher Anweisung übersetzte es
/// englischen Text ins Deutsche (geprüft am 13.10.2026).
fn correct_instructions(language: &str) -> String {
    format!(
        "You correct dictated text in {language} with the smallest possible changes. Fix only \
spelling, capitalization, punctuation and clear grammar mistakes. Keep every word that is \
already correct, keep the word order, never add, repeat, remove or translate words, and keep \
citations like \"§ 433 II BGB\", abbreviations and names unchanged. Reply with the corrected \
text only.

Example input: ich glaube das wir morgen kommen weil es regnet
Example output: Ich glaube, dass wir morgen kommen, weil es regnet.

Example input: i think its to late for every one
Example output: I think it's too late for everyone."
    )
}

/// Warum nicht überarbeitet werden konnte.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PolishError {
    /// Apple Intelligence ist nicht verfügbar, lehnt ab oder antwortet
    /// nicht rechtzeitig.
    #[error(transparent)]
    Apple(#[from] HelperError),
    /// Die Antwort war leer.
    #[error("das Sprachmodell lieferte keinen Text")]
    Empty,
    /// Das lokale Modell steht nicht bereit (llama.cpp oder Datei fehlt …).
    #[error(transparent)]
    Llama(#[from] LlamaError),
    /// Die Anfrage an das lokale Modell scheiterte.
    #[error("lokales Modell nicht erreichbar: {0}")]
    Request(#[source] Box<ureq::Error>),
    /// llama-server lehnte die Anfrage ab (etwa zu langer Text für seinen
    /// Zusammenhang); mit seiner eigenen Begründung.
    #[error("lokales Modell lehnt ab (HTTP {status}): {}", .message.as_deref().unwrap_or("ohne Begründung"))]
    Server {
        /// Der HTTP-Status.
        status: u16,
        /// Was der Server dazu schrieb.
        message: Option<String>,
    },
    /// Das lokale Modell scheiterte und Apple Intelligence danach auch.
    #[error("{local}; Apple Intelligence: {apple}")]
    Both {
        /// Warum das lokale Modell nicht ging.
        local: Note,
        /// Warum Apple Intelligence nicht ging.
        apple: Box<PolishError>,
    },
    /// Die Antwort des lokalen Modells war nicht lesbar.
    #[error("lokales Modell: Antwort nicht lesbar: {0}")]
    Response(#[source] Box<ureq::Error>),
    /// Das lokale Modell hat die Längengrenze erreicht – es wiederholt sich
    /// oder schreibt weiter, statt nur zu korrigieren bzw. zu übersetzen.
    #[error("lokales Modell: Antwort viel länger als der Text, abgebrochen")]
    TooLong,
}

impl Explain for PolishError {
    fn english(&self) -> String {
        match self {
            Self::Apple(source) => source.english(),
            Self::Empty => "the language model returned no text".to_owned(),
            Self::Llama(source) => source.english(),
            Self::Request(source) => format!("local model unreachable: {source}"),
            Self::Server { status, message } => format!(
                "local model refuses (HTTP {status}): {}",
                message.as_deref().unwrap_or("without a reason")
            ),
            Self::Both { local, apple } => {
                format!("{}; Apple Intelligence: {}", local.en(), apple.english())
            }
            Self::Response(source) => format!("local model: unreadable answer: {source}"),
            Self::TooLong => "local model: answer much longer than the text, stopped".to_owned(),
        }
    }
}

/// Ein überarbeiteter Text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Polished {
    /// Der Text (nach dem Sicherheitsnetz).
    pub text: String,
    /// Warum das lokale Modell nicht ging, falls auf Apple Intelligence
    /// ausgewichen wurde.
    pub fallback: Option<Note>,
}
