//! Lange Transkripte: `YouTube`-Video oder Audio-/Videodatei, auf Wunsch
//! danach übersetzt.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use super::{ControlEvent, Controller, Model, Platform, State};
use crate::app::source::{self, LanguageChoice, LoadError, Loaded, Origin, Source, SourceKind};
use crate::app::status::{ClipboardLink, Percent, TranscriptView, UiUpdate};
use crate::app::worker::{Job, TranscriptId, TranscriptOutcome};
use crate::audio::range::{RangeError, RangeInput};
use crate::engine::{EngineError, Progress};
use crate::feedback::Sound;
use crate::language::Language;
use crate::note::{Explain, Note, UiLanguage, UserText, Words};
use crate::paste::PasteError;
use crate::polish::{
    PolishError, PolishRequest, Translated, TranslationEvent, TranslationOutcome,
    TranslationRequest, Untranslated,
};
use crate::text::ChapterHeadings;
use crate::youtube::VideoUrl;

/// Zustand des langen Transkripts.
pub(super) enum TranscriptJob {
    /// Nichts in Arbeit.
    Idle,
    /// Die Tonspur wird geladen.
    Downloading {
        /// Woher (Video oder Datei).
        origin: Origin,
        /// Titel, falls schon bekannt ([`Source::known_title`]).
        title: Option<UserText>,
        /// Abbruch-Flag, geteilt mit dem Lade-Thread.
        cancel: Arc<AtomicBool>,
        /// Der Lade-Thread.
        thread: JoinHandle<()>,
        /// Danach in diese Sprache übersetzen.
        translate: Option<Language>,
    },
    /// Das Modell transkribiert.
    Transcribing {
        /// Nummer des Auftrags beim Modell-Thread.
        id: TranscriptId,
        /// Woher (Video oder Datei).
        origin: Origin,
        /// Abbruch-Flag, geteilt mit dem Modell-Thread.
        cancel: Arc<AtomicBool>,
        /// Titel des Videos bzw. Dateiname.
        title: UserText,
        /// Fortschritt in Prozent.
        percent: Percent,
        /// Sprache der Tonspur.
        language: Language,
        /// Danach in diese Sprache übersetzen.
        translate: Option<Language>,
    },
    /// Das Sprachmodell übersetzt das fertige Transkript.
    Translating {
        /// Woher (Video oder Datei).
        origin: Origin,
        /// Abbruch-Flag, geteilt mit dem Überarbeiten-Thread.
        cancel: Arc<AtomicBool>,
        /// Titel in der Originalsprache.
        title: UserText,
        /// Das Transkript in der Originalsprache (wird bei Abbruch kopiert).
        original: String,
        /// Von welcher in welche Sprache.
        languages: LanguagePair,
        /// Fortschritt in Prozent.
        percent: Percent,
    },
}

/// Von welcher in welche Sprache übersetzt wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LanguagePair {
    /// Sprache der Tonspur.
    pub(super) from: Language,
    /// Zielsprache.
    pub(super) to: Language,
}

/// Ein Transkript, dessen Erkennung eben endete.
#[derive(Debug)]
struct Recognized {
    /// Woher (Video oder Datei).
    origin: Origin,
    /// Titel in der Originalsprache.
    title: UserText,
    /// Abbruch-Flag des Auftrags; gilt auch für eine folgende Übersetzung.
    cancel: Arc<AtomicBool>,
}

/// Das letzte Transkript je Art der Quelle (nur bis zum Beenden).
#[derive(Debug, Default)]
pub(super) struct LastTranscripts {
    youtube: Option<String>,
    file: Option<String>,
}

impl LastTranscripts {
    /// Das letzte Transkript dieser Art, falls es eins gibt.
    fn get(&self, kind: SourceKind) -> Option<&str> {
        match kind {
            SourceKind::Youtube => self.youtube.as_deref(),
            SourceKind::File => self.file.as_deref(),
        }
    }

    /// Merkt sich ein Transkript als letztes dieser Art.
    fn set(&mut self, kind: SourceKind, transcript: String) {
        match kind {
            SourceKind::Youtube => self.youtube = Some(transcript),
            SourceKind::File => self.file = Some(transcript),
        }
    }
}

/// Was vom Laden einer Quelle bleibt, wenn die Tonspur da ist.
struct Fetch {
    /// Woher die Tonspur kommt.
    origin: Origin,
    /// Abbrechen-Schalter des Auftrags.
    cancel: Arc<AtomicBool>,
    /// Danach in diese Sprache übersetzen.
    translate: Option<Language>,
}

