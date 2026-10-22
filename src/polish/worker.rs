//! Der Überarbeiten-Thread: nimmt fertige Diktate entgegen, lässt sie vom
//! Sprachmodell korrigieren und übernimmt über das Sicherheitsnetz nur
//! Korrekturen. Außerdem übersetzt er lange Transkripte; zwischen zwei
//! Absätzen kommen wartende Diktate dran.
//!
//! Er besitzt den llama-server ([`Llama`]) und beendet ihn nach der
//! eingestellten Pause ohne Auftrag – so wie der Modell-Thread die
//! Spracherkennung entlädt.

use std::collections::{BTreeSet, VecDeque};
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::guard::{self, Spelling};
use super::llama::{Llama, LlamaError, ServerPid};
use super::spelling::{self, SpellingError};
use super::translate::{self, Unit};
use super::{Backend, PolishError, Polished, TIMEOUT, apple, correct_instructions, local};
use crate::clock::{Clock, SystemClock};
use crate::dictation_id::DictationId;
use crate::feedback::Feedback;
use crate::language::Language;
use crate::note::{Explain, Note};
use crate::process::Limit;
use crate::settings::Unload;

/// Ein Auftrag an den Überarbeiten-Thread.
#[derive(Debug)]
pub(crate) enum PolishRequest {
    /// Eine Aufnahme mit fn + ⌥ hat begonnen: ein lokales Modell schon
    /// laden (dauert einige Sekunden), während noch gesprochen wird.
    Warmup,
    /// Ein fertiges Diktat überarbeiten.
    Polish {
        /// Nummer des Diktats (kommt mit dem Ergebnis zurück).
        dictation: DictationId,
        /// Der Text.
        text: String,
        /// Seine Sprache.
        language: Language,
    },
    /// Ab jetzt hiermit überarbeiten. Beim Wechsel zu Apple Intelligence
    /// wird ein laufendes lokales Modell beendet.
    UseBackend(Backend),
    /// Ein langes Transkript übersetzen.
    Translate(TranslationRequest),
    /// Diktator wird beendet: Der Thread endet nach dem laufenden Auftrag
    /// (eine laufende Übersetzung bricht ab) und beendet seinen
    /// llama-server; wartende Aufträge fallen weg.
    Shutdown,
}

/// Was übersetzt werden soll.
#[derive(Debug)]
pub(crate) struct TranslationRequest {
    /// Titel des Videos (wird mit übersetzt).
    pub title: String,
    /// Das Transkript, Absätze durch Leerzeilen getrennt.
    pub text: String,
    /// Seine Sprache.
    pub from: Language,
    /// Zielsprache.
    pub to: Language,
    /// Wird gesetzt, um abzubrechen.
    pub cancel: Arc<AtomicBool>,
}

/// Nachricht vom Übersetzen.
#[derive(Debug)]
pub(crate) enum TranslationEvent {
    /// So viele Stücke (Titel, Überschriften, Absätze) sind fertig.
    Progress {
        /// Fertige Stücke.
        done: usize,
        /// Alle Stücke.
        total: usize,
    },
    /// Die Übersetzung ist zu Ende.
    Done(TranslationOutcome),
}

/// Wie eine Übersetzung endete.
#[derive(Debug)]
pub(crate) enum TranslationOutcome {
    /// Alle Stücke sind durch (manche vielleicht im Original geblieben).
    Finished(Box<Translated>),
    /// Abgebrochen.
    Cancelled,
}

/// Ein übersetztes Transkript.
#[derive(Debug)]
pub(crate) struct Translated {
    /// Der übersetzte Titel.
    pub title: String,
    /// Das übersetzte Transkript, Zeitmarken und Überschriften an ihrem Platz.
    pub text: String,
    /// So viele Stücke gab es (mit dem Titel).
    pub total: usize,
    /// Stücke, an denen das Modell scheiterte; sie stehen im Original da.
    pub untranslated: Option<Untranslated>,
    /// Warum das lokale Modell nicht ging, falls auf Apple Intelligence
    /// ausgewichen wurde.
    pub fallback: Option<Note>,
}

/// Stücke, die im Original blieben.
#[derive(Debug)]
pub(crate) struct Untranslated {
    /// Wie viele.
    pub count: usize,
    /// Warum das erste scheiterte.
    pub first_error: PolishError,
}

impl Untranslated {
    /// Zählt ein weiteres Stück, an dem das Modell mit `error` scheiterte,
    /// zu den `earlier`; der erste Fehler bleibt der genannte.
    fn add(earlier: Option<Self>, error: PolishError) -> Self {
        match earlier {
            Some(earlier) => Self {
                count: earlier.count.saturating_add(1),
                first_error: earlier.first_error,
            },
            None => Self {
                count: 1,
                first_error: error,
            },
        }
    }
}

