//! Das Sprachmodell: Cohere Transcribe über transcribe.cpp.
//!
//! Das Modell wird einmal geladen und bleibt im Speicher; jede Aufnahme
//! wird in Abschnitte von höchstens 30 s geteilt und nacheinander
//! transkribiert. Satzzeichen und Großschreibung liefert das Modell selbst.

use crate::note::{Explain, Note};
use std::path::{Path, PathBuf};

use transcribe_cpp::{Model, RunOptions, Session, TimestampKind};

use crate::audio::Pcm16k;
use crate::audio::chunk::{self, MAX_CHUNK_SAMPLES};
use crate::language::Language;
use crate::paths;

/// Fehler des Modells.
#[derive(Debug, thiserror::Error)]
pub(crate) enum EngineError {
    /// Die Modelldatei fehlt.
    #[error("Modelldatei fehlt: {path}")]
    ModelMissing {
        /// Erwarteter Pfad, mit `~` für den Benutzerordner
        /// ([`paths::tilde`]).
        path: String,
    },
    /// Das Modell ließ sich nicht laden.
    #[error("Modell laden: {0}")]
    Load(#[source] Box<transcribe_cpp::Error>),
    /// Das Laden ist vorhin gescheitert; bis zum nächsten Anlass (neue
    /// Aufnahme, neues Transkript) wird es nicht wiederholt, damit nicht
    /// jeder Teil eines Diktats Sekunden auf denselben Fehler wartet.
    #[error("Modell lädt nicht (vorhin gescheitert: {0})")]
    FailedBefore(Note),
    /// Das Transkribieren eines Abschnitts schlug fehl.
    #[error("Abschnitt {chunk} von {total}: {source}")]
    Run {
        /// Nummer des Abschnitts (ab 1).
        chunk: usize,
        /// Anzahl der Abschnitte.
        total: usize,
        /// Ursache.
        #[source]
        source: Box<transcribe_cpp::Error>,
    },
}

impl Explain for EngineError {
    fn english(&self) -> String {
        match self {
            Self::ModelMissing { path } => format!("model file missing: {path}"),
            Self::Load(source) => format!("loading the model: {source}"),
            Self::FailedBefore(reason) => {
                format!("model does not load (failed earlier: {})", reason.en())
            }
            Self::Run {
                chunk,
                total,
                source,
            } => format!("section {chunk} of {total}: {source}"),
        }
    }
}

/// Ein Abschnitt ist gescheitert; was die Abschnitte davor ergaben, geht
/// nicht verloren.
#[derive(Debug)]
pub(crate) struct Unfinished {
    /// Der Text der Abschnitte vor dem gescheiterten (leer, wenn es der
    /// erste war oder das Modell gar nicht lief).
    pub text: String,
    /// Warum es nicht weiterging.
    pub error: EngineError,
}

impl From<EngineError> for Unfinished {
    /// Gescheitert, bevor ein Abschnitt fertig war.
    fn from(error: EngineError) -> Self {
        Self {
            text: String::new(),
            error,
        }
    }
}

/// Fortschritt beim Transkribieren langer Aufnahmen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Progress {
    /// Fertige Abschnitte.
    pub done: usize,
    /// Alle Abschnitte.
    pub total: usize,
}

/// Was das Modell zu einem Abschnitt erkannte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Recognized {
    /// Der Text.
    pub text: String,
    /// Hörte das Modell vorzeitig auf, warum; der Text ist dann
    /// unvollständig (transcribe.cpp: „incomplete by contract“).
    pub cut: Option<Cut>,
}

/// Warum das Modell einen Abschnitt nicht zu Ende schrieb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cut {
    /// Die Längengrenze der Ausgabe war erreicht.
    Length,
    /// Die Ausgabe begann sich zu wiederholen.
    Repetition,
}

impl Cut {
    /// Name fürs Protokoll.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Length => "Längengrenze",
            Self::Repetition => "Wiederholung",
        }
    }
}

/// Ein geladenes Spracherkennungsmodell, wie der Modell-Thread es
/// braucht; Tests geben ein nachgebautes, das auf Wunsch scheitert.
pub(crate) trait Recognizer {
    /// Transkribiert Audio beliebiger Länge, Abschnitt für Abschnitt.
    ///
    /// # Errors
    /// [`Unfinished`] mit [`EngineError::Run`], wenn ein Abschnitt
    /// fehlschlägt; der Text der Abschnitte davor bleibt darin.
    fn transcribe(&mut self, pcm: &Pcm16k, language: Language) -> Result<Recognized, Unfinished>;