impl<P: Platform> Controller<P> {
    /// Video aus der Zwischenablage laden lassen.
    pub(super) fn start_youtube(
        &mut self,
        state: &State,
        choice: LanguageChoice,
        range: &RangeInput,
        translate: Option<Language>,
    ) {
        let clipboard = match self.platform.clipboard_text() {
            Ok(clipboard) => clipboard,
            Err(error) => return self.clipboard_unreadable(&error),
        };
        let Some(url) = clipboard.and_then(|text| text.parse::<VideoUrl>().ok()) else {
            self.feedback.play(Sound::Error);
            self.note(Note::new(
                "Kein YouTube-Link in der Zwischenablage.",
                "No YouTube link on the clipboard.",
            ));
            return;
        };
        let range = match range.parse() {
            Ok(range) => range,
            Err(error) => return self.reject_range(&error),
        };
        self.start_transcript(
            state,
            Source::Youtube {
                url,
                language: choice,
                range,
                translate,
            },
        );
    }

    /// Sagt dem Panel, ob die Zwischenablage einen YouTube-Link hält. Ohne
    /// Ton und Meldung: Die Maus liegt nur auf einem Knopf.
    pub(super) fn check_link(&self) {
        let link = match self.platform.clipboard_text() {
            Ok(Some(text)) if text.parse::<VideoUrl>().is_ok() => ClipboardLink::Youtube,
            Ok(Some(_) | None) => ClipboardLink::Missing,
            Err(error) => {
                self.feedback.log(format!(
                    "Zwischenablage für die Link-Prüfung nicht lesbar: {error}"
                ));
                ClipboardLink::Unreadable
            }
        };
        self.platform.show(UiUpdate::ClipboardLink(link));
    }

    /// Die aufs Panel gezogene Datei laden lassen, ohne sie die im Finder
    /// kopierte Datei (oder den kopierten Pfad). Ein Bereich gilt nur für
    /// eine einzelne Datei.
    pub(super) fn start_file(
        &mut self,
        state: &State,
        language: Language,
        dropped: Vec<PathBuf>,
        range: &RangeInput,
    ) {
        let range = match range.parse() {
            Ok(range) => range,
            Err(error) => return self.reject_range(&error),
        };
        let paths = if dropped.is_empty() {
            match self.clipboard_paths() {
                Ok(paths) => paths,
                Err(error) => return self.clipboard_unreadable(&error),
            }
        } else {
            dropped
        };
        if paths.is_empty() {
            self.feedback.play(Sound::Error);
            self.note(Note::new(
                "Keine Datei – aufs Panel ziehen oder im Finder auswählen und ⌘C drücken.",
                "No file – drag it onto the panel or select it in the Finder and press ⌘C.",
            ));
            return;
        }
        let (files, others): (Vec<PathBuf>, Vec<PathBuf>) =
            paths.into_iter().partition(|path| path.is_file());
        if let Some(other) = others.first() {
            let other = UserText::new(other.display().to_string());
            self.feedback.play(Sound::Error);
            self.note(Note::showing(|shown| {
                (
                    format!("„{}“ ist keine Datei.", shown.text(&other)),
                    format!("“{}” is not a file.", shown.text(&other)),
                )
            }));
        }
        if range.is_some() && files.len() > 1 {
            self.feedback.play(Sound::Error);
            self.note(Note::new(
                format!(
                    "Ein Bereich geht nur bei einer einzelnen Datei – gewählt sind {}.",
                    files.len()
                ),
                format!(
                    "A range only works with a single file – {} are selected.",
                    files.len()
                ),
            ));
            return;
        }
        for path in files {
            self.start_transcript(
                state,
                Source::File {
                    path,
                    language,
                    range,
                },
            );
        }
    }

    /// Der Bereich aus dem Panel ist unlesbar.
    fn reject_range(&self, error: &RangeError) {
        self.feedback.play(Sound::Error);
        self.note(
            error
                .note()
                .around(|de| format!("Bereich: {de}"), |en| format!("Range: {en}")),
        );
    }

    /// Die im Finder kopierten Dateien, sonst ein kopierter Pfad; leer,
    /// wenn die Zwischenablage keins von beiden hat.
    fn clipboard_paths(&self) -> Result<Vec<PathBuf>, PasteError> {
        let files = self.platform.clipboard_files()?;
        if !files.is_empty() {
            return Ok(files);
        }
        let text = self.platform.clipboard_text()?;
        Ok(text
            .and_then(|text| source::path_from_text(&text, dirs::home_dir().as_deref()))
            .into_iter()
            .collect())
    }

    /// Die Zwischenablage ließ sich nicht lesen (etwa weil eine andere App
    /// sie gerade hält) – nicht dasselbe wie „kein Link darin“.
    fn clipboard_unreadable(&self, error: &PasteError) {
        self.feedback.play(Sound::Error);
        self.note(Note::new(
            format!("Zwischenablage nicht lesbar: {error}"),
            format!("Clipboard not readable: {}", error.english()),
        ));
    }

