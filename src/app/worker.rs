//! Der Modell-Thread: lädt das Modell einmal und arbeitet Aufträge ab.
//!
//! Diktate haben Vorrang: Während ein Video oder eine Datei transkribiert wird,
//! schaut der Thread nach jedem 30-s-Abschnitt nach wartenden Diktaten und
//! erledigt sie zuerst. Verwirft der Nutzer ein Diktat (Esc), fallen seine
//! noch wartenden Teile weg ([`Job::Discard`]).
//!
//! Je nach Einstellung wird das Modell nach einer Leerlaufzeit entladen und
//! beim nächsten Auftrag neu geladen – nie während einer Aufnahme (vom
//! [`Job::Warmup`] bis zu ihrem letzten Teil oder ihrem Verwerfen). Scheitert
//! das Laden, warten die folgenden Teile nicht jedes Mal Sekunden auf
//! denselben Fehler: Bis zur nächsten Aufnahme oder zum nächsten Transkript
//! melden sie [`EngineError::FailedBefore`].
//!
//! Der Thread gehört einem [`ModelThread`]. Fällt der weg, gibt der Thread
//! das Modell frei und endet; Aufträge, die dann noch warten, bleiben
//! liegen. Liegt beim Prozessende noch Modellspeicher auf der GPU, bricht
//! ggml/Metal mit SIGABRT ab, und der Wächter hielte „Beenden“ für einen
//! Absturz.

use crate::note::{Explain, Note};
use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::audio::chunk::{self, MAX_CHUNK_SAMPLES};
use crate::audio::{self, Pcm16k, wav};
use crate::clock::{Clock, SystemClock};
use crate::dictation_id::DictationId;
use crate::engine::{EngineError, Loader, ModelFile, Progress, Recognized, Recognizer, Unfinished};
use crate::feedback::{Feedback, Logging};
use crate::language::Language;
use crate::paths;
use crate::settings::Unload;
use crate::text::dictionary::{DictionaryFile, Refresh};
use crate::text::{self, Chapter, Piece, Timestamps};

/// Welcher Teil eines Diktats ein Auftrag ist. Ein Diktat wird während der
/// Aufnahme an Sprechpausen in Teile geschnitten, die gleich transkribiert
/// werden; der letzte kommt beim Beenden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PartId {
    /// Fortlaufende Nummer des Diktats (verworfene Diktate erkennt die
    /// Steuerung daran).
    pub dictation: DictationId,
    /// Teil oder letzter Teil.
    pub kind: PartKind,
}

/// Teil oder letzter Teil eines Diktats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PartKind {
    /// Ein Teil während der Aufnahme.
    Partial,
    /// Der Rest beim Beenden; danach ist das Diktat fertig.
    Last,
}

/// Ein Auftrag an das Modell.
#[derive(Debug)]
pub(crate) enum Job {
    /// Eine Aufnahme hat begonnen: das Modell schon laden, falls es
    /// entladen war, damit das Diktat danach nicht darauf warten muss.
    Warmup,
    /// Ab jetzt so entladen (Einstellung geändert).
    SetUnload(Unload),
    /// Einen Teil eines Diktats transkribieren.
    Dictation {
        /// Die Aufnahme.
        pcm: Pcm16k,
        /// Gewählte Sprache.
        language: Language,
        /// Welcher Teil.
        part: PartId,
    },
    /// Ein Diktat wurde verworfen (Esc): seine noch wartenden Teile fallen
    /// weg; ihre Antworten erwartet niemand mehr.
    Discard {
        /// Nummer des Diktats.
        dictation: DictationId,
    },
    /// Eine lange Aufnahme (YouTube-Video, Datei) transkribieren.
    Transcript {
        /// Nummer des Auftrags; Fortschritt und Ergebnis tragen sie.
        id: TranscriptId,
        /// Die Tonspur.
        pcm: Pcm16k,
        /// Sprache des Videos.
        language: Language,
        /// Wo die Tonspur im Original beginnt (Anfang eines Bereichs).
        begin: Duration,
        /// Zeitmarken vor den Absätzen?
        timestamps: Timestamps,
        /// Kapitel, die zu Überschriften werden.
        chapters: Vec<Chapter>,
        /// Wird gesetzt, um abzubrechen.
        cancel: Arc<AtomicBool>,
    },
    /// Diktator wird beendet: das Modell freigeben und den Thread beenden
    /// (schickt [`ModelThread`] beim Wegfallen).
    Shutdown,
}

/// Nummer eines langen Auftrags. Die Steuerung erkennt daran, zu welchem
/// Transkript Fortschritt und Ergebnis gehören, und verwirft fremde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TranscriptId(pub u64);

/// In welcher Sprache ein langes Transkript erkannt und wie es gegliedert
/// wird.
#[derive(Debug)]
struct Layout {
    /// Gesprochene Sprache.
    language: Language,
    /// Wo die Tonspur im Original beginnt; Zeitmarken zählen ab dort.
    begin: Duration,
    /// Zeitmarken vor den Absätzen?
    timestamps: Timestamps,
    /// Kapitel als Überschriften.
    chapters: Vec<Chapter>,
}

/// Ergebnis eines langen Auftrags.
#[derive(Debug)]
pub(crate) enum TranscriptOutcome {
    /// Vollständig transkribiert.
    Finished(String),
    /// Abgebrochen; enthält, was bis dahin erkannt war.
    Cancelled(String),
    /// Das Modell ist an einem Abschnitt gescheitert.
    Failed {
        /// Was bis dahin erkannt war (leer, wenn nichts).
        partial: String,
        /// Woran es scheiterte.
        error: EngineError,
    },
}

