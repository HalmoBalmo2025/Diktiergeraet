//! Die Steuerung: ein Zustandsautomat über Tasten, Panel und Modell-Ergebnisse.
//!
//! Diktat (`dictation.rs`):
//! ```text
//! Lädt ──Modell bereit──► Bereit ──fn──► Aufnahme ──fn──► Verarbeitung ──Text──► Bereit
//!  │                         ▲              │ Esc                │ mit ⌥
//!  │ fn                      └──────────────┘                    ▼
//!  └──► Aufnahme …                                         Überarbeiten ──Text──► Bereit
//!
//! Modell lädt nicht / Modell-Thread weg ──► Nicht verfügbar (aus jedem Zustand)
//! ```
//! Das laufende Diktat gehört zum Zustand: Es entsteht mit der Aufnahme und
//! endet, wenn der Text eingefügt ist. Ein Diktat beginnt auch schon,
//! während das Modell lädt; danach geht es zurück zu „lädt“, solange das
//! Modell nicht bereit ist ([`Model`]). Ein Diktat, das „nicht verfügbar“
//! trifft, endet mit dem schon erkannten Text.
//!
//! Lange Transkripte (`YouTube`-Video oder Datei, `transcript.rs`) laufen
//! daneben: Leerlauf → Laden (eigener Thread) → Transkribieren
//! (Modell-Thread, Diktate haben Vorrang) → Leerlauf.
//!
//! Die Aufnahme lebt auf diesem Thread (cpal-Streams wechseln den Thread
//! nicht); transkribiert wird im Modell-Thread, überarbeitet im
//! Überarbeiten-Thread, eingefügt wieder hier. Jeder Schritt steht im
//! Protokoll (`~/Library/Logs/Diktator.log`).

mod dictation;
mod platform;
#[cfg(test)]
mod tests;
mod transcript;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use self::dictation::Dictation;
pub(crate) use self::platform::{Mac, Microphone, Platform};
use self::transcript::{LastTranscripts, TranscriptJob};
use super::history::{History, HistorySerial};
use super::source::{LanguageChoice, LoadError, Loaded, Source, SourceKind};
use super::status::{Status, UiUpdate};
use super::worker::{Job, TranscriptOutcome, WorkerEvent};
use crate::audio::Level;
use crate::audio::chunk::PartTiming;
use crate::audio::range::RangeInput;
use crate::dictation_id::DictationId;
use crate::engine::EngineError;
use crate::feedback::{Feedback, Sound};
use crate::hotkey::{HotkeyError, HotkeyEvent, Mode, Slot, Tap};
use crate::language::Language;
use crate::note::{Explain, Note, UiLanguage, Words};
use crate::polish::{PolishDone, PolishRequest, TranslationEvent, TranslationOutcome};
use crate::text::dictionary::{self, DictionaryError};
use crate::text::{ChapterHeadings, Timestamps};
use crate::{paths, text};

/// Befehle aus dem Panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PanelCommand {
    /// Video aus der Zwischenablage transkribieren.
    Youtube {
        /// Sprache, automatisch oder fest.
        language: LanguageChoice,
        /// Nur dieser Bereich (leere Felder = alles).
        range: RangeInput,
        /// Danach in diese Sprache übersetzen.
        translate: Option<Language>,
    },
    /// Dateien transkribieren: die aufs Panel gezogenen oder die im Finder
    /// kopierten, nacheinander.
    File {
        /// Sprache der Aufnahmen.
        language: Language,
        /// Die gezogenen Dateien; leer = aus der Zwischenablage.
        paths: Vec<PathBuf>,
        /// Nur dieser Bereich (leere Felder = alles; nur bei einer Datei).
        range: RangeInput,
    },
    /// Laufendes Video bzw. laufende Datei abbrechen.
    CancelTranscript,
    /// Dem Panel sagen, ob die Zwischenablage einen YouTube-Link hält.
    CheckLink,
    /// Letztes Transkript dieser Art erneut in die Zwischenablage legen.
    CopyLastTranscript(SourceKind),
    /// Wörterbuch im Texteditor öffnen.
    OpenDictionary,
    /// Diktat aus dem Verlauf (fortlaufende Nummer) in die Zwischenablage.
    CopyHistory(HistorySerial),
    /// Das Panel wurde geöffnet oder geschlossen.
    Visibility(PanelState),
    /// Neue Sprache für fn + ⇧ (gilt ab der nächsten Aufnahme).
    SecondaryLanguage(Language),
    /// Zeitmarken in langen Transkripten (gilt ab dem nächsten).
    Timestamps(Timestamps),
    /// Kapitelüberschriften in Video-Transkripten (gilt ab dem nächsten).
    Chapters(ChapterHeadings),
    /// Wann ein Diktat in Teilen transkribiert wird (gilt ab der nächsten
    /// Aufnahme: eine laufende behält ihre, sonst könnte ein Teil länger
    /// werden, als [`PartTiming`] verspricht).
    Parts(PartTiming),
    /// Sprache der Oberfläche (für den Titel übersetzter Transkripte).
    Language(UiLanguage),
}