/// Ergebnis eines Auftrags.
#[derive(Debug)]
pub(crate) struct PolishDone {
    /// Nummer des Diktats.
    pub dictation: DictationId,
    /// Der überarbeitete Text oder warum nicht.
    pub result: Result<Polished, PolishError>,
}

/// Der gestartete Überarbeiten-Thread. [`PolishThread::stop`] (oder das
/// Wegfallen) beendet ihn und wartet auf ihn, dann erst den llama-server:
/// So startet kein laufender Auftrag einen Server neu, der schon beendet
/// ist.
#[derive(Debug)]
pub(crate) struct PolishThread {
    /// Eingang für Aufträge.
    requests: Sender<PolishRequest>,
    /// Zum Beenden des llama-servers, falls der Thread es nicht mehr tut.
    server: ServerPid,
    /// Der Thread; `None`, sobald beendet.
    thread: Option<JoinHandle<()>>,
}

/// Der Überarbeiten-Thread ist abgestürzt.
#[derive(Debug, thiserror::Error)]
#[error("Überarbeiten-Thread ist abgestürzt")]
pub(crate) struct PolishCrashed;

impl PolishThread {
    /// Ein Eingang für Aufträge.
    #[must_use]
    pub(crate) fn requests(&self) -> Sender<PolishRequest> {
        self.requests.clone()
    }

    /// Beendet den Thread, wartet auf ihn und danach auf das Ende seines
    /// llama-servers. Mehrfach aufrufbar.
    ///
    /// # Errors
    /// [`PolishCrashed`], wenn der Thread abgestürzt ist.
    pub(crate) fn stop(&mut self) -> Result<(), PolishCrashed> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        // Scheitert nur, wenn der Thread schon beendet ist.
        match self.requests.send(PolishRequest::Shutdown) {
            Ok(()) | Err(mpsc::SendError(_)) => {}
        }
        // Der Inhalt einer Panik ist hier nichts wert; der Absturz zählt.
        let joined = thread.join().map_err(|_panic| PolishCrashed);
        // Im Normalfall hat der Thread ihn schon beendet; sonst hier. Danach
        // startet `Llama` keinen neuen mehr.
        self.server.kill();
        joined
    }
}

impl Drop for PolishThread {
    fn drop(&mut self) {
        // Ein Absturz wurde schon gemeldet oder zählt hier nicht mehr: der
        // Thread ist so oder so weg.
        match self.stop() {
            Ok(()) | Err(PolishCrashed) => {}
        }
    }
}

/// Startet den Überarbeiten-Thread (zunächst mit Apple Intelligence);
/// `server` erfährt die Nummer jedes llama-servers, den er startet.
/// Ergebnisse gehen (umgewandelt in `T`) an `events`; Ladezeiten,
/// Rechtschreibprüfung und Sicherheitsnetz landen im Protokoll.
///
/// # Errors
/// Wenn kein Thread gestartet werden kann.
pub(crate) fn spawn<T: From<PolishDone> + From<TranslationEvent> + Send + 'static>(
    events: Sender<T>,
    feedback: Feedback,
    server: ServerPid,
) -> io::Result<PolishThread> {
    let (requests, receiver) = mpsc::channel();
    let llama = Llama::new(server.share());
    let thread = thread::Builder::new()
        .name("polish".to_owned())
        .spawn(move || {
            let polisher = Polisher {
                backend: Backend::Apple,
                models: Machine { llama },
                spelling: MacSpelling,
                clock: SystemClock,
                pending: VecDeque::new(),
                events,
                feedback,
            };
            polisher.serve(&receiver);
        })?;
    Ok(PolishThread {
        requests,
        server,
        thread: Some(thread),
    })
}

/// Die Sprachmodelle, die der Thread fragt; Tests geben nachgebaute, die
/// auf Wunsch scheitern.
pub(super) trait Models {
    /// Apple Intelligence, höchstens bis `limit`.
    ///
    /// # Errors
    /// [`PolishError`], wenn es nicht verfügbar ist, ablehnt, nicht
    /// rechtzeitig antwortet oder abgebrochen wird.
    fn apple(
        &mut self,
        text: &str,
        instructions: &str,
        limit: Limit<'_>,
    ) -> Result<String, PolishError>;

    /// Das lokale Modell `model` (startet seinen Server, falls nötig),
    /// höchstens `timeout` lang.
    ///
    /// # Errors
    /// [`PolishError`], wenn der Server nicht startet oder die Anfrage
    /// scheitert.
    fn local(
        &mut self,
        question: &Question<'_>,
        model: &Path,
        timeout: Duration,
    ) -> Result<String, PolishError>;

    /// Startet den Server für `model` schon einmal.
    ///
    /// # Errors
    /// [`LlamaError`], wenn er nicht startet.
    fn warm_up(&mut self, model: &Path) -> Result<Warmed, LlamaError>;

    /// Wann der laufende Server zuletzt antwortete; `None` ohne Server.
    fn last_used(&self) -> Option<Instant>;