/// Was der Modell-Thread meldet.
#[derive(Debug)]
pub(crate) enum WorkerEvent {
    /// Modell geladen, bereit für Aufträge.
    Ready,
    /// Das Modell ließ sich nicht laden; der Thread ist beendet.
    LoadFailed(EngineError),
    /// Ergebnis eines Teils eines Diktats.
    Dictated {
        /// Welcher Teil.
        part: PartId,
        /// Der Text; `None`, wenn keine Sprache darin war. Scheiterte
        /// ein Abschnitt, steht der Text der Abschnitte davor im Fehler.
        result: Result<Option<String>, Unfinished>,
    },
    /// Fortschritt eines langen Auftrags.
    TranscriptProgress {
        /// Welcher Auftrag.
        id: TranscriptId,
        /// Wie weit.
        progress: Progress,
    },
    /// Ein langer Auftrag ist beendet.
    TranscriptFinished {
        /// Welcher Auftrag.
        id: TranscriptId,
        /// Wie er endete.
        outcome: TranscriptOutcome,
    },
}

/// Fehler beim Starten des Modell-Threads.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkerError {
    /// Der Thread ließ sich nicht starten.
    #[error("Modell-Thread: {0}")]
    Spawn(#[from] std::io::Error),
}

impl Explain for WorkerError {
    fn english(&self) -> String {
        match self {
            Self::Spawn(source) => format!("model thread: {source}"),
        }
    }
}

/// Der laufende Modell-Thread. Fällt er weg, gibt der Thread das Modell
/// frei; `drop` wartet darauf (höchstens bis der laufende Auftrag bzw. der
/// laufende 30-s-Abschnitt fertig ist).
#[derive(Debug)]
pub(crate) struct ModelThread {
    /// Eingang des Threads.
    jobs: Sender<Job>,
    /// Der Thread; `None` erst in `drop`.
    thread: Option<JoinHandle<()>>,
    /// Für einen abgestürzten Thread.
    feedback: Feedback,
}

impl ModelThread {
    /// Ein Eingang für Aufträge.
    #[must_use]
    pub(crate) fn jobs(&self) -> Sender<Job> {
        self.jobs.clone()
    }
}

impl Drop for ModelThread {
    fn drop(&mut self) {
        self.stop();
    }
}

impl ModelThread {
    /// Beendet den Thread (er gibt dabei das Modell frei) und wartet auf
    /// ihn; mehrfach aufrufbar.
    pub(crate) fn stop(&mut self) {
        // Scheitert nur, wenn der Thread schon beendet ist.
        match self.jobs.send(Job::Shutdown) {
            Ok(()) | Err(mpsc::SendError(_)) => {}
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            self.feedback.log("Modell-Thread ist abgestürzt");
        }
    }
}

/// Startet den Modell-Thread.
///
/// Ereignisse gehen (umgewandelt in `T`) an `events`; Aufträge nimmt
/// [`ModelThread::jobs`] entgegen. Jedes Ergebnis läuft durch das
/// Wörterbuch unter `dictionary_path`. `unload` sagt, wann das Modell
/// entladen wird. Ladezeit und Dauer jeder Transkription landen im
/// Protokoll von `feedback`.
///
/// # Errors
/// [`WorkerError::Spawn`], wenn kein Thread gestartet werden kann.
pub(crate) fn spawn<T: From<WorkerEvent> + Send + 'static>(
    model_path: PathBuf,
    dictionary_path: PathBuf,
    unload: Unload,
    events: Sender<T>,
    feedback: Feedback,
) -> Result<ModelThread, WorkerError> {
    let (jobs, receiver) = mpsc::channel();
    let thread_feedback = feedback.clone();
    let handle = thread::Builder::new()
        .name("model".to_owned())
        .spawn(move || {
            let loader = ModelFile(model_path);
            let started = SystemClock.now();
            let engine = match loader.load() {
                Ok(engine) => engine,
                Err(error) => {
                    report(&events, WorkerEvent::LoadFailed(error));
                    return;
                }
            };
            thread_feedback.log(format!(
                "Modell geladen in {:.1} s",
                SystemClock.now().duration_since(started).as_secs_f64()
            ));
            let mut worker = Worker::new(
                Setup {
                    loader,
                    clock: SystemClock,
                    unload,
                    dictionary: DictionaryFile::new(dictionary_path),
                    inspection: paths::app_dir()
                        .ok()
                        .map(|dir| dir.join("letzte-leere-aufnahme.wav")),
                },
                Some(engine),
                receiver,
                events,
                thread_feedback,
            );
            worker.serve();
        })?;
    Ok(ModelThread {
        jobs,
        thread: Some(handle),
        feedback,
    })
}

/// Meldet ein Ereignis. `false`, wenn niemand mehr zuhört.
fn report<T: From<WorkerEvent>>(events: &Sender<T>, event: WorkerEvent) -> bool {
    events.send(T::from(event)).is_ok()
}

/// Womit ein [`Worker`] arbeitet.
struct Setup<L, C> {
    /// Lädt das Modell (neu).
    loader: L,
    /// Für Zeiten im Protokoll.
    clock: C,
    /// Wann entladen wird.
    unload: Unload,
    /// Das Wörterbuch.
    dictionary: DictionaryFile,
    /// Wohin eine Aufnahme ohne erkannte Sprache zum Nachhören kommt;
    /// `None` ohne App-Ordner.
    inspection: Option<PathBuf>,
}