/// Die Einstellungen, die die Steuerung liest und die sich zur Laufzeit
/// über [`PanelCommand`]s ändern. Beim Start kommen sie aus `settings.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Live {
    /// Sprache für fn + ⇧.
    pub secondary: Language,
    /// Zeitmarken in langen Transkripten.
    pub timestamps: Timestamps,
    /// Kapitelüberschriften in Video-Transkripten.
    pub chapters: ChapterHeadings,
    /// Wann ein Diktat in Teilen transkribiert wird.
    pub parts: PartTiming,
    /// Sprache der Oberfläche.
    pub language: UiLanguage,
}

/// Die Leitungen der Steuerung zu den anderen Threads.
#[derive(Debug)]
pub(crate) struct Connections {
    /// Der Eingang der Steuerung selbst (für Lade-Threads).
    pub events: Sender<ControlEvent>,
    /// Aufträge an den Modell-Thread.
    pub jobs: Sender<Job>,
    /// Aufträge an den Überarbeiten-Thread.
    pub polish: Sender<PolishRequest>,
    /// Sagt dem Tasten-Mitleser, ob Esc gerade abgefangen werden soll.
    pub escape_capture: Arc<AtomicBool>,
}

/// Ob das Panel offen ist. Ist es offen, hat Diktator den Fokus; ein Diktat
/// landet dann in der Zwischenablage statt im Panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelState {
    /// Offen.
    Open,
    /// Geschlossen.
    Closed,
}

/// Alles, worauf die Steuerung reagiert.
#[derive(Debug)]
pub(crate) enum ControlEvent {
    /// Eine Taste wurde ausgelöst.
    Hotkey(HotkeyEvent),
    /// Der Tasten-Mitleser ist ausgefallen.
    HotkeyFailed(HotkeyError),
    /// Ein Befehl aus dem Panel.
    Panel(PanelCommand),
    /// Das Laden eines Videos oder einer Datei ist beendet.
    SourceLoaded(Result<Loaded, LoadError>),
    /// Nachricht vom Modell-Thread.
    Worker(WorkerEvent),
    /// Taktgeber (alle halbe Sekunde): Während einer Aufnahme wird nachgesehen,
    /// ob ein Teil bis zu einer Sprechpause fertig ist.
    Tick,
    /// Pegeltakt (achtmal pro Sekunde): Während einer Aufnahme geht der
    /// Pegel an die Anzeige an der Schreibmarke.
    Meter,
    /// Der Überarbeiten-Thread ist mit einem Diktat fertig.
    Polished(PolishDone),
    /// Nachricht vom Übersetzen eines langen Transkripts.
    Translation(TranslationEvent),
    /// Diktator wird beendet: laufende Arbeit abbrechen und enden.
    Shutdown,
}

impl From<HotkeyEvent> for ControlEvent {
    fn from(event: HotkeyEvent) -> Self {
        Self::Hotkey(event)
    }
}

impl From<WorkerEvent> for ControlEvent {
    fn from(event: WorkerEvent) -> Self {
        Self::Worker(event)
    }
}

impl From<PolishDone> for ControlEvent {
    fn from(done: PolishDone) -> Self {
        Self::Polished(done)
    }
}

impl From<TranslationEvent> for ControlEvent {
    fn from(event: TranslationEvent) -> Self {
        Self::Translation(event)
    }
}

/// Ob ein Ereignis ein Takt ist (die kommen ständig und werden nicht
/// protokolliert).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Routine {
    /// Kein Takt.
    No,
    /// Takt für fertige Teile.
    Tick,
    /// Takt für die Pegelanzeige.
    Meter,
}