    /// Transkribiert ein Stück von höchstens 30 s (ohne weiteres Teilen).
    ///
    /// # Errors
    /// Der Fehler des Modells; welcher Abschnitt es war, weiß der Aufrufer.
    fn transcribe_piece(
        &mut self,
        samples: &[f32],
        language: Language,
    ) -> Result<Recognized, Box<transcribe_cpp::Error>>;
}

/// Woher der Modell-Thread sein Modell lädt (und nach dem Entladen neu).
pub(crate) trait Loader {
    /// Das geladene Modell.
    type Model: Recognizer;

    /// Lädt das Modell (dauert einige Sekunden).
    ///
    /// # Errors
    /// [`EngineError::ModelMissing`] oder [`EngineError::Load`].
    fn load(&self) -> Result<Self::Model, EngineError>;
}

/// Das Modell in einer GGUF-Datei.
#[derive(Debug, Clone)]
pub(crate) struct ModelFile(pub PathBuf);

impl Loader for ModelFile {
    type Model = Engine;

    fn load(&self) -> Result<Engine, EngineError> {
        Engine::load(&self.0)
    }
}

/// Ein geladenes Modell mit einer Sitzung.
#[derive(Debug)]
pub(crate) struct Engine {
    session: Session,
}

impl Engine {
    /// Lädt das Modell (dauert einige Sekunden).
    ///
    /// # Errors
    /// [`EngineError::ModelMissing`] oder [`EngineError::Load`].
    pub(crate) fn load(path: &Path) -> Result<Self, EngineError> {
        if !path.is_file() {
            return Err(EngineError::ModelMissing {
                path: paths::tilde(path),
            });
        }
        let model = Model::load(path).map_err(|error| EngineError::Load(Box::new(error)))?;
        let session = model
            .session()
            .map_err(|error| EngineError::Load(Box::new(error)))?;
        Ok(Self { session })
    }
}

impl Recognizer for Engine {
    fn transcribe(&mut self, pcm: &Pcm16k, language: Language) -> Result<Recognized, Unfinished> {
        let options = run_options(language);
        let chunks = chunk::split(pcm.samples(), MAX_CHUNK_SAMPLES);
        transcribe_sections(&chunks, language, |samples| {
            accept(self.session.run(samples, &options))
        })
    }

    fn transcribe_piece(
        &mut self,
        samples: &[f32],
        language: Language,
    ) -> Result<Recognized, Box<transcribe_cpp::Error>> {
        accept(self.session.run(samples, &run_options(language))).map_err(Box::new)
    }
}

/// Das Ergebnis eines Laufs. Bricht das Modell wegen Länge oder
/// Wiederholung ab, wird der bis dahin erkannte Text verwendet statt alles
/// zu verwerfen, mit dem Vermerk, dass er unvollständig ist.
fn accept(
    result: Result<transcribe_cpp::Transcript, transcribe_cpp::Error>,
) -> Result<Recognized, transcribe_cpp::Error> {
    match result {
        Ok(transcript) => Ok(Recognized {
            text: transcript.text,
            cut: None,
        }),
        Err(transcribe_cpp::Error::OutputTruncated {
            partial: Some(partial),
            message: _,
        }) => Ok(Recognized {
            text: partial.text,
            cut: Some(Cut::Length),
        }),
        Err(transcribe_cpp::Error::OutputRepetition {
            partial: Some(partial),
            message: _,
        }) => Ok(Recognized {
            text: partial.text,
            cut: Some(Cut::Repetition),
        }),
        Err(error) => Err(error),
    }
}

/// Einstellungen für einen Lauf des Modells: feste Sprache, ohne Zeitmarken.
fn run_options(language: Language) -> RunOptions {
    RunOptions {
        language: Some(language.code().to_owned()),
        timestamps: TimestampKind::None,
        ..RunOptions::default()
    }
}