    /// Startet das Laden für ein langes Transkript; läuft schon eins, wartet
    /// es in der Schlange.
    fn start_transcript(&mut self, state: &State, source: Source) {
        match &self.transcript {
            TranscriptJob::Downloading {
                origin: _,
                title: _,
                cancel: _,
                thread: _,
                translate: _,
            }
            | TranscriptJob::Transcribing {
                id: _,
                origin: _,
                cancel: _,
                title: _,
                percent: _,
                language: _,
                translate: _,
            }
            | TranscriptJob::Translating {
                origin: _,
                cancel: _,
                title: _,
                original: _,
                languages: _,
                percent: _,
            } => return self.queue(source),
            TranscriptJob::Idle => match state {
                State::Loading => return self.model_not_ready(),
                // Das Modell kommt nicht mehr: dessen Grund, nicht „noch
                // nicht“.
                State::Unavailable { reason } => {
                    self.feedback.play(Sound::Error);
                    self.note(reason.around(
                        |de| format!("Kein Transkript möglich: {de}"),
                        |en| format!("No transcript possible: {en}"),
                    ));
                    return;
                }
                // Ein Diktat kann schon laufen, während das Modell noch lädt.
                State::Ready
                | State::Recording {
                    recording: _,
                    dictation: _,
                }
                | State::Processing(_)
                | State::Polishing(_) => match self.model {
                    Model::Ready => {}
                    Model::Loading => return self.model_not_ready(),
                },
            },
        }
        self.load(source);
    }