/// Ob das Erkennungsmodell schon einmal fertig geladen war. Getrennt vom
/// Zustand des Diktats, weil ein Diktat schon während des Ladens beginnt:
/// Endet es, geht es zurück zu „lädt“, nicht zu „bereit“.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Model {
    /// Lädt noch.
    Loading,
    /// Geladen.
    Ready,
}

/// Zustand des Diktats.
enum State {
    /// Das Modell wird geladen; fn nimmt trotzdem schon auf.
    Loading,
    /// Bereit für fn.
    Ready,
    /// Aufnahme läuft; fertige Teile werden schon transkribiert.
    Recording {
        /// Die laufende Aufnahme.
        recording: Box<dyn Microphone>,
        /// Was davon schon erkannt ist.
        dictation: Dictation,
    },
    /// Das Modell transkribiert den Rest.
    Processing(Dictation),
    /// Das Sprachmodell überarbeitet das fertige Diktat.
    Polishing(Dictation),
    /// Das Modell fehlt oder ließ sich nicht laden.
    Unavailable {
        /// Was dem Nutzer bei fn erneut gesagt wird.
        reason: Note,
    },
}

impl State {
    /// Name für das Protokoll.
    fn name(&self) -> String {
        match self {
            Self::Loading => "lädt".to_owned(),
            Self::Ready => "bereit".to_owned(),
            Self::Recording {
                dictation,
                recording: _,
            } => format!("Aufnahme {}", dictation.language),
            Self::Processing(_) => "verarbeitet".to_owned(),
            Self::Polishing(_) => "überarbeitet".to_owned(),
            Self::Unavailable { reason: _ } => "nicht verfügbar".to_owned(),
        }
    }

    /// Ob gerade aufgenommen wird.
    const fn is_recording(&self) -> bool {
        match self {
            Self::Recording {
                recording: _,
                dictation: _,
            } => true,
            Self::Loading
            | Self::Ready
            | Self::Processing(_)
            | Self::Polishing(_)
            | Self::Unavailable { reason: _ } => false,
        }
    }

    /// Ob Esc gerade Diktator gehört: solange ein Diktat läuft.
    const fn captures_escape(&self) -> bool {
        match self {
            Self::Recording {
                recording: _,
                dictation: _,
            }
            | Self::Processing(_)
            | Self::Polishing(_) => true,
            Self::Loading | Self::Ready | Self::Unavailable { reason: _ } => false,
        }
    }

    /// Das laufende Diktat, falls eins läuft; eine laufende Aufnahme endet
    /// dabei.
    fn into_dictation(self) -> Option<Dictation> {
        match self {
            Self::Recording {
                dictation,
                recording: _,
            }
            | Self::Processing(dictation)
            | Self::Polishing(dictation) => Some(dictation),
            Self::Loading | Self::Ready | Self::Unavailable { reason: _ } => None,
        }
    }

    /// Das laufende Diktat, wenn es die Nummer `id` hat. Ergebnisse
    /// verworfener Diktate finden so keins.
    fn dictation_mut(&mut self, id: DictationId) -> Option<&mut Dictation> {
        match self {
            Self::Recording {
                dictation,
                recording: _,
            }
            | Self::Processing(dictation)
            | Self::Polishing(dictation) => Some(dictation).filter(|dictation| dictation.id == id),
            Self::Loading | Self::Ready | Self::Unavailable { reason: _ } => None,
        }
    }
}

/// Die Steuerung. Läuft mit [`Controller::run`] auf einem eigenen Thread.
pub(crate) struct Controller<P: Platform> {
    state: State,
    /// Ob das Modell schon geladen ist.
    model: Model,
    /// Das lange Transkript, falls eins läuft.
    transcript: TranscriptJob,
    /// Was danach drankommt (mehrere Dateien auf einmal).
    waiting: VecDeque<Source>,
    /// Letztes Transkript je Art der Quelle.
    last_transcripts: LastTranscripts,
    history: History,
    panel: PanelState,
    /// Nummer des nächsten Diktats.
    next_dictation: DictationId,
    /// Nummer des nächsten langen Auftrags beim Modell-Thread.
    next_transcript: u64,
    /// Sprache für fn + ⇧.
    secondary: Language,
    /// Zeitmarken in langen Transkripten?
    timestamps: Timestamps,
    /// Kapitelüberschriften in Video-Transkripten?
    chapters: ChapterHeadings,
    /// Wann ein Diktat in Teilen transkribiert wird (gilt ab der nächsten
    /// Aufnahme).
    parts: PartTiming,
    /// Sprache der Oberfläche.
    language: UiLanguage,
    /// Zuletzt angezeigte Pegelstufe.
    meter: Level,
    /// Dauerhaftes Problem (Tasten ausgefallen); zeigt ⚠ in der Menüleiste.
    problem: Option<Note>,
    /// Eingang der Steuerung selbst (für Lade-Threads).
    events: Sender<ControlEvent>,
    /// Aufträge an den Modell-Thread.
    jobs: Sender<Job>,
    /// Aufträge an den Überarbeiten-Thread.
    polish: Sender<PolishRequest>,
    escape_capture: Arc<AtomicBool>,
    feedback: Feedback,
    platform: P,
}