/// Ob gerade aufgenommen wird; dann wird das Modell nicht entladen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recording {
    /// Keine Aufnahme.
    Idle,
    /// Seit [`Job::Warmup`], bis zum letzten Teil oder zum Verwerfen.
    Running,
}

/// Wo ein Auftrag erledigt wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum During {
    /// Zwischen den Aufträgen.
    Idle,
    /// Zwischen zwei Abschnitten eines langen Transkripts.
    Transcript,
}

/// Der laufende Modell-Thread.
struct Worker<T, L: Loader, C> {
    /// `None`, solange das Modell entladen ist.
    engine: Option<L::Model>,
    loader: L,
    clock: C,
    unload: Unload,
    dictionary: DictionaryFile,
    inspection: Option<PathBuf>,
    /// Warum das Laden zuletzt scheiterte; bis zum nächsten Anlass wird es
    /// nicht wiederholt.
    load_failure: Option<Note>,
    recording: Recording,
    jobs: Receiver<Job>,
    /// Schon abgeholte, noch nicht erledigte Aufträge (in Reihenfolge).
    backlog: VecDeque<Job>,
    events: Sender<T>,
    feedback: Feedback,
}

impl<T: From<WorkerEvent>, L: Loader, C: Clock> Worker<T, L, C> {
    /// Ein Modell-Thread mit `engine` (schon geladen oder `None`).
    fn new(
        setup: Setup<L, C>,
        engine: Option<L::Model>,
        jobs: Receiver<Job>,
        events: Sender<T>,
        feedback: Feedback,
    ) -> Self {
        Self {
            engine,
            loader: setup.loader,
            clock: setup.clock,
            unload: setup.unload,
            dictionary: setup.dictionary,
            inspection: setup.inspection,
            load_failure: None,
            recording: Recording::Idle,
            jobs,
            backlog: VecDeque::new(),
            events,
            feedback,
        }
    }

    /// Arbeitet Aufträge ab, bis niemand mehr welche schickt, niemand
    /// mehr zuhört oder Diktator beendet wird.
    fn serve(&mut self) {
        self.refresh_dictionary();
        if !self.report(WorkerEvent::Ready) {
            return;
        }
        while let Some(job) = self.next_job() {
            if !self.handle(job, During::Idle) {
                return;
            }
        }
    }

    /// Erledigt einen Auftrag. `false`, wenn niemand mehr zuhört oder
    /// Diktator beendet wird.
    fn handle(&mut self, job: Job, during: During) -> bool {
        match job {
            Job::Warmup => {
                self.recording = Recording::Running;
                // Eine neue Aufnahme ist ein neuer Anlass zu laden.
                self.load_failure = None;
                self.warm_up();
                true
            }
            Job::SetUnload(unload) => {
                self.unload = unload;
                true
            }
            Job::Dictation {
                pcm,
                language,
                part,
            } => {
                match part.kind {
                    PartKind::Partial => {}
                    PartKind::Last => self.recording = Recording::Idle,
                }
                self.dictate(&pcm, language, part)
            }
            // Die wartenden Teile hat `drop_discarded` schon entfernt.
            Job::Discard { dictation: _ } => {
                self.recording = Recording::Idle;
                true
            }
            Job::Transcript {
                id,
                pcm,
                language,
                begin,
                timestamps,
                chapters,
                cancel,
            } => match during {
                During::Idle => {
                    let layout = Layout {
                        language,
                        begin,
                        timestamps,
                        chapters,
                    };
                    self.transcribe_long(id, &pcm, &layout, &cancel)
                }
                // Kommt nach dem laufenden dran; so bekommt jeder lange
                // Auftrag seine Antwort, auf die die Steuerung wartet.
                During::Transcript => {
                    self.backlog.push_back(Job::Transcript {
                        id,
                        pcm,
                        language,
                        begin,
                        timestamps,
                        chapters,
                        cancel,
                    });
                    true
                }
            },
            Job::Shutdown => false,
        }
    }

    /// Holt, was im Kanal wartet, in den Rückstand und entfernt die Teile
    /// verworfener Diktate. `true`, wenn Diktator beendet wird.
    fn collect_jobs(&mut self) -> bool {
        self.backlog.extend(self.jobs.try_iter());
        let discarded: BTreeSet<DictationId> = self
            .backlog
            .iter()
            .filter_map(|job| match job {
                Job::Discard { dictation } => Some(*dictation),
                Job::Warmup
                | Job::SetUnload(_)
                | Job::Dictation {
                    pcm: _,
                    language: _,
                    part: _,
                }
                | Job::Transcript {
                    id: _,
                    pcm: _,
                    language: _,
                    begin: _,
                    timestamps: _,
                    chapters: _,
                    cancel: _,
                }
                | Job::Shutdown => None,
            })
            .collect();
        self.backlog.retain(|job| match job {
            Job::Dictation {
                part,
                pcm: _,
                language: _,
            } => !discarded.contains(&part.dictation),
            Job::Warmup
            | Job::SetUnload(_)
            | Job::Discard { dictation: _ }
            | Job::Transcript {
                id: _,
                pcm: _,
                language: _,
                begin: _,
                timestamps: _,
                chapters: _,
                cancel: _,
            }
            | Job::Shutdown => true,
        });
        self.backlog.iter().any(is_shutdown)
    }

    /// Der nächste Auftrag. `None`, wenn niemand mehr Aufträge schicken
    /// kann oder Diktator beendet wird – dann bleibt liegen, was noch
    /// wartet, statt das Beenden aufzuhalten.
    fn next_job(&mut self) -> Option<Job> {
        if self.backlog.is_empty() {
            let job = self.wait_for_job()?;
            self.backlog.push_back(job);
        }
        if self.collect_jobs() {
            return None;
        }
        self.backlog.pop_front()
    }