    /// Lädt die Tonspur auf einem eigenen Thread; das Ergebnis kommt als
    /// [`ControlEvent::SourceLoaded`].
    fn load(&mut self, source: Source) {
        let origin = source.origin();
        let kind = origin.kind();
        let title = source.known_title();
        let translate = source.translate();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);
        let events = self.events.clone();
        let spawned = thread::Builder::new()
            .name("source".to_owned())
            .spawn(move || {
                let result = source::load(&source, &thread_cancel);
                // Scheitert nur, wenn die Steuerung schon beendet ist.
                match events.send(ControlEvent::SourceLoaded(result)) {
                    Ok(()) | Err(mpsc::SendError(_)) => {}
                }
            });
        match spawned {
            Ok(thread) => {
                self.note(loading_note(kind, title.as_ref()));
                self.transcript = TranscriptJob::Downloading {
                    origin,
                    title,
                    cancel,
                    thread,
                    translate,
                };
            }
            Err(error) => {
                self.feedback.play(Sound::Error);
                self.note(Note::new(
                    format!("{kind}: Thread nicht startbar: {error}"),
                    format!("{}: cannot start thread: {error}", kind.english_label()),
                ));
            }
        }
    }

    /// Ein Transkript wartet in der Schlange, bis das laufende fertig ist.
    fn queue(&mut self, source: Source) {
        let kind = source.kind();
        let title = source.known_title();
        self.waiting.push_back(source);
        let waiting = self.waiting.len();
        self.note(Note::showing(|shown| {
            let (de, en) = title.as_ref().map_or_else(
                || (kind.to_string(), kind.english_label().to_owned()),
                |title| {
                    (
                        format!("„{}“", shown.text(title)),
                        format!("“{}”", shown.text(title)),
                    )
                },
            );
            (
                format!("{de} wartet – {waiting} in der Warteschlange."),
                format!("{en} is waiting – {waiting} in the queue."),
            )
        }));
    }

    /// Ein Transkript braucht das geladene Modell.
    fn model_not_ready(&self) {
        self.feedback.play(Sound::Error);
        self.note(Note::new(
            "Das Modell ist noch nicht bereit.",
            "The model is not ready yet.",
        ));
    }

    /// Das Modell ist weg: Ein Transkript beim Modell bekommt keine Antwort
    /// mehr und endet, wartende fallen weg. Laden und Übersetzen laufen
    /// ohne das Modell weiter; ein geladenes findet danach kein Modell.
    pub(super) fn end_transcription(&mut self) {
        let dropped = self.waiting.len();
        self.waiting.clear();
        let previous = std::mem::replace(&mut self.transcript, TranscriptJob::Idle);
        match previous {
            TranscriptJob::Transcribing {
                title,
                id: _,
                origin: _,
                cancel: _,
                percent: _,
                language: _,
                translate: _,
            } => {
                self.feedback.play(Sound::Error);
                self.note(Note::showing(|shown| {
                    (
                        format!(
                            "„{}“: abgebrochen, das Modell steht nicht bereit ({dropped} wartende verworfen).",
                            shown.text(&title)
                        ),
                        format!(
                            "“{}”: cancelled, the model is not available ({dropped} waiting discarded).",
                            shown.text(&title)
                        ),
                    )
                }));
            }
            other @ (TranscriptJob::Idle
            | TranscriptJob::Downloading {
                origin: _,
                title: _,
                cancel: _,
                thread: _,
                translate: _,
            }
            | TranscriptJob::Translating {
                origin: _,
                cancel: _,
                title: _,
                original: _,
                languages: _,
                percent: _,
            }) => self.transcript = other,
        }
    }

    /// Die Tonspur ist da (oder nicht): ans Modell geben. Ist der
    /// Modell-Thread weg, ist Diktator danach nicht verfügbar.
    pub(super) fn on_loaded(&mut self, state: State, result: Result<Loaded, LoadError>) -> State {
        let Some(fetch) = self.take_downloading() else {
            return state;
        };
        let kind = fetch.origin.kind();
        match result {
            // Abgebrochen ist abgebrochen, auch wenn das Laden dabei mit
            // einem anderen Fehler endete.
            Ok(_) | Err(_) if fetch.cancel.load(Ordering::Relaxed) => {
                self.note(cancelled(kind));
                state
            }
            Ok(loaded) => self.transcribe_loaded(state, fetch, loaded),
            Err(LoadError::Cancelled) => {
                self.note(cancelled(kind));
                state
            }
            Err(error) => {
                self.feedback.play(Sound::Error);
                self.note(error.note().around(
                    |de| format!("{kind}: {de}"),
                    |en| format!("{}: {en}", kind.english_label()),
                ));
                state
            }
        }
    }

    /// Nimmt den laufenden Ladevorgang aus dem Zustand und wartet auf
    /// seinen Thread. `None` (und der Zustand bleibt), wenn gerade nichts
    /// geladen wird.
    fn take_downloading(&mut self) -> Option<Fetch> {
        match std::mem::replace(&mut self.transcript, TranscriptJob::Idle) {
            TranscriptJob::Downloading {
                origin,
                cancel,
                thread,
                translate,
                title: _,
            } => {
                if thread.join().is_err() {
                    self.feedback.log("Lade-Thread ist abgestürzt");
                }
                Some(Fetch {
                    origin,
                    cancel,
                    translate,
                })
            }
            other @ (TranscriptJob::Idle
            | TranscriptJob::Transcribing {
                id: _,
                origin: _,
                cancel: _,
                title: _,
                percent: _,
                language: _,
                translate: _,
            }
            | TranscriptJob::Translating {
                origin: _,
                cancel: _,
                title: _,
                original: _,
                languages: _,
                percent: _,
            }) => {
                self.transcript = other;
                self.feedback
                    .log("Ladeergebnis ohne laufenden Ladevorgang verworfen");
                None
            }
        }
    }

    /// Gibt die geladene Tonspur an das Modell; ist der Modell-Thread weg,
    /// ist Diktator danach nicht verfügbar.
    fn transcribe_loaded(&mut self, state: State, fetch: Fetch, loaded: Loaded) -> State {
        let Fetch {
            origin,
            cancel,
            translate,
        } = fetch;
        let Loaded {
            title,
            language,
            pcm,
            begin,
            chapters,
        } = loaded;
        let seconds = pcm.duration().as_secs();
        // „45 s“ statt „0 min“.
        let length = match seconds.checked_div(60).unwrap_or(0) {
            0 => format!("{seconds} s"),
            minutes => format!("{minutes} min"),
        };
        let id = TranscriptId(self.next_transcript);
        self.next_transcript = self.next_transcript.wrapping_add(1);
        let job = Job::Transcript {
            id,
            pcm,
            language,
            begin,
            timestamps: self.timestamps,
            chapters: match self.chapters {
                ChapterHeadings::On => chapters,
                ChapterHeadings::Off => Vec::new(),
            },
            cancel: Arc::clone(&cancel),
        };
        if self.jobs.send(job).is_err() {
            return self.worker_gone(state);
        }
        self.note(Note::showing(|shown| {
            (
                format!(
                    "„{}“ ({language}, {length}) wird transkribiert …",
                    shown.text(&title)
                ),
                format!(
                    "Transcribing “{}” ({language}, {length}) …",
                    shown.text(&title)
                ),
            )
        }));
        self.transcript = TranscriptJob::Transcribing {
            id,
            origin,
            cancel,
            title,
            percent: Percent::ZERO,
            language,
            translate,
        };
        state
    }

    /// Fortschritt in der Menüleiste; Fortschritt eines anderen Auftrags
    /// fällt weg.
    pub(super) fn on_transcript_progress(&mut self, id: TranscriptId, progress: Progress) {
        match &mut self.transcript {
            TranscriptJob::Transcribing {
                id: running,
                percent,
                origin: _,
                cancel: _,
                title: _,
                language: _,
                translate: _,
            } if *running == id => *percent = Percent::of(progress.done, progress.total),
            TranscriptJob::Transcribing {
                id: _,
                origin: _,
                cancel: _,
                title: _,
                percent: _,
                language: _,
                translate: _,
            }
            | TranscriptJob::Idle
            | TranscriptJob::Downloading {
                origin: _,
                title: _,
                cancel: _,
                thread: _,
                translate: _,
            }
            | TranscriptJob::Translating {
                origin: _,
                cancel: _,
                title: _,
                original: _,
                languages: _,
                percent: _,
            } => self
                .feedback
                .log("Fortschritt ohne laufenden Auftrag verworfen"),
        }
    }

    /// Das Modell ist mit dem Video bzw. der Datei `id` fertig. Ist der
    /// Auftrag inzwischen abgebrochen, zählt ein vollständiges Ergebnis als
    /// abgebrochen (die Meldung sagte schon „wird abgebrochen“).
    pub(super) fn on_transcript_finished(&mut self, id: TranscriptId, outcome: TranscriptOutcome) {
        let Some((recognized, translation)) = self.take_transcribing(id) else {
            return;
        };
        let outcome = match outcome {
            TranscriptOutcome::Finished(text) if recognized.cancel.load(Ordering::Relaxed) => {
                TranscriptOutcome::Cancelled(text)
            }
            other @ (TranscriptOutcome::Finished(_)
            | TranscriptOutcome::Cancelled(_)
            | TranscriptOutcome::Failed {
                partial: _,
                error: _,
            }) => other,
        };
        match outcome {
            TranscriptOutcome::Finished(transcript) => {
                if let Some(languages) = translation {
                    self.start_translation(recognized, transcript, languages);
                } else {
                    let headline =
                        self.save_beside(&recognized.origin, &recognized.title, &transcript);
                    self.deliver_transcript(
                        recognized.origin.kind(),
                        &recognized.title,
                        &transcript,
                        &headline,
                    );
                }
            }
            TranscriptOutcome::Cancelled(partial) if partial.is_empty() => {
                self.note(Note::new(
                    "Transkript abgebrochen.",
                    "Transcript cancelled.",
                ));
            }
            TranscriptOutcome::Cancelled(partial) => {
                self.deliver_transcript(
                    recognized.origin.kind(),
                    &recognized.title,
                    &partial,
                    &Headline::new(
                        Note::new(
                            "Abgebrochen, bisheriger Teil",
                            "Cancelled, the part so far was",
                        ),
                        None,
                    ),
                );
            }
            TranscriptOutcome::Failed { partial, error } => {
                self.transcription_failed(&recognized, &partial, &error);
            }
        }
    }

    /// Nimmt die laufende Erkennung `id` aus dem Zustand, samt dem
    /// Sprachpaar, falls danach übersetzt wird. `None` (und der Zustand
    /// bleibt), wenn gerade keine Erkennung mit dieser Nummer läuft.
    fn take_transcribing(
        &mut self,
        id: TranscriptId,
    ) -> Option<(Recognized, Option<LanguagePair>)> {
        let previous = std::mem::replace(&mut self.transcript, TranscriptJob::Idle);
        match previous {
            TranscriptJob::Transcribing {
                id: running,
                origin,
                title,
                cancel,
                language,
                translate,
                percent: _,
            } if running == id => Some((
                Recognized {
                    origin,
                    title,
                    cancel,
                },
                // Ist das Video schon in der Zielsprache, gibt es nichts zu tun.
                translate
                    .filter(|to| *to != language)
                    .map(|to| LanguagePair { from: language, to }),
            )),
            other @ (TranscriptJob::Transcribing {
                id: _,
                origin: _,
                cancel: _,
                title: _,
                percent: _,
                language: _,
                translate: _,
            }
            | TranscriptJob::Idle
            | TranscriptJob::Downloading {
                origin: _,
                title: _,
                cancel: _,
                thread: _,
                translate: _,
            }
            | TranscriptJob::Translating {
                origin: _,
                cancel: _,
                title: _,
                original: _,
                languages: _,
                percent: _,
            }) => {
                self.transcript = other;
                self.feedback
                    .log("Transkript ohne laufenden Auftrag verworfen");
                None
            }
        }
    }

    /// Die Erkennung scheiterte. Was bis zum Fehler erkannt war, geht nicht
    /// verloren; ohne erkannten Text bleibt nur die Meldung.
    fn transcription_failed(
        &mut self,
        recognized: &Recognized,
        partial: &str,
        error: &EngineError,
    ) {
        let title = &recognized.title;
        if partial.is_empty() {
            self.feedback.play(Sound::Error);
            self.note(Note::showing(|shown| {
                (
                    format!(
                        "„{}“: Transkription fehlgeschlagen: {error}",
                        shown.text(title)
                    ),
                    format!(
                        "“{}”: transcription failed: {}",
                        shown.text(title),
                        error.english()
                    ),
                )
            }));
            return;
        }
        self.deliver_transcript(
            recognized.origin.kind(),
            title,
            partial,
            &Headline::new(
                Note::new(
                    format!("Transkription fehlgeschlagen ({error}), bisheriger Teil"),
                    format!(
                        "Transcription failed ({}), the part so far was",
                        error.english()
                    ),
                ),
                None,
            ),
        );
    }

    /// Gibt das fertige Transkript zum Übersetzen an den
    /// Überarbeiten-Thread.
    fn start_translation(
        &mut self,
        recognized: Recognized,
        original: String,
        languages: LanguagePair,
    ) {
        let Recognized {
            origin,
            title,
            cancel,
        } = recognized;
        // Titel und Original bleiben hier: Bei Abbruch wird das Original kopiert.
        let request = TranslationRequest {
            title: title.shown().to_owned(),
            text: original.clone(),
            from: languages.from,
            to: languages.to,
            cancel: Arc::clone(&cancel),
        };
        if self.polish.send(PolishRequest::Translate(request)).is_err() {
            self.feedback.play(Sound::Error);
            let headline = Headline::new(
                Note::new(
                    "Nicht übersetzt (Sprachmodell nicht erreichbar), Original",
                    "Not translated (language model unreachable), original",
                ),
                None,
            );
            self.deliver_transcript(origin.kind(), &title, &original, &headline);
            return;
        }
        self.note(Note::showing(|shown| {
            (
                format!(
                    "„{}“ wird übersetzt ({} → {}) …",
                    shown.text(&title),
                    languages.from,
                    languages.to
                ),
                format!(
                    "Translating “{}” ({} → {}) …",
                    shown.text(&title),
                    languages.from,
                    languages.to
                ),
            )
        }));
        self.transcript = TranscriptJob::Translating {
            origin,
            cancel,
            title,
            original,
            languages,
            percent: Percent::ZERO,
        };
    }

    /// Nachricht vom Übersetzen.
    pub(super) fn on_translation(&mut self, event: TranslationEvent) {
        match event {
            TranslationEvent::Progress { done, total } => match &mut self.transcript {
                TranscriptJob::Translating {
                    percent,
                    origin: _,
                    cancel: _,
                    title: _,
                    original: _,
                    languages: _,
                } => *percent = Percent::of(done, total),
                TranscriptJob::Idle
                | TranscriptJob::Downloading {
                    origin: _,
                    title: _,
                    cancel: _,
                    thread: _,
                    translate: _,
                }
                | TranscriptJob::Transcribing {
                    id: _,
                    origin: _,
                    cancel: _,
                    title: _,
                    percent: _,
                    language: _,
                    translate: _,
                } => self
                    .feedback
                    .log("Übersetzungsfortschritt ohne laufende Übersetzung verworfen"),
            },
            TranslationEvent::Done(outcome) => self.on_translation_done(outcome),
        }
    }

    /// Die Übersetzung ist zu Ende: Übersetzung oder Original kopieren.
    fn on_translation_done(&mut self, outcome: TranslationOutcome) {
        let (origin, title, original, languages) =
            match std::mem::replace(&mut self.transcript, TranscriptJob::Idle) {
                TranscriptJob::Translating {
                    origin,
                    title,
                    original,
                    languages,
                    cancel: _,
                    percent: _,
                } => (origin, title, original, languages),
                other @ (TranscriptJob::Idle
                | TranscriptJob::Downloading {
                    origin: _,
                    title: _,
                    cancel: _,
                    thread: _,
                    translate: _,
                }
                | TranscriptJob::Transcribing {
                    id: _,
                    origin: _,
                    cancel: _,
                    title: _,
                    percent: _,
                    language: _,
                    translate: _,
                }) => {
                    self.transcript = other;
                    self.feedback
                        .log("Übersetzung ohne laufenden Auftrag verworfen");
                    return;
                }
            };
        let kind = origin.kind();
        match outcome {
            TranslationOutcome::Finished(translated) => match &translated.untranslated {
                Some(untranslated) if untranslated.count >= translated.total => {
                    self.feedback.play(Sound::Error);
                    let headline = not_translated(&untranslated.first_error);
                    self.deliver_transcript(kind, &title, &original, &headline);
                }
                Some(_) | None => self.deliver_translation(kind, &translated, languages),
            },
            TranslationOutcome::Cancelled => {
                self.deliver_transcript(
                    kind,
                    &title,
                    &original,
                    &Headline::new(
                        Note::new(
                            "Übersetzung abgebrochen, Original",
                            "Translation cancelled, original",
                        ),
                        None,
                    ),
                );
            }
        }
    }

    /// Legt die (ganz oder teils) gelungene Übersetzung in die
    /// Zwischenablage.
    fn deliver_translation(
        &mut self,
        kind: SourceKind,
        translated: &Translated,
        languages: LanguagePair,
    ) {
        let full_title = translated_title(translated, languages.from, self.language);
        let headline = translation_headline(translated, languages.to);
        self.deliver_transcript(kind, &full_title, &translated.text, &headline);
    }

    /// Speichert ein Datei-Transkript als Textdatei neben der Datei; liefert
    /// die Überschrift der Meldung.
    fn save_beside(&self, origin: &Origin, title: &UserText, transcript: &str) -> Headline {
        let subject = Note::new("Transkript", "Transcript");
        // Nur Dateien haben einen Ordner, in den das Transkript gehört.
        let Origin::File(path) = origin else {
            return Headline::new(subject, None);
        };
        match source::save_beside(path, &full_text(title, transcript)) {
            Ok(saved) => {
                self.feedback.log("Transkript neben der Datei gespeichert");
                // `save_beside` baut den Namen selbst; einen Pfad ohne
                // Namen liefert es nicht.
                let name = UserText::new(
                    saved
                        .file_name()
                        .map_or_else(|| saved.to_string_lossy(), |name| name.to_string_lossy()),
                );
                Headline::new(
                    subject,
                    Some(Note::showing(|shown| {
                        (
                            format!(" und als „{}“ gespeichert", shown.text(&name)),
                            format!(" and saved as “{}”", shown.text(&name)),
                        )
                    })),
                )
            }
            Err(error) => {
                self.feedback.play(Sound::Error);
                Headline::new(
                    subject,
                    Some(Note::new(
                        format!(", aber nicht gespeichert ({error})"),
                        format!(" but not saved ({})", error.english()),
                    )),
                )
            }
        }
    }

    /// Legt Titel und Transkript in die Zwischenablage und merkt es sich als
    /// letztes dieser Art. Scheitert das Kopieren, sagt die Meldung trotzdem
    /// alles andere aus `headline` (gespeichert, Fehler davor).
    fn deliver_transcript(
        &mut self,
        kind: SourceKind,
        title: &UserText,
        transcript: &str,
        headline: &Headline,
    ) {
        let full = full_text(title, transcript);
        let words = Words::of(transcript);
        let (sound, copied) = match self.platform.copy(&full) {
            Ok(()) => (Sound::Done, Note::new("kopiert", "copied")),
            Err(error) => (
                Sound::Error,
                Note::new(
                    format!("nicht kopiert ({error})"),
                    format!("not copied ({})", error.english()),
                ),
            ),
        };
        let headline = headline.with(&copied);
        let message = Note::showing(|shown| {
            (
                format!(
                    "{}: „{}“ ({})",
                    headline.de_for(shown),
                    shown.text(title),
                    words.de()
                ),
                format!(
                    "{}: “{}” ({})",
                    headline.en(),
                    shown.text(title),
                    words.en()
                ),
            )
        });
        self.feedback.play(sound);
        self.feedback.notify(message.clone());
        self.note(message);
        self.last_transcripts.set(kind, full);
    }

    /// Bricht das laufende Video bzw. die laufende Datei ab; wartende
    /// fallen weg.
    pub(super) fn cancel_transcript(&mut self) {
        let dropped = self.waiting.len();
        self.waiting.clear();
        match &self.transcript {
            TranscriptJob::Downloading {
                cancel,
                origin: _,
                title: _,
                thread: _,
                translate: _,
            }
            | TranscriptJob::Transcribing {
                cancel,
                id: _,
                origin: _,
                title: _,
                percent: _,
                language: _,
                translate: _,
            }
            | TranscriptJob::Translating {
                cancel,
                origin: _,
                title: _,
                original: _,
                languages: _,
                percent: _,
            } => {
                cancel.store(true, Ordering::Relaxed);
                self.note(match dropped {
                    0 => Note::new("Transkript wird abgebrochen …", "Cancelling transcript …"),
                    dropped => Note::new(
                        format!("Transkript wird abgebrochen … ({dropped} wartende verworfen)"),
                        format!("Cancelling transcript … ({dropped} waiting discarded)"),
                    ),
                });
            }
            TranscriptJob::Idle => self.note(Note::new(
                "Es läuft kein Transkript.",
                "No transcript is running.",
            )),
        }
    }

    /// Ist kein Transkript in Arbeit, kommt das nächste wartende dran.
    pub(super) fn start_waiting(&mut self, state: &State) {
        match self.transcript {
            TranscriptJob::Idle => {
                if let Some(source) = self.waiting.pop_front() {
                    self.start_transcript(state, source);
                }
            }
            TranscriptJob::Downloading {
                origin: _,
                title: _,
                cancel: _,
                thread: _,
                translate: _,
            }
            | TranscriptJob::Transcribing {
                id: _,
                origin: _,
                cancel: _,
                title: _,
                percent: _,
                language: _,
                translate: _,
            }
            | TranscriptJob::Translating {
                origin: _,
                cancel: _,
                title: _,
                original: _,
                languages: _,
                percent: _,
            } => {}
        }
    }

    /// Legt das letzte Transkript dieser Art erneut in die Zwischenablage.
    pub(super) fn copy_last_transcript(&self, kind: SourceKind) {
        let Some(transcript) = self.last_transcripts.get(kind) else {
            self.note(Note::new(
                format!("Noch kein {kind}-Transkript seit dem Start."),
                format!("No {} transcript since start.", kind.english_label()),
            ));
            return;
        };
        match self.platform.copy(transcript) {
            Ok(()) => self.note(Note::new(
                format!("Letztes {kind}-Transkript kopiert."),
                format!("Last {} transcript copied.", kind.english_label()),
            )),
            Err(error) => {
                self.feedback.play(Sound::Error);
                self.note(Note::new(
                    format!("Kopieren fehlgeschlagen: {error}"),
                    format!("Copying failed: {}", error.english()),
                ));
            }
        }
    }

    /// Stand des langen Transkripts fürs Panel.
    pub(super) fn transcript_view(&self) -> TranscriptView {
        match &self.transcript {
            TranscriptJob::Idle => TranscriptView::Idle,
            TranscriptJob::Downloading {
                origin,
                title,
                cancel: _,
                thread: _,
                translate: _,
            } => TranscriptView::Loading {
                kind: origin.kind(),
                title: title.clone(),
                queued: self.waiting.len(),
            },
            TranscriptJob::Transcribing {
                title,
                percent,
                id: _,
                origin: _,
                cancel: _,
                language: _,
                translate: _,
            } => TranscriptView::Transcribing {
                title: title.clone(),
                percent: *percent,
                queued: self.waiting.len(),
            },
            TranscriptJob::Translating {
                title,
                languages,
                percent,
                origin: _,
                cancel: _,
                original: _,
            } => TranscriptView::Translating {
                title: title.clone(),
                to: languages.to,
                percent: *percent,
                queued: self.waiting.len(),
            },
        }
    }
}