impl<P: Platform> Controller<P> {
    /// Neue Steuerung im Zustand „Modell lädt“.
    ///
    /// `platform` sind Mikrofon, Zwischenablage und Anzeige; `live` sind
    /// die Einstellungen beim Start.
    pub(crate) fn new(
        connections: Connections,
        feedback: Feedback,
        live: Live,
        platform: P,
    ) -> Self {
        let Connections {
            events,
            jobs,
            polish,
            escape_capture,
        } = connections;
        let Live {
            secondary,
            timestamps,
            chapters,
            parts,
            language,
        } = live;
        feedback.log("Diktator gestartet, Modell lädt");
        let controller = Self {
            state: State::Loading,
            model: Model::Loading,
            transcript: TranscriptJob::Idle,
            waiting: VecDeque::new(),
            last_transcripts: LastTranscripts::default(),
            history: History::default(),
            panel: PanelState::Closed,
            next_dictation: DictationId::FIRST,
            next_transcript: 0,
            secondary,
            timestamps,
            chapters,
            parts,
            language,
            meter: Level::SILENT,
            problem: None,
            events,
            jobs,
            polish,
            escape_capture,
            feedback,
            platform,
        };
        controller.refresh_status();
        controller
    }

    /// Verarbeitet Ereignisse bis [`ControlEvent::Shutdown`]. Die Steuerung
    /// hält selbst einen Sender auf ihren Eingang (für Lade-Threads); ohne
    /// dieses Ereignis endete sie nie.
    pub(crate) fn run(mut self, receiver: &Receiver<ControlEvent>) {
        for event in receiver {
            match event {
                ControlEvent::Shutdown => {
                    self.process(event);
                    return;
                }
                ControlEvent::Hotkey(_)
                | ControlEvent::HotkeyFailed(_)
                | ControlEvent::Panel(_)
                | ControlEvent::SourceLoaded(_)
                | ControlEvent::Worker(_)
                | ControlEvent::Tick
                | ControlEvent::Meter
                | ControlEvent::Polished(_)
                | ControlEvent::Translation(_) => self.process(event),
            }
        }
    }

    /// Ein Ereignis: protokollieren, Folgezustand bestimmen, Anzeige
    /// aktualisieren.
    fn process(&mut self, event: ControlEvent) {
        // Nur kurz belegt: `handle` liefert den Folgezustand.
        let state = std::mem::replace(&mut self.state, State::Loading);
        let was_recording = state.is_recording();
        // Takte kommen ständig; sie stehen nicht im Protokoll.
        let routine = match event {
            ControlEvent::Tick => Routine::Tick,
            ControlEvent::Meter => Routine::Meter,
            // Fortschritt des Übersetzens kommt je Absatz; nicht protokollieren.
            ControlEvent::Translation(TranslationEvent::Progress { done: _, total: _ }) => {
                Routine::No
            }
            ControlEvent::Hotkey(_)
            | ControlEvent::HotkeyFailed(_)
            | ControlEvent::Panel(_)
            | ControlEvent::SourceLoaded(_)
            | ControlEvent::Worker(_)
            | ControlEvent::Polished(_)
            | ControlEvent::Translation(_)
            | ControlEvent::Shutdown => {
                self.feedback.log(format!(
                    "Zustand {}: {}",
                    state.name(),
                    describe_event(&event)
                ));
                Routine::No
            }
        };
        let state = self.handle(state, event);
        // Ist ein langes Transkript fertig, kommt das nächste wartende dran.
        self.start_waiting(&state);
        self.state = state;
        // Esc gehört nur während eines Diktats Diktator, sonst
        // der App vorne. Aus dem Zustand abgeleitet, damit kein Weg das
        // Zurücksetzen vergisst.
        self.escape_capture
            .store(self.state.captures_escape(), Ordering::Relaxed);
        let recording = self.state.is_recording();
        match routine {
            Routine::No => self.refresh_status(),
            // Ein Takt ändert die Anzeige nur, wenn er eine Aufnahme beendet
            // (Fehler, Modell-Thread weg); sonst bliebe die Oberfläche im
            // Leerlauf zweimal pro Sekunde beschäftigt.
            Routine::Tick if was_recording && !recording => self.refresh_status(),
            Routine::Tick | Routine::Meter => {}
        }
    }