/// Transkribiert `sections` nacheinander mit `run`. Scheitert einer, endet
/// es dort; der Text der Abschnitte davor kommt mit dem Fehler zurück.
/// Hörte das Modell bei einem Abschnitt vorzeitig auf, steht der erste Grund im Ergebnis.
fn transcribe_sections(
    sections: &[&[f32]],
    language: Language,
    mut run: impl FnMut(&[f32]) -> Result<Recognized, transcribe_cpp::Error>,
) -> Result<Recognized, Unfinished> {
    let total = sections.len();
    let mut texts = Vec::with_capacity(total);
    let mut cut = None;
    for (index, samples) in sections.iter().enumerate() {
        match run(samples) {
            Ok(recognized) => {
                cut = cut.or(recognized.cut);
                texts.push(recognized.text);
            }
            Err(source) => {
                return Err(Unfinished {
                    text: join_texts(&texts, language),
                    error: EngineError::Run {
                        chunk: index.saturating_add(1),
                        total,
                        source: Box::new(source),
                    },
                });
            }
        }
    }
    Ok(Recognized {
        text: join_texts(&texts, language),
        cut,
    })
}

/// Fügt die Texte der Abschnitte zusammen, mit dem Abstand der Sprache
/// (Japanisch und Chinesisch ohne Leerzeichen).
fn join_texts(texts: &[String], language: Language) -> String {
    texts
        .iter()
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(language.gap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_trimmed_non_empty_texts() {
        let texts = vec![
            " Erster Satz. ".to_owned(),
            String::new(),
            "Zweiter Satz.".to_owned(),
        ];
        assert_eq!(
            join_texts(&texts, Language::German),
            "Erster Satz. Zweiter Satz."
        );
        let japanese = vec!["今日は".to_owned(), "雨です。".to_owned()];
        assert_eq!(join_texts(&japanese, Language::Japanese), "今日は雨です。");
    }

    #[test]
    fn a_failing_section_keeps_the_text_before_it() {
        let sections: [&[f32]; 3] = [&[1.0], &[2.0], &[3.0]];
        let mut runs = 0;
        let unfinished = transcribe_sections(&sections, Language::German, |samples| {
            runs += 1;
            match samples {
                [1.0] => Ok(whole("Erster Satz.")),
                [2.0] => Ok(whole(" Zweiter Satz. ")),
                _ => Err(transcribe_cpp::Error::Busy("belegt".to_owned())),
            }
        })
        .unwrap_err();
        assert_eq!(unfinished.text, "Erster Satz. Zweiter Satz.");
        assert!(
            matches!(
                unfinished.error,
                EngineError::Run {
                    chunk: 3,
                    total: 3,
                    source: _
                }
            ),
            "{:?}",
            unfinished.error
        );
        assert_eq!(runs, 3);
        let all =
            transcribe_sections(&sections[..2], Language::German, |_| Ok(whole("Satz."))).unwrap();
        assert_eq!(all, whole("Satz. Satz."));
    }

    /// Ein vollständig erkannter Text.
    fn whole(text: &str) -> Recognized {
        Recognized {
            text: text.to_owned(),
            cut: None,
        }
    }

    #[test]
    fn a_cut_answer_keeps_its_text_and_says_so() {
        let partial = |text: &str| {
            Some(Box::new(transcribe_cpp::Transcript {
                text: text.to_owned(),
                ..transcribe_cpp::Transcript::default()
            }))
        };
        assert_eq!(
            accept(Err(transcribe_cpp::Error::OutputTruncated {
                message: "budget".to_owned(),
                partial: partial("Bis hier"),
            }))
            .unwrap(),
            Recognized {
                text: "Bis hier".to_owned(),
                cut: Some(Cut::Length)
            }
        );
        assert_eq!(
            accept(Err(transcribe_cpp::Error::OutputRepetition {
                message: "loop".to_owned(),
                partial: partial("Einmal"),
            }))
            .unwrap()
            .cut,
            Some(Cut::Repetition)
        );
        accept(Err(transcribe_cpp::Error::OutputTruncated {
            message: "budget".to_owned(),
            partial: None,
        }))
        .unwrap_err();
        let sections: [&[f32]; 2] = [&[1.0], &[2.0]];
        let joined = transcribe_sections(&sections, Language::German, |samples| match samples {
            [1.0] => Ok(whole("Eins.")),
            _ => Ok(Recognized {
                text: "Zwei".to_owned(),
                cut: Some(Cut::Repetition),
            }),
        })
        .unwrap();
        assert_eq!(
            joined.cut,
            Some(Cut::Repetition),
            "der Abbruch geht nicht verloren"
        );
    }
}