/// Überschrift der Meldung zu einem fertigen Transkript: „{vorher}
/// kopiert{danach}“, etwa „Transkript kopiert und als „x.txt“ gespeichert“;
/// scheitert das Kopieren, steht statt „kopiert“, warum nicht.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Headline {
    /// Was kopiert wird („Transkript“, „Abgebrochen, bisheriger Teil“).
    before: Note,
    /// Was noch geschah (gespeichert, womit übersetzt); beginnt mit dem
    /// Trennzeichen.
    after: Option<Note>,
}

impl Headline {
    /// Eine Überschrift aus `before` und `after`.
    const fn new(before: Note, after: Option<Note>) -> Self {
        Self { before, after }
    }

    /// Die Überschrift mit `copied` („kopiert“ oder warum nicht).
    fn with(&self, copied: &Note) -> Note {
        Note::showing(|shown| {
            let (after_de, after_en) = self
                .after
                .as_ref()
                .map_or(("", ""), |after| (after.de_for(shown), after.en()));
            (
                format!(
                    "{} {}{after_de}",
                    self.before.de_for(shown),
                    copied.de_for(shown)
                ),
                format!("{} {}{after_en}", self.before.en(), copied.en()),
            )
        })
    }
}

/// Anteil in Prozent.
/// Titel der Übersetzung: „Hallo zusammen (aus dem Spanischen)“, auf
/// Englisch „Hallo zusammen (translated from Spanish)“.
fn translated_title(translated: &Translated, from: Language, language: UiLanguage) -> UserText {
    UserText::new(match language {
        UiLanguage::German => format!("{} (aus dem {}en)", translated.title, from.name()),
        UiLanguage::English => format!(
            "{} (translated from {})",
            translated.title,
            from.english_name()
        ),
    })
}