    /// Ein Ereignis im gegebenen Zustand; liefert den Folgezustand.
    fn handle(&mut self, state: State, event: ControlEvent) -> State {
        match event {
            ControlEvent::Hotkey(HotkeyEvent::Tap(Tap { slot, mode })) => {
                let language = match slot {
                    Slot::Primary => Language::German,
                    Slot::Secondary => self.secondary,
                };
                self.on_tap(state, language, mode)
            }
            ControlEvent::Hotkey(HotkeyEvent::Escape) => self.on_escape(state),
            ControlEvent::HotkeyFailed(error) => {
                self.problem(error.note());
                state
            }
            ControlEvent::Panel(command) => {
                self.on_panel(&state, command);
                state
            }
            ControlEvent::SourceLoaded(result) => self.on_loaded(state, result),
            ControlEvent::Worker(event) => self.on_worker(state, event),
            ControlEvent::Tick => self.on_tick(state),
            ControlEvent::Meter => {
                self.on_meter(&state);
                state
            }
            ControlEvent::Polished(done) => self.on_polished(state, done),
            ControlEvent::Translation(event) => {
                self.on_translation(event);
                state
            }
            ControlEvent::Shutdown => self.shut_down(state),
        }
    }

    /// Diktator wird beendet: Eine laufende Aufnahme endet mit dem Zustand,
    /// ein laufendes Transkript wird abgebrochen (auf den Lade-Thread wird
    /// gewartet), wartende fallen weg.
    fn shut_down(&mut self, state: State) -> State {
        // Erkannter Text eines laufenden Diktats geht nicht verloren:
        // Verlauf und Zwischenablage.
        if let Some(dictation) = state.into_dictation()
            && !dictation.text.is_empty()
        {
            self.history.push(&dictation.text);
            if let Err(error) = self.platform.copy(&dictation.text) {
                self.feedback
                    .log(format!("Diktat beim Beenden nicht kopiert: {error}"));
            }
        }
        self.waiting.clear();
        match std::mem::replace(&mut self.transcript, TranscriptJob::Idle) {
            TranscriptJob::Downloading {
                cancel,
                thread,
                origin: _,
                title: _,
                translate: _,
            } => {
                cancel.store(true, Ordering::Relaxed);
                if thread.join().is_err() {
                    self.feedback.log("Lade-Thread ist abgestürzt");
                }
            }
            TranscriptJob::Transcribing {
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
            } => cancel.store(true, Ordering::Relaxed),
            TranscriptJob::Idle => {}
        }
        State::Unavailable {
            reason: Note::new("Diktator wird beendet.", "Diktator is quitting."),
        }
    }

    // ───────────────────────── Modell-Thread ─────────────────────────

    /// Nachricht vom Modell-Thread.
    fn on_worker(&mut self, state: State, event: WorkerEvent) -> State {
        match event {
            WorkerEvent::Ready => {
                self.model = Model::Ready;
                match state {
                    State::Loading => State::Ready,
                    other @ (State::Ready
                    | State::Recording {
                        recording: _,
                        dictation: _,
                    }
                    | State::Processing(_)
                    | State::Polishing(_)
                    | State::Unavailable { reason: _ }) => other,
                }
            }
            WorkerEvent::LoadFailed(error) => self.model_gone(state, describe_engine_error(&error)),
            WorkerEvent::Dictated { part, result } => self.on_part(state, part, result),
            WorkerEvent::TranscriptProgress { id, progress } => {
                self.on_transcript_progress(id, progress);
                state
            }
            WorkerEvent::TranscriptFinished { id, outcome } => {
                self.on_transcript_finished(id, outcome);
                state
            }
        }
    }