    /// Beendet den lokalen Server, falls einer läuft.
    fn stop_local(&mut self);

    /// Der nächste Start des Servers wird wieder versucht.
    fn retry_local(&mut self);
}

/// Eine Frage an ein lokales Modell.
pub(super) struct Question<'a> {
    /// Der Text.
    pub(super) text: &'a str,
    /// Was mit ihm geschehen soll.
    pub(super) instructions: &'a str,
    /// Seine Sprache (für die Längengrenze der Antwort).
    pub(super) language: Language,
}

/// Wie das Vorab-Starten ausging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Warmed {
    /// Der Server lief schon.
    AlreadyRunning,
    /// Eben gestartet.
    Started,
}

/// Die echten Modelle: Apple Intelligence über `diktator-llm`, das lokale
/// über llama-server.
struct Machine {
    llama: Llama,
}

impl Models for Machine {
    fn apple(
        &mut self,
        text: &str,
        instructions: &str,
        limit: Limit<'_>,
    ) -> Result<String, PolishError> {
        apple::ask(text, instructions, limit)
    }

    fn local(
        &mut self,
        question: &Question<'_>,
        model: &Path,
        timeout: Duration,
    ) -> Result<String, PolishError> {
        let url = self.llama.url_for(model)?;
        let answer = local::ask(
            question.text,
            question.instructions,
            &url,
            question.language,
            timeout,
        );
        self.llama.finished();
        answer
    }

    fn warm_up(&mut self, model: &Path) -> Result<Warmed, LlamaError> {
        let already_running = self.llama.last_used().is_some();
        self.llama.url_for(model)?;
        self.llama.finished();
        Ok(if already_running {
            Warmed::AlreadyRunning
        } else {
            Warmed::Started
        })
    }

    fn last_used(&self) -> Option<Instant> {
        self.llama.last_used()
    }

    fn stop_local(&mut self) {
        self.llama.stop();
    }

    fn retry_local(&mut self) {
        self.llama.retry();
    }
}

/// Die Rechtschreibprüfung; läuft auf eigenem Thread neben dem Modell.
pub(super) trait Speller: Sync {
    /// Die Wörter aus `text`, die richtig geschrieben sind.
    ///
    /// # Errors
    /// [`SpellingError`], wenn die Prüfung nicht geht.
    fn correct_words(
        &self,
        text: &str,
        language: Language,
    ) -> Result<BTreeSet<String>, SpellingError>;
}

/// Die Rechtschreibprüfung von macOS.
struct MacSpelling;

impl Speller for MacSpelling {
    fn correct_words(
        &self,
        text: &str,
        language: Language,
    ) -> Result<BTreeSet<String>, SpellingError> {
        spelling::correct_words(text, language)
    }
}

/// Bis wann eine Frage dauern darf, Ausweichen eingeschlossen, und ob sie
/// abgebrochen werden kann (beim Übersetzen).
#[derive(Debug, Clone, Copy)]
struct Budget<'a> {
    deadline: Instant,
    cancel: Option<&'a AtomicBool>,
}

impl<'a> Budget<'a> {
    /// Die Grenze für einen Lauf, der jetzt (`now`) beginnt: die Zeit bis
    /// zur Frist, dazu das Abbruch-Flag, falls es eins gibt.
    fn limit(self, now: Instant) -> Limit<'a> {
        let remaining = self.deadline.saturating_duration_since(now);
        match self.cancel {
            Some(cancel) => Limit::AfterOrCancel(remaining, cancel),
            None => Limit::After(remaining),
        }
    }
}

/// Der laufende Überarbeiten-Thread.
struct Polisher<T, M, S, C> {
    backend: Backend,
    models: M,
    spelling: S,
    clock: C,
    /// Aufträge, die während einer Übersetzung kamen und danach drankommen.
    pending: VecDeque<PolishRequest>,
    events: Sender<T>,
    feedback: Feedback,
}