/// Überschrift der Meldung: „Ins Deutsche übersetzt und kopiert“, dazu,
/// was im Original blieb oder womit übersetzt wurde.
fn translation_headline(translated: &Translated, to: Language) -> Headline {
    let total = translated.total;
    let (kept_de, kept_en) = translated.untranslated.as_ref().map_or_else(
        || (String::new(), String::new()),
        |Untranslated { count, first_error }| {
            (
                // Der Titel zählt mit: So viele Stücke übersetzt der
                // Überarbeiten-Thread.
                format!(
                    " ({count} von {total} Stücken (Titel, Überschriften, Absätze) im Original: {first_error})"
                ),
                format!(
                    " ({count} of {total} pieces (title, headings, paragraphs) left in the original: {})",
                    first_error.english()
                ),
            )
        },
    );
    let fallback = translated.fallback.as_ref().map(super::apple_fallback);
    let (fallback_de, fallback_en) = fallback.map_or_else(
        || (String::new(), String::new()),
        |note| (format!(" – {}", note.de()), format!(" – {}", note.en())),
    );
    let after = (translated.untranslated.is_some() || translated.fallback.is_some()).then(|| {
        Note::new(
            format!("{kept_de}{fallback_de}"),
            format!("{kept_en}{fallback_en}"),
        )
    });
    Headline::new(
        Note::new(
            format!("Ins {}e übersetzt und", to.name()),
            format!("Translated into {} and", to.english_name()),
        ),
        after,
    )
}