    /// Wartet auf einen Auftrag; entlädt das Modell, wenn so lange keiner
    /// kommt, wie eingestellt – außer während einer Aufnahme. `None`, wenn
    /// niemand mehr Aufträge schicken kann.
    fn wait_for_job(&mut self) -> Option<Job> {
        loop {
            let Some(idle) = self.unload_after() else {
                return self.jobs.recv().ok();
            };
            match self.jobs.recv_timeout(idle) {
                Ok(job) => return Some(job),
                Err(RecvTimeoutError::Timeout) => {
                    self.engine = None;
                    self.feedback.log(format!(
                        "Modell nach {} min Leerlauf entladen",
                        idle.as_secs().checked_div(60).unwrap_or(0)
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// Nach wie langem Leerlauf das Modell jetzt entladen würde; `None`,
    /// wenn gar nicht: so eingestellt, schon entladen oder mitten in einer
    /// Aufnahme.
    fn unload_after(&self) -> Option<Duration> {
        match self.unload {
            Unload::Never => None,
            Unload::AfterIdle(idle) => match self.recording {
                Recording::Running => None,
                Recording::Idle => self.engine.as_ref().map(|_| idle),
            },
        }
    }

    /// Das Modell; lädt es neu, wenn es entladen war. Scheiterte das Laden
    /// seit dem letzten Anlass schon einmal, gleich mit diesem Grund.
    fn engine(&mut self) -> Result<&mut L::Model, EngineError> {
        if let Some(engine) = self.engine.take() {
            return Ok(self.engine.insert(engine));
        }
        if let Some(reason) = &self.load_failure {
            return Err(EngineError::FailedBefore(reason.clone()));
        }
        let started = self.clock.now();
        let engine = self.loader.load().inspect_err(|error| {
            self.load_failure = Some(error.note());
        })?;
        self.feedback.log(format!(
            "Modell neu geladen in {:.1} s",
            self.clock.now().duration_since(started).as_secs_f64()
        ));
        Ok(self.engine.insert(engine))
    }

    /// Lädt das Modell, falls es entladen war. Ein Fehler steht nur im
    /// Protokoll; der erste Teil danach meldet ihn.
    fn warm_up(&mut self) {
        if let Err(error) = self.engine() {
            self.feedback
                .log(format!("Modell nicht vorab ladbar: {error}"));
        }
    }

    /// Meldet ein Ereignis. `false`, wenn niemand mehr zuhört.
    fn report(&self, event: WorkerEvent) -> bool {
        report(&self.events, event)
    }

    /// Liest das Wörterbuch neu, falls es sich geändert hat. Ein Fehler
    /// wird gemeldet, mit den Regeln, die dann gelten.
    fn refresh_dictionary(&mut self) {
        match self.dictionary.refresh() {
            Ok(Refresh::Unchanged) => {}
            Ok(Refresh::Reloaded) => self.feedback.log("Wörterbuch gelesen"),
            Ok(Refresh::Created) => self.feedback.log("Wörterbuch angelegt"),
            Err(error) => {
                let note = self.dictionary.failure_note(&error);
                self.feedback.log(note.log_line());
                self.feedback.notify(note);
            }
        }
    }

    /// Transkribiert ein Diktat und meldet das Ergebnis.
    fn dictate(&mut self, pcm: &Pcm16k, language: Language, part: PartId) -> bool {
        self.refresh_dictionary();
        let result = self.transcribe_dictation(pcm, language);
        self.report(WorkerEvent::Dictated { part, result })
    }

    /// Protokolliert, dass das Modell einen Text nicht zu Ende schrieb (nur
    /// die Länge, nicht den Text).
    fn log_cut(&self, recognized: &Recognized, which: &str) {
        if let Some(cut) = recognized.cut {
            self.feedback.log(format!(
                "{which}: Modell hörte vorzeitig auf ({}), {} Zeichen behalten – unvollständig",
                cut.label(),
                recognized.text.chars().count()
            ));
        }
    }

    /// Stille abschneiden und transkribieren. `None`, wenn keine Sprache
    /// darin war oder das Modell nichts erkannt hat.
    fn transcribe_dictation(
        &mut self,
        pcm: &Pcm16k,
        language: Language,
    ) -> Result<Option<String>, Unfinished> {
        let levels = chunk::measure(pcm.samples());
        let Some(speech) = chunk::trim_silence(pcm.samples()) else {
            self.feedback.log(format!(
                "Aufnahme {:.1} s: keine Sprache gefunden ({levels})",
                pcm.duration().as_secs_f64()
            ));
            self.keep_for_inspection(pcm);
            return Ok(None);
        };
        let speech = Pcm16k::from_samples(speech.to_vec());
        let started = self.clock.now();
        let raw = match self.engine()?.transcribe(&speech, language) {
            Ok(raw) => raw,
            Err(Unfinished {
                text: recognised,
                error,
            }) => {
                // Der Text vor dem gescheiterten Abschnitt bleibt.
                let text =
                    text::clean_dictation(&recognised, self.dictionary.dictionary(), language);
                return Err(Unfinished { text, error });
            }
        };
        self.log_cut(&raw, "Diktat");
        if text::implausibly_long(raw.text.trim(), speech.duration()) {
            self.feedback.log(format!(
                "Sprache {:.1} s, aber {} Zeichen erkannt – zu viel, als erfunden verworfen ({levels})",
                speech.duration().as_secs_f64(),
                raw.text.trim().chars().count()
            ));
            self.keep_for_inspection(pcm);
            return Ok(None);
        }
        let text = text::clean_dictation(&raw.text, self.dictionary.dictionary(), language);
        self.feedback.log(format!(
            "Aufnahme {:.1} s, Sprache {:.1} s, transkribiert in {:.2} s: {} Zeichen ({levels})",
            pcm.duration().as_secs_f64(),
            speech.duration().as_secs_f64(),
            self.clock.now().duration_since(started).as_secs_f64(),
            text.chars().count()
        ));
        Ok((!text.is_empty()).then_some(text))
    }

    /// Transkribiert ein Video Abschnitt für Abschnitt. Zwischen den
    /// Abschnitten werden wartende Diktate erledigt und der Abbruch geprüft;
    /// ein Abbruch nach dem letzten Abschnitt zählt auch.
    fn transcribe_long(
        &mut self,
        id: TranscriptId,
        pcm: &Pcm16k,
        layout: &Layout,
        cancel: &AtomicBool,
    ) -> bool {
        self.refresh_dictionary();
        // Ein neues Transkript ist ein neuer Anlass zu laden.
        self.load_failure = None;
        let started = self.clock.now();
        let pieces = chunk::split(pcm.samples(), MAX_CHUNK_SAMPLES);
        let total = pieces.len();
        let mut texts = Vec::with_capacity(total);
        // Die Abschnitte liegen lückenlos hintereinander.
        let mut offset = 0_usize;
        for (index, piece) in pieces.into_iter().enumerate() {
            let start = layout.begin.saturating_add(audio::duration_of(offset));
            offset = offset.saturating_add(piece.len());
            if cancel.load(Ordering::Relaxed) {
                let partial = self.finish_transcript(&texts, layout);
                return self.finished(id, TranscriptOutcome::Cancelled(partial));
            }
            if !self.dictations_first() {
                return false;
            }
            let section = Section {
                number: index.saturating_add(1),
                total,
                start,
            };
            match self.transcribe_section(piece, layout.language, section) {
                Ok(Some(text)) => texts.push(text),
                Ok(None) => {}
                Err(error) => {
                    // Was schon erkannt ist, geht nicht verloren.
                    let partial = self.finish_transcript(&texts, layout);
                    return self.finished(id, TranscriptOutcome::Failed { partial, error });
                }
            }
            let progress = Progress {
                done: index.saturating_add(1),
                total,
            };
            if !self.report(WorkerEvent::TranscriptProgress { id, progress }) {
                return false;
            }
        }
        self.feedback.log(format!(
            "Transkript: {:.0} s Audio in {:.1} s transkribiert ({total} Abschnitte)",
            pcm.duration().as_secs_f64(),
            self.clock.now().duration_since(started).as_secs_f64()
        ));
        let transcript = self.finish_transcript(&texts, layout);
        let outcome = if cancel.load(Ordering::Relaxed) {
            TranscriptOutcome::Cancelled(transcript)
        } else {
            TranscriptOutcome::Finished(transcript)
        };
        self.finished(id, outcome)
    }

    /// Ein Abschnitt eines langen Transkripts; `None` ohne Sprache darin.
    /// Zeit und Länge des Stücks zählen ab dem Beginn der Sprache darin,
    /// nicht ab dem des Abschnitts (Zeitmarken und Kapitel).
    fn transcribe_section(
        &mut self,
        samples: &[f32],
        language: Language,
        section: Section,
    ) -> Result<Option<Piece>, EngineError> {
        let Some(speech) = chunk::speech_range(samples) else {
            return Ok(None);
        };
        let start = section
            .start
            .saturating_add(audio::duration_of(speech.start));
        let length = audio::duration_of(speech.len());
        let Some(speech) = samples.get(speech) else {
            debug_assert!(
                false,
                "invariant: der Bereich der Sprache liegt im Abschnitt"
            );
            return Ok(None);
        };
        let recognized = self
            .engine()?
            .transcribe_piece(speech, language)
            .map_err(|source| EngineError::Run {
                chunk: section.number,
                total: section.total,
                source,
            })?;
        self.log_cut(&recognized, &format!("Abschnitt {}", section.number));
        Ok(Some(Piece {
            text: recognized.text,
            start,
            length,
        }))
    }

    /// Meldet das Ende des langen Auftrags `id`.
    fn finished(&self, id: TranscriptId, outcome: TranscriptOutcome) -> bool {
        self.report(WorkerEvent::TranscriptFinished { id, outcome })
    }

    /// Fügt die Abschnitte zum fertigen Transkript zusammen.
    fn finish_transcript(&self, pieces: &[Piece], layout: &Layout) -> String {
        text::clean_transcript(
            pieces,
            self.dictionary.dictionary(),
            layout.language,
            layout.timestamps,
            &layout.chapters,
        )
    }

    /// Erledigt alle wartenden Diktate; ein weiterer langer Auftrag wartet,
    /// bis der laufende fertig ist. `false`, wenn niemand mehr zuhört oder
    /// Diktator beendet wird.
    fn dictations_first(&mut self) -> bool {
        if self.collect_jobs() {
            return false;
        }
        let waiting = std::mem::take(&mut self.backlog);
        for job in waiting {
            if !self.handle(job, During::Transcript) {
                return false;
            }
        }
        true
    }

    /// Speichert eine Aufnahme ohne erkannte Sprache, damit man nachhören
    /// kann, was das Mikrofon geliefert hat – nur bei eingeschaltetem
    /// Protokoll, denn sie kann die Stimme des Nutzers enthalten. Die Datei
    /// wird jedes Mal überschrieben.
    fn keep_for_inspection(&self, pcm: &Pcm16k) {
        match self.feedback.logging() {
            Logging::On => {}
            Logging::Off => return,
        }
        let Some(path) = &self.inspection else {
            self.feedback
                .log("Aufnahme nicht gespeichert: kein App-Ordner");
            return;
        };
        match wav::write(path, pcm) {
            Ok(()) => self
                .feedback
                .log(format!("Aufnahme gespeichert: {}", paths::tilde(path))),
            Err(error) => self
                .feedback
                .log(format!("Aufnahme nicht gespeichert: {error}")),
        }
    }
}

/// Ein Abschnitt eines langen Transkripts.
#[derive(Debug, Clone, Copy)]
struct Section {
    /// Nummer ab 1.
    number: usize,
    /// Anzahl aller Abschnitte.
    total: usize,
    /// Wo er im Original beginnt.
    start: Duration,
}

/// Ist das der Auftrag, Diktator zu beenden?
const fn is_shutdown(job: &Job) -> bool {
    match job {
        Job::Shutdown => true,
        Job::Warmup
        | Job::SetUnload(_)
        | Job::Dictation {
            pcm: _,
            language: _,
            part: _,
        }
        | Job::Discard { dictation: _ }
        | Job::Transcript {
            id: _,
            pcm: _,
            language: _,
            begin: _,
            timestamps: _,
            chapters: _,
            cancel: _,
        } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use crate::engine::Cut;
    use crate::testing::TempDir;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// Was das nachgebaute Modell bei einem Stück tut.
    #[derive(Debug, Clone)]
    enum Answer {
        /// Erkennt diesen Text.
        Text(&'static str),
        /// Scheitert.
        Fails,
        /// Erkennt den Text und setzt danach das Abbruch-Flag (wie ein
        /// „Abbrechen“ während des letzten Abschnitts).
        TextThenCancel(&'static str, Arc<AtomicBool>),
    }

    /// Ein nachgebautes Modell; antwortet der Reihe nach mit `answers`,
    /// danach mit „Satz.“.
    struct FakeModel {
        answers: Rc<RefCell<VecDeque<Answer>>>,
    }

    impl FakeModel {
        fn next(&self) -> Answer {
            self.answers
                .borrow_mut()
                .pop_front()
                .unwrap_or(Answer::Text("Satz."))
        }
    }

    impl Recognizer for FakeModel {
        fn transcribe(
            &mut self,
            _pcm: &Pcm16k,
            _language: Language,
        ) -> Result<Recognized, Unfinished> {
            match self.next() {
                Answer::Text(text) | Answer::TextThenCancel(text, _) => Ok(Recognized {
                    text: text.to_owned(),
                    cut: None,
                }),
                Answer::Fails => Err(Unfinished::from(EngineError::Run {
                    chunk: 1,
                    total: 1,
                    source: Box::new(transcribe_cpp::Error::Busy("belegt".to_owned())),
                })),
            }
        }

        fn transcribe_piece(
            &mut self,
            _samples: &[f32],
            _language: Language,
        ) -> Result<Recognized, Box<transcribe_cpp::Error>> {
            match self.next() {
                Answer::Text(text) => Ok(Recognized {
                    text: text.to_owned(),
                    cut: text.ends_with('…').then_some(Cut::Length),
                }),
                Answer::TextThenCancel(text, cancel) => {
                    cancel.store(true, Ordering::Relaxed);
                    Ok(Recognized {
                        text: text.to_owned(),
                        cut: None,
                    })
                }
                Answer::Fails => Err(Box::new(transcribe_cpp::Error::Busy("belegt".to_owned()))),
            }
        }
    }

    /// Lädt ein [`FakeModel`]; zählt die Ladevorgänge und scheitert auf
    /// Wunsch.
    struct FakeLoader {
        loads: Rc<Cell<usize>>,
        fails: Rc<Cell<bool>>,
        answers: Rc<RefCell<VecDeque<Answer>>>,
    }

    impl Loader for FakeLoader {
        type Model = FakeModel;

        fn load(&self) -> Result<FakeModel, EngineError> {
            self.loads.set(self.loads.get() + 1);
            if self.fails.get() {
                return Err(EngineError::ModelMissing {
                    path: "~/x.gguf".to_owned(),
                });
            }
            Ok(FakeModel {
                answers: Rc::clone(&self.answers),
            })
        }
    }

    /// Ein Modell-Thread mit nachgebautem Modell, auf dem Thread des Tests.
    struct Harness {
        loads: Rc<Cell<usize>>,
        fails: Rc<Cell<bool>>,
        answers: Rc<RefCell<VecDeque<Answer>>>,
        jobs: Option<Sender<Job>>,
        events: Receiver<WorkerEvent>,
        worker: Worker<WorkerEvent, FakeLoader, FakeClock>,
        _dir: TempDir,
    }

    /// Ob das Modell beim Start der Prüfumgebung schon geladen ist.
    #[derive(Debug, Clone, Copy)]
    enum Model {
        Loaded,
        NotLoaded,
    }

    impl Harness {
        fn new(name: &str, unload: Unload, model: Model) -> Self {
            let dir = TempDir::new(&format!("worker-{name}"));
            let loads = Rc::new(Cell::new(0));
            let fails = Rc::new(Cell::new(false));
            let answers = Rc::new(RefCell::new(VecDeque::new()));
            let fake_loader = FakeLoader {
                loads: Rc::clone(&loads),
                fails: Rc::clone(&fails),
                answers: Rc::clone(&answers),
            };
            let engine = match model {
                Model::Loaded => Some(FakeModel {
                    answers: Rc::clone(&answers),
                }),
                Model::NotLoaded => None,
            };
            let (jobs, receiver) = mpsc::channel();
            let (sender, events) = mpsc::channel();
            let worker = Worker::new(
                Setup {
                    loader: fake_loader,
                    clock: FakeClock::new(),
                    unload,
                    dictionary: DictionaryFile::new(dir.path().join("dictionary.toml")),
                    inspection: Some(dir.path().join("leer.wav")),
                },
                engine,
                receiver,
                sender,
                Feedback::silent(),
            );
            Self {
                loads,
                fails,
                answers,
                jobs: Some(jobs),
                events,
                worker,
                _dir: dir,
            }
        }

        fn send(&self, job: Job) {
            self.jobs.as_ref().unwrap().send(job).unwrap();
        }

        fn answer(&self, answers: impl IntoIterator<Item = Answer>) {
            self.answers.borrow_mut().extend(answers);
        }

        /// Arbeitet alles Geschickte ab und liefert die Ereignisse.
        fn run(mut self) -> (Vec<WorkerEvent>, usize) {
            drop(self.jobs.take());
            self.worker.serve();
            (self.events.try_iter().collect(), self.loads.get())
        }
    }

    /// Eine Sekunde, die als Sprache gilt.
    fn speech(seconds: usize) -> Vec<f32> {
        (0..seconds * 16_000)
            .map(|index| if (index / 40) % 2 == 0 { 0.1 } else { -0.1 })
            .collect()
    }

    fn part(dictation: u64, kind: PartKind) -> Job {
        Job::Dictation {
            pcm: Pcm16k::from_samples(speech(1)),
            language: Language::German,
            part: PartId {
                dictation: DictationId::new(dictation),
                kind,
            },
        }
    }

    fn transcript(pcm: Vec<f32>, cancel: &Arc<AtomicBool>) -> Job {
        Job::Transcript {
            id: TranscriptId(7),
            pcm: Pcm16k::from_samples(pcm),
            language: Language::German,
            begin: Duration::ZERO,
            timestamps: Timestamps::On,
            chapters: Vec::new(),
            cancel: Arc::clone(cancel),
        }
    }

    /// Die Diktatteile, die ein Ergebnis bekamen, in Reihenfolge, mit dem
    /// Fehler, falls einer.
    fn dictated(events: &[WorkerEvent]) -> Vec<(PartId, Option<String>)> {
        events
            .iter()
            .filter_map(|event| match event {
                WorkerEvent::Dictated { part, result } => Some((
                    *part,
                    result
                        .as_ref()
                        .err()
                        .map(|unfinished| unfinished.error.to_string()),
                )),
                WorkerEvent::Ready
                | WorkerEvent::LoadFailed(_)
                | WorkerEvent::TranscriptProgress { id: _, progress: _ }
                | WorkerEvent::TranscriptFinished { id: _, outcome: _ } => None,
            })
            .collect()
    }

    /// Das Ende des langen Auftrags, falls gemeldet.
    fn outcome(events: Vec<WorkerEvent>) -> Option<(TranscriptId, TranscriptOutcome)> {
        events.into_iter().find_map(|event| match event {
            WorkerEvent::TranscriptFinished { id, outcome } => Some((id, outcome)),
            WorkerEvent::Ready
            | WorkerEvent::LoadFailed(_)
            | WorkerEvent::Dictated { part: _, result: _ }
            | WorkerEvent::TranscriptProgress { id: _, progress: _ } => None,
        })
    }

    #[test]
    fn dictations_go_before_the_sections_of_a_transcript() {
        let harness = Harness::new("priority", Unload::Never, Model::Loaded);
        let cancel = Arc::new(AtomicBool::new(false));
        harness.send(transcript(speech(70), &cancel));
        harness.send(part(1, PartKind::Last));
        let (events, _) = harness.run();
        let dictation = events
            .iter()
            .position(|event| matches!(event, WorkerEvent::Dictated { part: _, result: _ }))
            .expect("Diktat erledigt");
        let finished = events
            .iter()
            .position(|event| {
                matches!(event, WorkerEvent::TranscriptFinished { id: _, outcome: _ })
            })
            .expect("Transkript fertig");
        assert!(
            dictation < finished,
            "das Diktat wartet nicht auf das Transkript"
        );
    }

    #[test]
    fn shutdown_in_the_backlog_ends_without_finishing_the_transcript() {
        let harness = Harness::new("shutdown", Unload::Never, Model::Loaded);
        let cancel = Arc::new(AtomicBool::new(false));
        harness.send(transcript(speech(70), &cancel));
        harness.send(Job::Shutdown);
        let (events, _) = harness.run();
        assert!(
            outcome(events).is_none(),
            "beim Beenden bleibt liegen, was wartet"
        );
    }

    #[test]
    fn a_failing_section_names_its_number_and_keeps_the_text_before() {
        let harness = Harness::new("section", Unload::Never, Model::Loaded);
        harness.answer([Answer::Text("Erster Satz."), Answer::Fails]);
        let cancel = Arc::new(AtomicBool::new(false));
        harness.send(transcript(speech(70), &cancel));
        let (events, _) = harness.run();
        let (id, outcome) = outcome(events).expect("Ende gemeldet");
        assert_eq!(id, TranscriptId(7));
        let TranscriptOutcome::Failed { partial, error } = outcome else {
            panic!("gescheitert erwartet: {outcome:?}");
        };
        assert!(partial.contains("Erster Satz."), "{partial}");
        assert!(
            matches!(
                error,
                EngineError::Run {
                    chunk: 2,
                    total: 3,
                    source: _
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn a_cancel_during_the_last_section_counts() {
        let harness = Harness::new("cancel-last", Unload::Never, Model::Loaded);
        let cancel = Arc::new(AtomicBool::new(false));
        harness.answer([Answer::TextThenCancel("Ganz.", Arc::clone(&cancel))]);
        harness.send(transcript(speech(10), &cancel));
        let (events, _) = harness.run();
        let (_, outcome) = outcome(events).expect("Ende gemeldet");
        assert!(
            matches!(&outcome, TranscriptOutcome::Cancelled(text) if text.contains("Ganz.")),
            "{outcome:?}"
        );
    }

    #[test]
    fn timestamps_point_at_the_speech_not_at_the_section() {
        let harness = Harness::new("timestamps", Unload::Never, Model::Loaded);
        harness.answer([Answer::Text("Erster Satz.")]);
        let mut samples = vec![0.0_f32; 20 * 16_000];
        samples.extend(speech(10));
        let speech_start = chunk::speech_range(&samples).unwrap().start;
        let cancel = Arc::new(AtomicBool::new(false));
        harness.send(transcript(samples, &cancel));
        let (events, _) = harness.run();
        let (_, TranscriptOutcome::Finished(text)) = outcome(events).expect("Ende") else {
            panic!("fertig erwartet");
        };
        let expected = format!("[0:{:02}]", audio::duration_of(speech_start).as_secs());
        assert!(text.starts_with(&expected), "Sprache ab {expected}: {text}");
    }

    #[test]
    fn a_failed_load_is_not_repeated_for_every_part() {
        let harness = Harness::new("load-fails", Unload::Never, Model::NotLoaded);
        harness.fails.set(true);
        harness.send(part(1, PartKind::Partial));
        harness.send(part(1, PartKind::Partial));
        harness.send(part(1, PartKind::Last));
        let (events, loads) = harness.run();
        assert_eq!(loads, 1, "einmal geladen, nicht je Teil");
        let results = dictated(&events);
        assert_eq!(results.len(), 3);
        assert!(
            results[2]
                .1
                .as_ref()
                .is_some_and(|error| error.contains("vorhin")),
            "{results:?}"
        );
    }

    #[test]
    fn a_new_recording_tries_the_load_again() {
        let harness = Harness::new("load-retry", Unload::Never, Model::NotLoaded);
        harness.fails.set(true);
        harness.send(part(1, PartKind::Last));
        harness.send(Job::Warmup);
        harness.send(part(2, PartKind::Last));
        let (_, loads) = harness.run();
        assert_eq!(loads, 2, "die neue Aufnahme versucht es noch einmal");
    }

    #[test]
    fn a_discarded_dictation_loses_its_waiting_parts() {
        let harness = Harness::new("discard", Unload::Never, Model::Loaded);
        harness.send(part(1, PartKind::Partial));
        harness.send(part(1, PartKind::Partial));
        harness.send(Job::Discard {
            dictation: DictationId::new(1),
        });
        harness.send(part(2, PartKind::Last));
        let (events, _) = harness.run();
        let parts: Vec<DictationId> = dictated(&events)
            .into_iter()
            .map(|(part, _)| part.dictation)
            .collect();
        assert_eq!(
            parts,
            [DictationId::new(2)],
            "nur das neue Diktat wird transkribiert"
        );
    }

    #[test]
    fn the_model_is_unloaded_when_idle_and_loaded_again() {
        let mut harness = Harness::new(
            "unload",
            Unload::AfterIdle(Duration::from_millis(1)),
            Model::Loaded,
        );
        let jobs = harness.jobs.take().unwrap();
        let sender = thread::Builder::new()
            .spawn(move || {
                SystemClock.sleep(Duration::from_millis(100));
                jobs.send(part(1, PartKind::Last)).unwrap();
            })
            .unwrap();
        let (events, loads) = harness.run();
        sender.join().unwrap();
        assert_eq!(loads, 1, "nach dem Leerlauf neu geladen");
        assert_eq!(dictated(&events).len(), 1);
    }

    #[test]
    fn the_model_stays_loaded_during_a_recording() {
        let mut harness = Harness::new(
            "recording",
            Unload::AfterIdle(Duration::from_millis(1)),
            Model::Loaded,
        );
        let jobs = harness.jobs.take().unwrap();
        let sender = thread::Builder::new()
            .spawn(move || {
                jobs.send(Job::Warmup).unwrap();
                SystemClock.sleep(Duration::from_millis(100));
                jobs.send(part(1, PartKind::Last)).unwrap();
            })
            .unwrap();
        let (events, loads) = harness.run();
        sender.join().unwrap();
        assert_eq!(loads, 0, "während der Aufnahme nicht entladen");
        assert_eq!(dictated(&events).len(), 1);
    }

    #[test]
    fn no_inspection_file_without_the_log() {
        let harness = Harness::new("inspection", Unload::Never, Model::Loaded);
        let path = harness.worker.inspection.clone().unwrap();
        harness.send(Job::Dictation {
            pcm: Pcm16k::from_samples(vec![0.0; 16_000]),
            language: Language::German,
            part: PartId {
                dictation: DictationId::new(1),
                kind: PartKind::Last,
            },
        });
        let (events, _) = harness.run();
        assert_eq!(dictated(&events).len(), 1);
        assert!(
            !path.exists(),
            "Protokoll aus: keine Aufnahme auf der Platte"
        );
    }
}