impl<T: From<PolishDone> + From<TranslationEvent>, M: Models, S: Speller, C: Clock>
    Polisher<T, M, S, C>
{
    /// Arbeitet Aufträge ab, bis niemand mehr welche schickt oder niemand
    /// mehr zuhört. Danach fällt der llama-server mit den Modellen weg.
    fn serve(mut self, requests: &Receiver<PolishRequest>) {
        while let Some(request) = self.next_request(requests) {
            if !self.handle(request, requests) {
                return;
            }
        }
    }

    /// Erledigt einen Auftrag. `false`, wenn niemand mehr zuhört.
    fn handle(&mut self, request: PolishRequest, requests: &Receiver<PolishRequest>) -> bool {
        match request {
            PolishRequest::Warmup => {
                self.warm_up();
                true
            }
            PolishRequest::UseBackend(backend) => {
                self.use_backend(backend);
                true
            }
            PolishRequest::Polish {
                dictation,
                text,
                language,
            } => {
                let result = self.polish(&text, language);
                self.send(PolishDone { dictation, result })
            }
            PolishRequest::Translate(request) => self.translate(&request, requests),
            PolishRequest::Shutdown => false,
        }
    }

    /// Schickt ein Ergebnis an die Steuerung. `false`, wenn niemand mehr
    /// zuhört.
    fn send(&self, event: impl Into<T>) -> bool {
        self.events.send(event.into()).is_ok()
    }

    /// Wartet auf den nächsten Auftrag; beendet den llama-server, wenn so
    /// lange keiner kommt, wie eingestellt. `None`, wenn niemand mehr
    /// Aufträge schicken kann.
    fn next_request(&mut self, requests: &Receiver<PolishRequest>) -> Option<PolishRequest> {
        if let Some(request) = self.pending.pop_front() {
            return Some(request);
        }
        loop {
            let Some(deadline) = self.unload_deadline() else {
                return requests.recv().ok();
            };
            match requests.recv_timeout(deadline.saturating_duration_since(self.clock.now())) {
                Ok(request) => return Some(request),
                Err(RecvTimeoutError::Timeout) => {
                    self.models.stop_local();
                    self.feedback
                        .log("Lokales Sprachmodell nach Leerlauf beendet");
                }
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// Wann der llama-server beendet wird; `None`, solange keiner läuft
    /// oder er bleiben soll.
    fn unload_deadline(&self) -> Option<Instant> {
        match &self.backend {
            Backend::Apple
            | Backend::Local {
                unload: Unload::Never,
                model: _,
            } => None,
            Backend::Local {
                unload: Unload::AfterIdle(idle),
                model: _,
            } => self.models.last_used()?.checked_add(*idle),
        }
    }

    /// Startet das lokale Modell, falls es gewählt ist und nicht läuft –
    /// bei jeder Aufnahme ein neuer Versuch, auch wenn der letzte scheiterte.
    /// Ein Fehler steht im Protokoll; das Überarbeiten danach nennt ihn
    /// ([`LlamaError::FailedBefore`]) und weicht auf Apple Intelligence aus.
    fn warm_up(&mut self) {
        let model = match &self.backend {
            Backend::Apple => return,
            Backend::Local { model, unload: _ } => model.clone(),
        };
        self.models.retry_local();
        let started = self.clock.now();
        match self.models.warm_up(&model) {
            Ok(Warmed::AlreadyRunning) => {}
            Ok(Warmed::Started) => self.feedback.log(format!(
                "Lokales Sprachmodell vorab geladen in {:.1} s",
                self.clock
                    .now()
                    .saturating_duration_since(started)
                    .as_secs_f64()
            )),
            Err(error) => self
                .feedback
                .log(format!("Lokales Sprachmodell nicht vorab ladbar: {error}")),
        }
    }

    /// Wechselt das Modell fürs Überarbeiten (eine laufende Übersetzung
    /// bleibt bei ihrem).
    fn use_backend(&mut self, backend: Backend) {
        match backend {
            // Ein laufendes lokales Modell gibt den Speicher frei.
            Backend::Apple => self.models.stop_local(),
            // Ein anderes Modell startet beim nächsten Auftrag.
            Backend::Local {
                model: _,
                unload: _,
            } => {}
        }
        self.backend = backend;
        self.models.retry_local();
    }

    /// Lässt `text` überarbeiten und übernimmt davon nur Korrekturen.
    fn polish(&mut self, text: &str, language: Language) -> Result<Polished, PolishError> {
        let instructions = correct_instructions(language.english_name());
        let budget = Budget {
            deadline: self.deadline(),
            cancel: None,
        };
        let backend = self.backend.clone();
        let (models, speller, clock) = (&mut self.models, &self.spelling, &self.clock);
        // Die Rechtschreibprüfung läuft, während das Modell arbeitet.
        let (answer, correct) = thread::scope(|scope| {
            let checking = thread::Builder::new()
                .name("spelling".to_owned())
                .spawn_scoped(scope, || speller.correct_words(text, language));
            let question = Question {
                text,
                instructions: &instructions,
                language,
            };
            let answer = ask(models, clock, &backend, &question, budget);
            let correct = match checking {
                Ok(checking) => checking.join().unwrap_or(Err(SpellingError::Crashed)),
                Err(error) => {
                    self.feedback
                        .log(format!("Rechtschreibprüfung ohne eigenen Thread: {error}"));
                    speller.correct_words(text, language)
                }
            };
            (answer, correct)
        });
        let answer = answer?;
        let spelling = match &correct {
            Ok(correct) => Spelling::Checked(correct),
            Err(error) => {
                self.feedback
                    .log(format!("Rechtschreibprüfung übersprungen: {error}"));
                Spelling::Unchecked
            }
        };
        // Nur Korrekturen übernehmen, nichts Erfundenes.
        let guarded = guard::keep_corrections(text, &answer.text, spelling);
        self.feedback.log(format!(
            "Überarbeitet: {} eingefügte Wörter verworfen, {} Wörter des Diktats behalten, {} richtig geschriebene Wörter geschützt",
            guarded.dropped_insertions, guarded.kept_originals, guarded.kept_correct
        ));
        Ok(Polished {
            text: guarded.text,
            fallback: answer.fallback,
        })
    }

    /// Bis wann eine Frage, die jetzt beginnt, dauern darf.
    fn deadline(&self) -> Instant {
        self.clock
            .now()
            .checked_add(TIMEOUT)
            .unwrap_or_else(|| self.clock.now())
    }

    /// Übersetzt ein Transkript Stück für Stück: erst den Titel, dann jede
    /// Überschrift und jeden Absatz ohne seine Zeitmarke. Scheitert das
    /// Modell an einem Stück, bleibt es im Original stehen. Zwischen zwei
    /// Stücken kommen wartende Diktate dran und wird der Abbruch geprüft;
    /// eine Frage an Apple Intelligence bricht er sofort ab. Das Modell steht
    /// für die ganze Übersetzung fest: Ein Wechsel unterwegs gilt erst für
    /// die nächste. `false`, wenn niemand mehr zuhört.
    fn translate(
        &mut self,
        request: &TranslationRequest,
        requests: &Receiver<PolishRequest>,
    ) -> bool {
        let started = self.clock.now();
        let backend = self.backend.clone();
        // Ein Startversuch je Übersetzung; scheitert er, weichen alle Stücke
        // gleich auf Apple Intelligence aus.
        self.models.retry_local();
        let instructions =
            translate::instructions(request.from.english_name(), request.to.english_name());
        let units = translate::split(&request.text);
        let texts: Vec<&str> = std::iter::once(request.title.as_str())
            .chain(units.iter().map(Unit::text))
            .collect();
        let total = texts.len();
        let mut answers = Answers::with_capacity(total);
        for (done, text) in (1..).zip(texts) {
            if request.cancel.load(Ordering::Relaxed) {
                return self.send(TranslationEvent::Done(TranslationOutcome::Cancelled));
            }
            if !self.dictations_first(requests) {
                return false;
            }
            let question = Question {
                text,
                instructions: &instructions,
                language: request.from,
            };
            let budget = Budget {
                deadline: self.deadline(),
                cancel: Some(&request.cancel),
            };
            answers.record(
                text,
                ask(&mut self.models, &self.clock, &backend, &question, budget),
            );
            if !self.send(TranslationEvent::Progress { done, total }) {
                return false;
            }
        }
        if request.cancel.load(Ordering::Relaxed) {
            return self.send(TranslationEvent::Done(TranslationOutcome::Cancelled));
        }
        self.feedback.log(format!(
            "Übersetzt ({} → {}): {total} Stücke in {:.0} s, {} im Original gelassen",
            request.from,
            request.to,
            self.clock
                .now()
                .saturating_duration_since(started)
                .as_secs_f64(),
            answers
                .untranslated
                .as_ref()
                .map_or(0, |untranslated| untranslated.count)
        ));
        self.send(TranslationEvent::Done(TranslationOutcome::Finished(
            Box::new(answers.into_translated(&units, total)),
        )))
    }

    /// Während einer Übersetzung: erledigt wartende Diktate sofort; eine
    /// weitere Übersetzung kommt danach dran. `false`, wenn niemand mehr
    /// zuhört.
    fn dictations_first(&mut self, requests: &Receiver<PolishRequest>) -> bool {
        while let Ok(request) = requests.try_recv() {
            if !self.dictation_first(request, requests) {
                return false;
            }
        }
        true
    }

    /// Eine Anfrage, die während einer Übersetzung eintrifft: Eine weitere
    /// Übersetzung wartet, alles andere wird gleich erledigt. `false`, wenn
    /// niemand mehr zuhört.
    fn dictation_first(
        &mut self,
        request: PolishRequest,
        requests: &Receiver<PolishRequest>,
    ) -> bool {
        match request {
            PolishRequest::Translate(_) => {
                self.pending.push_back(request);
                true
            }
            PolishRequest::Warmup
            | PolishRequest::UseBackend(_)
            | PolishRequest::Polish {
                dictation: _,
                text: _,
                language: _,
            }
            | PolishRequest::Shutdown => self.handle(request, requests),
        }
    }
}

/// Was eine Übersetzung bisher ergab, Stück für Stück.
#[derive(Debug)]
struct Answers {
    /// Je Stück die Übersetzung oder, wo das Modell scheiterte, das Original.
    texts: Vec<String>,
    /// Stücke, an denen das Modell scheiterte.
    untranslated: Option<Untranslated>,
    /// Warum das lokale Modell nicht ging, falls ausgewichen wurde.
    fallback: Option<Note>,
}

impl Answers {
    /// Noch keine Antworten; Platz für `total` Stücke.
    fn with_capacity(total: usize) -> Self {
        Self {
            texts: Vec::with_capacity(total),
            untranslated: None,
            fallback: None,
        }
    }

    /// Nimmt die Antwort auf die Frage nach `original` auf; scheiterte das
    /// Modell, bleibt das Original stehen.
    fn record(&mut self, original: &str, answer: Result<Polished, PolishError>) {
        match answer {
            Ok(answer) => {
                self.fallback = self.fallback.take().or(answer.fallback);
                self.texts.push(answer.text);
            }
            Err(error) => {
                self.untranslated = Some(Untranslated::add(self.untranslated.take(), error));
                self.texts.push(original.to_owned());
            }
        }
    }

    /// Setzt Titel und Text wieder zusammen: Das erste Stück ist der
    /// Titel, die übrigen gehören der Reihe nach zu `units`.
    fn into_translated(self, units: &[Unit], total: usize) -> Translated {
        let mut texts = self.texts.into_iter();
        // Ohne Stücke (kann nicht vorkommen: der Titel ist immer dabei)
        // bleibt der Titel leer.
        let title = texts.next().unwrap_or_default();
        let units: Vec<Unit> = units
            .iter()
            .zip(texts)
            .map(|(unit, text)| unit.with_text(text))
            .collect();
        Translated {
            title,
            text: translate::join(&units),
            total,
            untranslated: self.untranslated,
            fallback: self.fallback,
        }
    }
}

/// Die rohe Antwort des Modells `backend` (beim Überarbeiten und beim
/// Übersetzen), bis `budget.deadline`. Scheitert das lokale Modell
/// (llama.cpp fehlt, lädt nicht …), weicht Diktator in der verbleibenden
/// Zeit auf Apple Intelligence aus und nennt den Grund in `fallback`;
/// scheitert auch das, nennt der Fehler beide Gründe.
fn ask(
    models: &mut impl Models,
    clock: &impl Clock,
    backend: &Backend,
    question: &Question<'_>,
    budget: Budget<'_>,
) -> Result<Polished, PolishError> {
    match backend {
        Backend::Apple => Ok(Polished {
            text: models.apple(
                question.text,
                question.instructions,
                budget.limit(clock.now()),
            )?,
            fallback: None,
        }),
        Backend::Local { model, unload: _ } => ask_locally(models, clock, model, question, budget),
    }
}

/// Wie [`ask`] mit dem lokalen Modell `model`, Apple Intelligence als
/// Ausweg.
fn ask_locally(
    models: &mut impl Models,
    clock: &impl Clock,
    model: &Path,
    question: &Question<'_>,
    budget: Budget<'_>,
) -> Result<Polished, PolishError> {
    let remaining = budget.deadline.saturating_duration_since(clock.now());
    match models.local(question, model, remaining) {
        Ok(answer) => Ok(Polished {
            text: answer,
            fallback: None,
        }),
        Err(local) => match models.apple(
            question.text,
            question.instructions,
            budget.limit(clock.now()),
        ) {
            Ok(text) => Ok(Polished {
                text,
                fallback: Some(local.note()),
            }),
            Err(apple) => Err(PolishError::Both {
                local: local.note(),
                apple: Box::new(apple),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::polish::llama::LlamaError;

    /// Was der Überarbeiten-Thread meldet.
    #[derive(Debug)]
    enum Event {
        /// Ein Diktat.
        Polished,
        /// Eine Übersetzung.
        Translation,
    }

    impl From<PolishDone> for Event {
        fn from(_: PolishDone) -> Self {
            Self::Polished
        }
    }

    impl From<TranslationEvent> for Event {
        fn from(_: TranslationEvent) -> Self {
            Self::Translation
        }
    }

    #[test]
    fn stop_ends_the_thread_and_no_server_starts_afterwards() {
        let (events, _results) = mpsc::channel::<Event>();
        let mut polish = spawn(events, Feedback::silent(), ServerPid::new(None)).unwrap();
        polish.stop().unwrap();
        assert!(
            polish.requests().send(PolishRequest::Warmup).is_err(),
            "der Thread läuft nach stop weiter"
        );
        let mut late = Llama::new(polish.server.share());
        let error = late.url_for(Path::new("/gibt/es/nicht.gguf")).unwrap_err();
        assert!(matches!(error, LlamaError::Closed), "{error:?}");
        polish.stop().unwrap();
    }

    use crate::clock::FakeClock;
    use crate::helper::{Helper, HelperError};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    /// Was das nachgebaute Apple Intelligence bekam.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum AppleCall {
        /// Mit dieser Zeit und ohne Abbruch-Flag.
        After(Duration),
        /// Mit dieser Zeit und Abbruch-Flag.
        AfterOrCancel(Duration),
    }

    /// Wie sich ein nachgebautes Modell verhält.
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    enum Behaviour {
        /// Antwortet.
        #[default]
        Answers,
        /// Scheitert.
        Fails,
    }

    /// Nachgebaute Modelle: antworten mit dem Text in Großbuchstaben oder
    /// scheitern auf Wunsch; zählen mit.
    #[derive(Default)]
    struct FakeModels {
        apple: Behaviour,
        local: Behaviour,
        /// So weit stellt eine lokale Frage die Uhr vor.
        local_takes: Duration,
        clock: Option<Rc<FakeClock>>,
        apple_calls: Rc<RefCell<Vec<AppleCall>>>,
        local_calls: Rc<RefCell<usize>>,
        stopped: Rc<RefCell<usize>>,
        last_used: Option<Instant>,
        /// Setzt beim ersten Stück einer Übersetzung das Abbruch-Flag.
        cancel_on_first: Option<Arc<AtomicBool>>,
    }

    impl Models for FakeModels {
        fn apple(
            &mut self,
            text: &str,
            _instructions: &str,
            limit: Limit<'_>,
        ) -> Result<String, PolishError> {
            self.apple_calls.borrow_mut().push(match limit {
                Limit::After(time) => AppleCall::After(time),
                Limit::AfterOrCancel(time, _) => AppleCall::AfterOrCancel(time),
            });
            if let Some(cancel) = self.cancel_on_first.take() {
                cancel.store(true, Ordering::Relaxed);
            }
            if self.apple == Behaviour::Fails {
                return Err(PolishError::Apple(HelperError::Missing(Helper::Llm)));
            }
            Ok(text.to_uppercase())
        }

        fn local(
            &mut self,
            question: &Question<'_>,
            _model: &Path,
            _timeout: Duration,
        ) -> Result<String, PolishError> {
            *self.local_calls.borrow_mut() += 1;
            if let Some(clock) = &self.clock {
                clock.advance(self.local_takes);
            }
            if self.local == Behaviour::Fails {
                return Err(PolishError::Llama(LlamaError::MissingProgram));
            }
            Ok(question.text.to_uppercase())
        }

        fn warm_up(&mut self, _model: &Path) -> Result<Warmed, LlamaError> {
            Ok(Warmed::Started)
        }

        fn last_used(&self) -> Option<Instant> {
            self.last_used
        }

        fn stop_local(&mut self) {
            *self.stopped.borrow_mut() += 1;
            self.last_used = None;
        }

        fn retry_local(&mut self) {}
    }

    /// Hält nichts für richtig geschrieben.
    struct NoSpelling;

    impl Speller for NoSpelling {
        fn correct_words(
            &self,
            _text: &str,
            _language: Language,
        ) -> Result<BTreeSet<String>, SpellingError> {
            Ok(BTreeSet::new())
        }
    }

    fn local_backend(unload: Unload) -> Backend {
        Backend::Local {
            model: PathBuf::from("/m.gguf"),
            unload,
        }
    }

    /// Lässt einen Überarbeiten-Thread mit `models` die `requests`
    /// abarbeiten (auf dem Thread des Tests) und liefert seine Ereignisse.
    fn served(backend: Backend, models: FakeModels, requests: Vec<PolishRequest>) -> Vec<Outcome> {
        let (sender, receiver) = mpsc::channel();
        for request in requests {
            sender.send(request).unwrap();
        }
        drop(sender);
        let (events, results) = mpsc::channel();
        let polisher = Polisher {
            backend,
            models,
            spelling: NoSpelling,
            clock: FakeClock::new(),
            pending: VecDeque::new(),
            events,
            feedback: Feedback::silent(),
        };
        polisher.serve(&receiver);
        results.try_iter().collect()
    }

    /// Was der Thread meldete.
    #[derive(Debug)]
    enum Outcome {
        Polished(PolishDone),
        Translation(TranslationEvent),
    }

    impl From<PolishDone> for Outcome {
        fn from(done: PolishDone) -> Self {
            Self::Polished(done)
        }
    }

    impl From<TranslationEvent> for Outcome {
        fn from(event: TranslationEvent) -> Self {
            Self::Translation(event)
        }
    }

    fn polish_request(text: &str) -> PolishRequest {
        PolishRequest::Polish {
            dictation: DictationId::new(1),
            text: text.to_owned(),
            language: Language::German,
        }
    }

    fn polished(outcomes: Vec<Outcome>) -> Result<Polished, PolishError> {
        outcomes
            .into_iter()
            .find_map(|outcome| match outcome {
                Outcome::Polished(done) => Some(done.result),
                Outcome::Translation(_) => None,
            })
            .expect("kein Ergebnis")
    }

    fn translation(text: &str, cancel: &Arc<AtomicBool>) -> PolishRequest {
        PolishRequest::Translate(TranslationRequest {
            title: "Titel".to_owned(),
            text: text.to_owned(),
            from: Language::Spanish,
            to: Language::German,
            cancel: Arc::clone(cancel),
        })
    }

    /// Wie Übersetzungen endeten.
    fn translations_done(outcomes: &[Outcome]) -> Vec<&TranslationOutcome> {
        outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                Outcome::Translation(TranslationEvent::Done(done)) => Some(done),
                Outcome::Translation(TranslationEvent::Progress { done: _, total: _ })
                | Outcome::Polished(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_failing_local_model_falls_back_to_apple_and_says_why() {
        let models = FakeModels {
            local: Behaviour::Fails,
            ..FakeModels::default()
        };
        let result = polished(served(
            local_backend(Unload::Never),
            models,
            vec![polish_request("hallo")],
        ))
        .unwrap();
        assert_eq!(result.text, "HALLO", "die Antwort von Apple Intelligence");
        assert!(
            result
                .fallback
                .is_some_and(|reason| reason.de().starts_with("llama.cpp fehlt")),
            "Grund des Ausweichens"
        );
    }

    #[test]
    fn when_both_fail_both_reasons_are_reported() {
        let models = FakeModels {
            local: Behaviour::Fails,
            apple: Behaviour::Fails,
            ..FakeModels::default()
        };
        let error = polished(served(
            local_backend(Unload::Never),
            models,
            vec![polish_request("hallo")],
        ))
        .unwrap_err();
        let shown = error.to_string();
        assert!(
            shown.contains("llama.cpp fehlt") && shown.contains("diktator-llm fehlt"),
            "{shown}"
        );
    }

    #[test]
    fn the_fallback_gets_only_the_time_left_of_one_budget() {
        let clock = Rc::new(FakeClock::new());
        let mut models = FakeModels {
            local: Behaviour::Fails,
            local_takes: Duration::from_secs(100),
            clock: Some(Rc::clone(&clock)),
            ..FakeModels::default()
        };
        let calls = Rc::clone(&models.apple_calls);
        let budget = Budget {
            deadline: clock.now() + TIMEOUT,
            cancel: None,
        };
        let question = Question {
            text: "hallo",
            instructions: "x",
            language: Language::German,
        };
        ask(
            &mut models,
            &*clock,
            &local_backend(Unload::Never),
            &question,
            budget,
        )
        .unwrap();
        assert_eq!(
            *calls.borrow(),
            [AppleCall::After(Duration::from_secs(20))],
            "zusammen nicht mehr als eine Frist"
        );
    }

    #[test]
    fn a_cancelled_translation_ends_once_and_cancels_apple_too() {
        let cancel = Arc::new(AtomicBool::new(false));
        let models = FakeModels {
            cancel_on_first: Some(Arc::clone(&cancel)),
            ..FakeModels::default()
        };
        let calls = Rc::clone(&models.apple_calls);
        let outcomes = served(
            Backend::Apple,
            models,
            vec![translation("Uno.\n\nDos.", &cancel)],
        );
        let done = translations_done(&outcomes);
        assert_eq!(done.len(), 1, "genau ein Ende: {outcomes:?}");
        assert!(matches!(done[0], TranslationOutcome::Cancelled), "{done:?}");
        assert!(
            matches!(calls.borrow()[0], AppleCall::AfterOrCancel(_)),
            "Apple Intelligence bekommt das Abbruch-Flag"
        );
    }

    #[test]
    fn the_model_stays_the_same_for_a_whole_translation() {
        let cancel = Arc::new(AtomicBool::new(false));
        let models = FakeModels::default();
        let local_calls = Rc::clone(&models.local_calls);
        let outcomes = served(
            Backend::Apple,
            models,
            vec![
                translation("Uno.\n\nDos.", &cancel),
                PolishRequest::UseBackend(local_backend(Unload::Never)),
            ],
        );
        assert_eq!(translations_done(&outcomes).len(), 1);
        assert_eq!(*local_calls.borrow(), 0, "der Wechsel gilt erst danach");
    }

    #[test]
    fn an_idle_local_model_is_stopped() {
        let models = FakeModels {
            last_used: Some(SystemClock.now()),
            ..FakeModels::default()
        };
        let stopped = Rc::clone(&models.stopped);
        let (sender, receiver) = mpsc::channel::<PolishRequest>();
        let keep_open = thread::Builder::new()
            .spawn(move || {
                SystemClock.sleep(Duration::from_millis(100));
                drop(sender);
            })
            .unwrap();
        let (events, _results) = mpsc::channel::<Outcome>();
        let polisher = Polisher {
            backend: local_backend(Unload::AfterIdle(Duration::from_millis(1))),
            models,
            spelling: NoSpelling,
            clock: SystemClock,
            pending: VecDeque::new(),
            events,
            feedback: Feedback::silent(),
        };
        polisher.serve(&receiver);
        keep_open.join().unwrap();
        assert_eq!(*stopped.borrow(), 1, "nach dem Leerlauf beendet");
    }
}