/// „`YouTube`: lade Tonspur …“, oder mit bekanntem Titel „„…“ wird
/// gelesen …“.
fn loading_note(kind: SourceKind, title: Option<&UserText>) -> Note {
    let Some(title) = title else {
        return Note::new(
            format!("{kind}: lade Tonspur …"),
            format!("{}: loading audio …", kind.english_label()),
        );
    };
    Note::showing(|shown| {
        (
            format!("„{}“ wird gelesen …", shown.text(title)),
            format!("Reading “{}” …", shown.text(title)),
        )
    })
}

/// Überschrift, wenn kein Abschnitt übersetzt wurde und das Original
/// kommt; nennt den ersten Fehler.
fn not_translated(first_error: &PolishError) -> Headline {
    Headline::new(
        Note::new(
            format!("Nicht übersetzt ({first_error}), Original"),
            format!("Not translated ({}), original", first_error.english()),
        ),
        None,
    )
}

/// „`YouTube` abgebrochen.“ bzw. „Datei abgebrochen.“
fn cancelled(kind: SourceKind) -> Note {
    Note::new(
        format!("{kind} abgebrochen."),
        format!("{} cancelled.", kind.english_label()),
    )
}

/// Titel und Transkript, wie sie in Zwischenablage und Datei stehen.
fn full_text(title: &UserText, transcript: &str) -> String {
    format!("{}\n\n{transcript}", title.shown())
}