    // ───────────────────────── Panel ─────────────────────────

    /// Befehl aus dem Panel.
    fn on_panel(&mut self, state: &State, command: PanelCommand) {
        match command {
            PanelCommand::Youtube {
                language,
                range,
                translate,
            } => self.start_youtube(state, language, &range, translate),
            PanelCommand::File {
                language,
                paths,
                range,
            } => self.start_file(state, language, paths, &range),
            PanelCommand::CancelTranscript => self.cancel_transcript(),
            PanelCommand::CheckLink => self.check_link(),
            PanelCommand::CopyLastTranscript(kind) => self.copy_last_transcript(kind),
            PanelCommand::OpenDictionary => self.open_dictionary(),
            PanelCommand::CopyHistory(serial) => self.copy_history(serial),
            PanelCommand::Visibility(panel) => self.panel = panel,
            PanelCommand::SecondaryLanguage(language) => self.secondary = language,
            PanelCommand::Timestamps(timestamps) => self.timestamps = timestamps,
            PanelCommand::Chapters(chapters) => self.chapters = chapters,
            PanelCommand::Parts(parts) => self.parts = parts,
            PanelCommand::Language(language) => self.language = language,
        }
    }

    /// Legt ein Diktat aus dem Verlauf in die Zwischenablage.
    fn copy_history(&self, serial: HistorySerial) {
        let Some(dictated) = self.history.get(serial) else {
            self.note(Note::new(
                "Dieses Diktat ist nicht mehr im Verlauf.",
                "This dictation is no longer in the history.",
            ));
            return;
        };
        let words = Words::of(dictated);
        match self.platform.copy(dictated) {
            Ok(()) => self.note(Note::new(
                format!("Diktat kopiert ({}).", words.de()),
                format!("Dictation copied ({}).", words.en()),
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

    /// Öffnet das Wörterbuch im Texteditor.
    fn open_dictionary(&self) {
        let opened = paths::dictionary_path()
            .map_err(DictionaryError::from)
            .and_then(|path| dictionary::open_in_editor(&path));
        if let Err(error) = opened {
            self.feedback.play(Sound::Error);
            self.note(error.note());
        }
    }

    // ───────────────────────── Anzeige ─────────────────────────

    /// Der Zustand ohne Diktat: „lädt“, solange das Modell noch nicht
    /// geladen ist, sonst „bereit“.
    const fn idle(&self) -> State {
        match self.model {
            Model::Loading => State::Loading,
            Model::Ready => State::Ready,
        }
    }

    /// Der Modell-Thread ist weg (ein Auftrag ließ sich nicht senden):
    /// nichts geht mehr. Ein laufendes Diktat in `state` endet mit dem, was
    /// schon erkannt ist.
    fn worker_gone(&mut self, state: State) -> State {
        let reason = Note::new(
            "Der Modell-Thread ist beendet. Diktator bitte neu starten.",
            "The model thread has ended. Please restart Diktator.",
        );
        self.model_gone(state, reason)
    }

    /// Das Modell steht nicht mehr bereit (Laden gescheitert, Thread weg):
    /// Antworten auf Teile kommen keine mehr. Ein laufendes Diktat endet
    /// deshalb gleich mit dem, was schon erkannt ist, ohne Überarbeiten (der
    /// Zustand danach ist „nicht verfügbar“, nicht „bereit“).
    fn model_gone(&mut self, state: State, reason: Note) -> State {
        self.feedback.play(Sound::Error);
        self.feedback.notify(reason.clone());
        self.end_transcription();
        match state.into_dictation() {
            Some(mut dictation) => {
                dictation.problems.push(reason.clone());
                self.finish_unpolished(&mut dictation);
            }
            None => self.note(reason.clone()),
        }
        State::Unavailable { reason }
    }

    /// Dauerhaftes Problem: ⚠ in der Menüleiste, Notiz und Mitteilung.
    fn problem(&mut self, message: Note) {
        self.feedback.play(Sound::Error);
        self.feedback.notify(message.clone());
        self.note(message.clone());
        self.problem = Some(message);
    }

    /// Meldung oben im Panel, auch im Protokoll (auf Deutsch, ohne Text des
    /// Nutzers).
    fn note(&self, message: Note) {
        self.feedback.log(message.log_line());
        self.platform.show(UiUpdate::Note(message));
    }

    /// Aktualisiert Symbol und Panel aus dem aktuellen Zustand.
    fn refresh_status(&self) {
        self.platform.show(UiUpdate::Status(self.status()));
        self.platform
            .show(UiUpdate::Transcript(self.transcript_view()));
    }

    /// Was die Menüleiste zeigen soll. Ein Diktat geht vor dem langen
    /// Transkript.
    const fn status(&self) -> Status {
        match self.problem {
            Some(_) => Status::Problem,
            None => match &self.state {
                State::Loading => Status::Loading,
                State::Unavailable { reason: _ } => Status::Problem,
                State::Recording {
                    dictation,
                    recording: _,
                } => Status::Recording {
                    language: dictation.language,
                    mode: dictation.mode,
                },
                State::Processing(_) => Status::Processing,
                State::Polishing(_) => Status::Polishing,
                State::Ready => match &self.transcript {
                    TranscriptJob::Idle => Status::Ready,
                    TranscriptJob::Downloading {
                        origin: _,
                        title: _,
                        cancel: _,
                        thread: _,
                        translate: _,
                    } => Status::SourceLoading,
                    TranscriptJob::Transcribing {
                        percent,
                        id: _,
                        origin: _,
                        cancel: _,
                        title: _,
                        language: _,
                        translate: _,
                    } => Status::Transcribing { percent: *percent },
                    TranscriptJob::Translating {
                        percent,
                        origin: _,
                        cancel: _,
                        title: _,
                        original: _,
                        languages: _,
                    } => Status::Translating { percent: *percent },
                },
            },
        }
    }
}

/// Kurzbeschreibung eines Ereignisses für das Protokoll.
fn describe_event(event: &ControlEvent) -> String {
    match event {
        ControlEvent::Hotkey(HotkeyEvent::Tap(tap)) => format!(
            "fn{}{}",
            match tap.slot {
                Slot::Primary => "",
                Slot::Secondary => " ⇧",
            },
            match tap.mode {
                Mode::Raw => "",
                Mode::Polish => " ⌥",
            }
        ),
        ControlEvent::Hotkey(HotkeyEvent::Escape) => "Esc".to_owned(),
        ControlEvent::HotkeyFailed(error) => format!("Tasten ausgefallen: {error}"),
        ControlEvent::Panel(command) => format!("Panel: {}", describe_panel(command)),
        ControlEvent::SourceLoaded(Ok(downloaded)) => format!(
            "Tonspur geladen: {:.0} s, {}",
            downloaded.pcm.duration().as_secs_f64(),
            downloaded.language
        ),
        ControlEvent::SourceLoaded(Err(error)) => format!("Laden: {error}"),
        ControlEvent::Tick => "Takt".to_owned(),
        ControlEvent::Shutdown => "Beenden".to_owned(),
        ControlEvent::Meter => "Pegel".to_owned(),
        ControlEvent::Polished(done) => describe_polished(done),
        ControlEvent::Translation(event) => describe_translation(event),
        ControlEvent::Worker(event) => describe_worker(event),
    }
}

/// Kurzbeschreibung eines Ergebnisses vom Überarbeiten.
fn describe_polished(done: &PolishDone) -> String {
    match done {
        PolishDone {
            result: Ok(polished),
            dictation: _,
        } => format!(
            "überarbeitet ({} Zeichen){}",
            polished.text.chars().count(),
            polished
                .fallback
                .as_ref()
                .map(|local| format!(" – {local}, stattdessen Apple Intelligence"))
                .unwrap_or_default()
        ),
        PolishDone {
            result: Err(error),
            dictation: _,
        } => format!("nicht überarbeitet: {error}"),
    }
}

/// Kurzbeschreibung einer Nachricht vom Übersetzen.
fn describe_translation(event: &TranslationEvent) -> String {
    match event {
        TranslationEvent::Progress { done, total } => {
            format!("übersetzt {done}/{total}")
        }
        TranslationEvent::Done(TranslationOutcome::Finished(translated)) => format!(
            "Übersetzung fertig ({} Wörter, {} von {} Stücken im Original)",
            text::word_count(&translated.text),
            translated
                .untranslated
                .as_ref()
                .map_or(0, |untranslated| untranslated.count),
            translated.total
        ),
        TranslationEvent::Done(TranslationOutcome::Cancelled) => {
            "Übersetzung abgebrochen".to_owned()
        }
    }
}

/// Kurzbeschreibung einer Nachricht vom Modell-Thread.
fn describe_worker(event: &WorkerEvent) -> String {
    match event {
        WorkerEvent::Ready => "Modell bereit".to_owned(),
        WorkerEvent::LoadFailed(error) => format!("Modell-Fehler: {error}"),
        WorkerEvent::Dictated {
            result: Ok(Some(text)),
            part: _,
        } => {
            format!("Text erkannt ({} Zeichen)", text.chars().count())
        }
        WorkerEvent::Dictated {
            result: Ok(None),
            part: _,
        } => "kein Text erkannt".to_owned(),
        WorkerEvent::Dictated {
            result: Err(unfinished),
            part: _,
        } => format!(
            "Transkriptionsfehler: {} ({} Zeichen davor erkannt)",
            unfinished.error,
            unfinished.text.chars().count()
        ),
        WorkerEvent::TranscriptProgress { progress, id: _ } => {
            format!("Abschnitt {}/{}", progress.done, progress.total)
        }
        WorkerEvent::TranscriptFinished { outcome, id: _ } => match outcome {
            TranscriptOutcome::Finished(text) => {
                format!("Transkript fertig ({} Wörter)", text::word_count(text))
            }
            TranscriptOutcome::Cancelled(text) => {
                format!("Transkript abgebrochen ({} Wörter)", text::word_count(text))
            }
            TranscriptOutcome::Failed { partial, error } => format!(
                "Transkript-Fehler: {error} ({} Wörter davor)",
                text::word_count(partial)
            ),
        },
    }
}

/// Kurzbeschreibung eines Befehls aus dem Panel für das Protokoll: ohne
/// Pfade und Eingaben des Nutzers.
fn describe_panel(command: &PanelCommand) -> String {
    let range = |range: &RangeInput| {
        if range.from.trim().is_empty() && range.to.trim().is_empty() {
            ""
        } else {
            ", mit Bereich"
        }
    };
    match command {
        PanelCommand::Youtube {
            language,
            range: chosen,
            translate,
        } => format!(
            "YouTube ({language:?}{}{})",
            range(chosen),
            translate.map_or_else(String::new, |to| format!(", übersetzen in {to}"))
        ),
        PanelCommand::File {
            language,
            paths,
            range: chosen,
        } => format!(
            "Datei ({language}, {} gezogen{})",
            paths.len(),
            range(chosen)
        ),
        PanelCommand::CancelTranscript => "Transkript abbrechen".to_owned(),
        PanelCommand::CheckLink => "Link prüfen".to_owned(),
        PanelCommand::CopyLastTranscript(kind) => format!("letztes {kind}-Transkript kopieren"),
        PanelCommand::OpenDictionary => "Wörterbuch öffnen".to_owned(),
        PanelCommand::CopyHistory(serial) => format!("Verlauf {serial} kopieren"),
        PanelCommand::Visibility(panel) => format!("{panel:?}"),
        PanelCommand::SecondaryLanguage(language) => format!("zweite Sprache {language}"),
        PanelCommand::Timestamps(timestamps) => format!("{timestamps:?}"),
        PanelCommand::Chapters(chapters) => format!("Kapitel {chapters:?}"),
        PanelCommand::Parts(parts) => format!("{parts:?}"),
        PanelCommand::Language(language) => format!("{language:?}"),
    }
}

/// Meldung, wenn statt des lokalen Modells Apple Intelligence einsprang;
/// `local` ist der Grund.
fn apple_fallback(local: &Note) -> Note {
    Note::new(
        format!("{} – stattdessen Apple Intelligence genutzt.", local.de()),
        format!("{} – used Apple Intelligence instead.", local.en()),
    )
}

/// Verständliche Meldung, wenn das Modell nicht lädt.
fn describe_engine_error(error: &EngineError) -> Note {
    match error {
        EngineError::ModelMissing { path } => Note::new(
            format!(
                "Modelldatei fehlt: {path}. Download-Befehl steht im README unter „Voraussetzungen“."
            ),
            format!(
                "Model file missing: {path}. The download command is in the README under „Voraussetzungen“."
            ),
        ),
        EngineError::Load(_)
        | EngineError::FailedBefore(_)
        | EngineError::Run {
            chunk: _,
            total: _,
            source: _,
        } => Note::new(
            format!("Modell lädt nicht: {error}"),
            format!("Model does not load: {}", error.english()),
        ),
    }
}
