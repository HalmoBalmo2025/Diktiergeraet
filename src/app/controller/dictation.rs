//! Diktat: Aufnahme, Teile ans Modell, Einfügen, Überarbeiten.
//!
//! Was schiefgeht (ein Teil, das Einfügen, das Mikrofon, das Überarbeiten),
//! sammelt das Diktat in [`Dictation::problems`]. Jede Meldung zum Diktat
//! nennt sie zuerst, auch die am Ende: Eine spätere Meldung ersetzt nie
//! einen Fehler, den der Nutzer noch nicht gesehen hat. Fällt das Mikrofon
//! aus, endet das Diktat trotzdem normal mit dem, was schon aufgenommen
//! und erkannt ist.

use super::{Controller, Microphone, PanelState, Platform, State};
use crate::app::status::UiUpdate;
use crate::app::worker::{Job, PartId, PartKind};
use crate::audio::chunk::PartTiming;
use crate::audio::record::{Interrupted, RecordError};
use crate::audio::{self, Pcm16k};
use crate::context::{Cursor, CursorText};
use crate::dictation_id::DictationId;
use crate::engine::Unfinished;
use crate::feedback::Sound;
use crate::hotkey::Mode;
use crate::language::Language;
use crate::note::{Explain, Note, Words};
use crate::paste;
use crate::polish::{PolishDone, PolishRequest, Polished};
use crate::text::{self, AppStyle};
use std::time::Duration;

/// So viele Zeichen des zuletzt eingefügten Teils merkt sich Diktator als
/// Umgebung für den nächsten.
const MEMORY_CHARS: usize = 40;

/// So lange nimmt ein Diktat auf, ohne dass ein Teil abgeht (bei „erst am
/// Ende“); dann endet die Aufnahme von selbst, damit ein vergessenes
/// Mikrofon nicht unbegrenzt Speicher füllt und das Modell nicht an einem
/// riesigen Stück rechnet.
pub(super) const MAX_UNSENT: Duration = Duration::from_mins(10);

/// Das laufende Diktat: was davon schon erkannt ist. Bleibt bis zum letzten
/// Teil bestehen, auch wenn die Aufnahme schon beendet ist.
pub(super) struct Dictation {
    /// Fortlaufende Nummer (Ergebnisse verworfener Diktate werden daran
    /// erkannt und ignoriert).
    pub(super) id: DictationId,
    /// Sprache.
    pub(super) language: Language,
    /// Roh (Teile sofort einfügen) oder überarbeiten (am Ende alles auf
    /// einmal, nach dem Sprachmodell).
    pub(super) mode: Mode,
    /// Bisher erkannter Text aller Teile.
    pub(super) text: String,
    /// Bisher abgeschickte Teile (für das Protokoll).
    pub(super) parts: usize,
    /// Wann Teile abgehen; fest ab dem Start der Aufnahme.
    pub(super) timing: PartTiming,
    /// Ende des zuletzt eingefügten Teils: Umgebung für den nächsten Teil,
    /// wenn die App die Schreibmarke nicht verrät (Mail, Terminal).
    pub(super) last_inserted: Option<String>,
    /// Wohin der Text geht; fest ab dem Start der Aufnahme, damit Öffnen
    /// oder Schließen des Panels mitten im Diktat keinen Teil verliert oder
    /// doppelt liefert.
    pub(super) destination: Destination,
    /// Was beim Einfügen bisher geschah.
    pub(super) pasted: Pasted,
    /// Was bisher schiefging, in der Reihenfolge des Geschehens.
    pub(super) problems: Vec<Note>,
}

/// Wohin ein Diktat geht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Destination {
    /// An die Schreibmarke der App vorn (Panel geschlossen).
    Cursor,
    /// In die Zwischenablage: Das Panel hat den Fokus, an der Schreibmarke
    /// landete der Text im Panel.
    Clipboard,
}

impl Destination {
    /// Wohin ein Diktat geht, das bei diesem Stand des Panels beginnt.
    const fn for_panel(panel: PanelState) -> Self {
        match panel {
            PanelState::Closed => Self::Cursor,
            PanelState::Open => Self::Clipboard,
        }
    }
}

/// Was beim Einfügen eines Diktats bisher geschah.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pasted {
    /// Noch nichts eingefügt.
    Nothing,
    /// Jedes Einfügen gelang.
    All,
    /// Mindestens ein Einfügen scheiterte.
    Failed,
}

impl Dictation {
    /// Hängt den Text eines Teils an; Teile auf Japanisch und Chinesisch
    /// ohne Leerzeichen dazwischen.
    fn append(&mut self, part: &str) {
        if !self.text.is_empty() {
            self.text.push_str(self.language.gap());
        }
        self.text.push_str(part);
    }

    /// `note`, mit den Problemen des Diktats davor.
    fn with_problems(&self, note: Option<&Note>) -> Note {
        Note::joined(self.problems.iter().chain(note))
    }
}

impl<P: Platform> Controller<P> {
    /// fn (oder fn + ⇧) wurde angetippt. Auch während das Modell noch lädt,
    /// wird schon aufgenommen: Der Modell-Thread nimmt die Teile an, sobald
    /// es bereit ist.
    pub(super) fn on_tap(&mut self, state: State, language: Language, mode: Mode) -> State {
        match state {
            State::Ready | State::Loading => self.start_recording(language, mode),
            State::Recording {
                recording,
                dictation,
            } => self.finish_recording(recording, dictation),
            busy @ (State::Processing(_) | State::Polishing(_)) => {
                self.feedback.play(Sound::Error);
                self.note(Note::new(
                    "Die letzte Aufnahme wird noch verarbeitet.",
                    "The last recording is still being processed.",
                ));
                busy
            }
            State::Unavailable { reason } => {
                self.feedback.play(Sound::Error);
                self.feedback.notify(reason.clone());
                State::Unavailable { reason }
            }
        }
    }

    /// Esc während der Aufnahme oder der Verarbeitung verwirft das Diktat;
    /// schon eingefügte Teile bleiben stehen, und was schon erkannt ist,
    /// kommt in den Verlauf.
    /// Esc während des Überarbeitens fügt gleich den rohen Text ein; die
    /// späte Antwort wird dann verworfen.
    pub(super) fn on_escape(&mut self, state: State) -> State {
        match state {
            State::Recording {
                recording,
                dictation,
            } => {
                drop(recording);
                self.discard(&dictation)
            }
            // Auch wenn das Modell noch rechnet (etwa hinter einem langen
            // Abschnitt eines Transkripts): Die späten Antworten werden an
            // der Nummer erkannt und verworfen.
            State::Processing(dictation) => self.discard(&dictation),
            State::Polishing(mut dictation) => {
                dictation.problems.push(Note::new(
                    "Überarbeiten abgebrochen – roher Text.",
                    "Polishing cancelled – raw text.",
                ));
                self.deliver(&mut dictation);
                self.idle()
            }
            other @ (State::Loading | State::Ready | State::Unavailable { reason: _ }) => other,
        }
    }

    /// Verwirft ein Diktat auf Esc. Was schon erkannt ist, kommt in den
    /// Verlauf; schon eingefügte Teile bleiben stehen.
    fn discard(&mut self, dictation: &Dictation) -> State {
        self.feedback.play(Sound::Nothing);
        // Seine noch wartenden Teile braucht niemand mehr; scheitert nur,
        // wenn der Modell-Thread schon weg ist.
        if self
            .jobs
            .send(Job::Discard {
                dictation: dictation.id,
            })
            .is_err()
        {
            self.feedback
                .log("Verwerfen nicht gemeldet: Modell-Thread beendet");
        }
        if !dictation.text.is_empty() {
            self.history.push(&dictation.text);
            self.platform.show(UiUpdate::History(self.history.items()));
        }
        let cancelled = match dictation.pasted {
            Pasted::All => Note::new(
                "Diktat abgebrochen – schon eingefügter Text bleibt.",
                "Dictation cancelled – text already inserted stays.",
            ),
            Pasted::Nothing | Pasted::Failed if dictation.text.is_empty() => {
                Note::new("Diktat abgebrochen.", "Dictation cancelled.")
            }
            Pasted::Nothing | Pasted::Failed => Note::new(
                "Diktat abgebrochen – erkannter Text steht im Verlauf.",
                "Dictation cancelled – the recognised text is in the history.",
            ),
        };
        self.note(dictation.with_problems(Some(&cancelled)));
        self.idle()
    }

    /// Mikrofon öffnen und die Modelle schon laden lassen, während
    /// gesprochen wird.
    fn start_recording(&mut self, language: Language, mode: Mode) -> State {
        match self.platform.record() {
            Ok(recording) => {
                self.warm_up(mode);
                let dictation = Dictation {
                    id: self.next_dictation,
                    language,
                    mode,
                    text: String::new(),
                    parts: 0,
                    timing: self.parts,
                    last_inserted: None,
                    destination: Destination::for_panel(self.panel),
                    pasted: Pasted::Nothing,
                    problems: Vec::new(),
                };
                self.next_dictation = self.next_dictation.next();
                // Die Anzeige an der Schreibmarke beginnt bei null.
                self.meter = audio::Level::SILENT;
                self.feedback.play(Sound::Start);
                // Die Meldung zum letzten Diktat gilt nicht mehr.
                self.platform.show(UiUpdate::ClearNote);
                State::Recording {
                    recording,
                    dictation,
                }
            }
            Err(error) => {
                self.feedback.play(Sound::Error);
                self.note(Note::new(
                    format!("Aufnahme nicht möglich: {error}. Mikrofon-Recht prüfen."),
                    format!(
                        "Cannot record: {}. Check the microphone permission.",
                        error.english()
                    ),
                ));
                self.idle()
            }
        }
    }

    /// Lässt entladene Modelle laden. Läuft ein Thread nicht mehr, merkt
    /// das der erste echte Auftrag.
    fn warm_up(&self, mode: Mode) {
        let sent = self.jobs.send(Job::Warmup).is_ok()
            && match mode {
                Mode::Raw => true,
                Mode::Polish => self.polish.send(PolishRequest::Warmup).is_ok(),
            };
        if !sent {
            self.feedback
                .log("Vorab laden nicht möglich: Thread beendet");
        }
    }

    /// Taktgeber: Ist während der Aufnahme ein Teil bis zu einer Sprechpause
    /// fertig, geht er gleich ans Modell.
    pub(super) fn on_tick(&mut self, state: State) -> State {
        match state {
            State::Recording {
                recording,
                dictation,
            } => self.tick_recording(recording, dictation),
            other @ (State::Loading
            | State::Ready
            | State::Processing(_)
            | State::Polishing(_)
            | State::Unavailable { reason: _ }) => other,
        }
    }

    /// Ein Takt während der Aufnahme.
    fn tick_recording(
        &mut self,
        mut recording: Box<dyn Microphone>,
        mut dictation: Dictation,
    ) -> State {
        match recording.next_part(dictation.timing) {
            Ok(Some(pcm)) => {
                if self.send_part(&mut dictation, pcm, PartKind::Partial) {
                    State::Recording {
                        recording,
                        dictation,
                    }
                } else {
                    self.worker_gone(State::Processing(dictation))
                }
            }
            Ok(None) if recording.unsent() >= MAX_UNSENT => {
                let minutes = MAX_UNSENT.as_secs() / 60;
                self.dictation_problem(
                    &mut dictation,
                    Note::new(
                        format!("Aufnahme nach {minutes} Minuten beendet."),
                        format!("Recording stopped after {minutes} minutes."),
                    ),
                );
                self.finish_recording(recording, dictation)
            }
            Ok(None) => State::Recording {
                recording,
                dictation,
            },
            Err(error) => {
                self.recording_broke(&mut dictation, &error);
                // Was bis zum Ausfall aufgenommen war, geht als letzter Teil
                // ans Modell; danach endet das Diktat wie sonst.
                let rest = recording
                    .finish()
                    .unwrap_or_else(|Interrupted { captured, error: _ }| captured)
                    .pcm;
                self.send_last(dictation, rest)
            }
        }
    }

    /// Pegeltakt: Während der Aufnahme geht die Pegelstufe an die Anzeige,
    /// sobald sie sich ändert.
    pub(super) fn on_meter(&mut self, state: &State) {
        match state {
            State::Recording {
                recording,
                dictation: _,
            } => {
                let meter = audio::meter(recording.level());
                if meter != self.meter {
                    self.meter = meter;
                    self.platform.show(UiUpdate::Level(meter));
                }
            }
            State::Loading
            | State::Ready
            | State::Processing(_)
            | State::Polishing(_)
            | State::Unavailable { reason: _ } => {}
        }
    }

    /// Gibt einen Teil des Diktats ans Modell. `false`, wenn der
    /// Modell-Thread weg ist.
    fn send_part(&self, dictation: &mut Dictation, pcm: Pcm16k, kind: PartKind) -> bool {
        dictation.parts = dictation.parts.saturating_add(1);
        self.feedback.log(format!(
            "Teil {} ({}) geht ans Modell: {:.1} s",
            dictation.parts,
            match kind {
                PartKind::Partial => "während der Aufnahme",
                PartKind::Last => "Rest",
            },
            pcm.duration().as_secs_f64()
        ));
        let part = PartId {
            dictation: dictation.id,
            kind,
        };
        self.jobs
            .send(Job::Dictation {
                pcm,
                language: dictation.language,
                part,
            })
            .is_ok()
    }

    /// Gibt den Rest der Aufnahme ans Modell; das Diktat wartet dann auf
    /// den letzten Teil.
    fn send_last(&mut self, mut dictation: Dictation, pcm: Pcm16k) -> State {
        if !self.send_part(&mut dictation, pcm, PartKind::Last) {
            return self.worker_gone(State::Processing(dictation));
        }
        State::Processing(dictation)
    }

    /// Mikrofon schließen und den Rest der Aufnahme ans Modell geben.
    fn finish_recording(
        &mut self,
        recording: Box<dyn Microphone>,
        mut dictation: Dictation,
    ) -> State {
        self.feedback.play(Sound::Stop);
        let captured = match recording.finish() {
            Ok(captured) => captured,
            Err(Interrupted { error, captured }) => {
                self.recording_broke(&mut dictation, &error);
                captured
            }
        };
        let pcm = captured.pcm;
        self.feedback.log(format!(
            "Aufnahme {} beendet ({}): {:.1} s, {} Aussetzer gemeldet",
            dictation.language,
            captured.device.label(),
            pcm.duration().as_secs_f64(),
            captured.glitches
        ));
        self.send_last(dictation, pcm)
    }

    /// Das Mikrofon ist mitten im Diktat ausgefallen: melden; das Diktat
    /// geht mit dem weiter, was schon aufgenommen ist.
    fn recording_broke(&self, dictation: &mut Dictation, error: &RecordError) {
        self.feedback.play(Sound::Error);
        self.dictation_problem(
            dictation,
            Note::new(
                format!("Aufnahme fehlgeschlagen: {error}."),
                format!("Recording failed: {}.", error.english()),
            ),
        );
    }

    /// Merkt sich ein Problem des Diktats und zeigt alle bisherigen.
    fn dictation_problem(&self, dictation: &mut Dictation, problem: Note) {
        dictation.problems.push(problem);
        self.note(dictation.with_problems(None));
    }

    /// Ein Teil eines Diktats ist erkannt: gleich einfügen (oder bei offenem
    /// Panel sammeln). Mit dem letzten Teil ist das Diktat fertig. Scheiterte
    /// das Modell mitten im Teil, zählt, was es bis dahin erkannte.
    pub(super) fn on_part(
        &mut self,
        mut state: State,
        part: PartId,
        result: Result<Option<String>, Unfinished>,
    ) -> State {
        let Some(dictation) = state.dictation_mut(part.dictation) else {
            self.feedback
                .log("Ergebnis eines verworfenen Diktats ignoriert");
            return state;
        };
        match result {
            Ok(Some(text)) => self.take_part(dictation, &text),
            Ok(None) => {}
            Err(Unfinished { text, error }) => {
                if !text.is_empty() {
                    self.take_part(dictation, &text);
                }
                self.feedback.play(Sound::Error);
                self.dictation_problem(
                    dictation,
                    Note::new(
                        format!("Transkription fehlgeschlagen: {error}."),
                        format!("Transcription failed: {}.", error.english()),
                    ),
                );
            }
        }
        match part.kind {
            PartKind::Partial => {
                let words = Words::of(&dictation.text);
                self.note(dictation.with_problems(Some(&Note::new(
                    format!("{} bisher …", words.de()),
                    format!("{} so far …", words.en()),
                ))));
                state
            }
            PartKind::Last => self.finish_processing(state),
        }
    }

    /// Nimmt den Text eines Teils ins Diktat; im rohen Modus an die
    /// Schreibmarke gleich eingefügt.
    fn take_part(&self, dictation: &mut Dictation, text: &str) {
        match dictation.mode {
            Mode::Raw => match dictation.destination {
                Destination::Cursor => self.paste_part(dictation, text),
                Destination::Clipboard => {}
            },
            Mode::Polish => {}
        }
        dictation.append(text);
    }

    /// Der letzte Teil ist erkannt: überarbeiten lassen oder fertig.
    fn finish_processing(&mut self, state: State) -> State {
        match state {
            State::Processing(dictation) => self.finish_last(dictation),
            // Der letzte Teil geht erst beim Beenden der Aufnahme ans Modell;
            // in den anderen Zuständen ist er nicht der letzte des Diktats.
            other @ (State::Recording {
                recording: _,
                dictation: _,
            }
            | State::Polishing(_)
            | State::Loading
            | State::Ready
            | State::Unavailable { reason: _ }) => other,
        }
    }

    /// Mit dem letzten Teil: überarbeiten lassen oder abschließen.
    fn finish_last(&mut self, mut dictation: Dictation) -> State {
        match dictation.mode {
            // Die Teile sind schon eingefügt.
            Mode::Raw => self.finish_dictation(&dictation),
            Mode::Polish if dictation.text.is_empty() => self.finish_dictation(&dictation),
            Mode::Polish => {
                if self.start_polish(&dictation) {
                    return State::Polishing(dictation);
                }
                // Läuft der Überarbeiten-Thread nicht, wird roh eingefügt;
                // die Teile wurden bisher zurückgehalten.
                self.deliver(&mut dictation);
            }
        }
        self.idle()
    }

    /// Schließt ein Diktat ohne Überarbeiten ab, mit dem, was schon erkannt
    /// ist: Im rohen Modus sind die Teile schon eingefügt, sonst wird der
    /// rohe Text eingefügt.
    pub(super) fn finish_unpolished(&mut self, dictation: &mut Dictation) {
        match dictation.mode {
            Mode::Raw => self.finish_dictation(dictation),
            Mode::Polish => self.deliver(dictation),
        }
    }

    /// Gibt das fertige Diktat an den Überarbeiten-Thread. `false`, wenn er
    /// nicht mehr läuft; dann wird roh eingefügt.
    fn start_polish(&self, dictation: &Dictation) -> bool {
        let request = PolishRequest::Polish {
            dictation: dictation.id,
            text: dictation.text.clone(),
            language: dictation.language,
        };
        if self.polish.send(request).is_err() {
            self.feedback
                .log("Überarbeiten-Thread beendet; roher Text wird eingefügt");
            return false;
        }
        self.note(dictation.with_problems(Some(&Note::new("Wird überarbeitet …", "Polishing …"))));
        true
    }

    /// Das Überarbeiten ist fertig: überarbeiteten Text einfügen; ging es
    /// schief, den rohen.
    pub(super) fn on_polished(&mut self, state: State, done: PolishDone) -> State {
        match state {
            State::Polishing(dictation) if dictation.id == done.dictation => {
                self.polished(dictation, done)
            }
            other @ (State::Polishing(_)
            | State::Loading
            | State::Ready
            | State::Recording {
                recording: _,
                dictation: _,
            }
            | State::Processing(_)
            | State::Unavailable { reason: _ }) => {
                self.feedback
                    .log("Überarbeitung eines verworfenen Diktats ignoriert");
                other
            }
        }
    }

    /// Das Überarbeiten des laufenden Diktats ist fertig.
    fn polished(&mut self, mut dictation: Dictation, done: PolishDone) -> State {
        match done.result {
            Ok(Polished { text, fallback }) => {
                dictation.text = text;
                if let Some(local) = fallback {
                    dictation.problems.push(super::apple_fallback(&local));
                }
            }
            Err(error) => {
                self.feedback.play(Sound::Error);
                dictation.problems.push(Note::new(
                    format!("Nicht überarbeitet: {error}. Roher Text."),
                    format!("Not polished: {}. Raw text.", error.english()),
                ));
            }
        }
        self.deliver(&mut dictation);
        self.idle()
    }

    /// Fügt das ganze (überarbeitete oder rohe) Diktat ein und schließt es ab.
    fn deliver(&mut self, dictation: &mut Dictation) {
        match dictation.destination {
            Destination::Cursor => {
                let text = dictation.text.clone();
                self.paste_part(dictation, &text);
            }
            Destination::Clipboard => {}
        }
        self.finish_dictation(dictation);
    }

    /// Fügt einen Teil an der Schreibmarke ein.
    fn paste_part(&self, dictation: &mut Dictation, text: &str) {
        let previous = dictation.last_inserted.as_deref();
        let fitted = self.fit_to_cursor(text, dictation.language, previous);
        match self.platform.paste(&fitted) {
            Ok(clipboard) => {
                // Leerer Text hinterlässt keine Umgebung für den nächsten.
                dictation.last_inserted =
                    Some(tail_chars(&fitted, MEMORY_CHARS)).filter(|tail| !tail.is_empty());
                dictation.pasted = match dictation.pasted {
                    Pasted::Nothing | Pasted::All => Pasted::All,
                    Pasted::Failed => Pasted::Failed,
                };
                match clipboard {
                    paste::Pasted::Restored | paste::Pasted::LeftNewer => {}
                    // Eingefügt ist es; nur der alte Inhalt fehlt.
                    paste::Pasted::NotRestored(error) => {
                        self.feedback.play(Sound::Error);
                        self.dictation_problem(
                            dictation,
                            Note::new(
                                format!(
                                    "Die Zwischenablage ließ sich nicht wiederherstellen: {error}."
                                ),
                                format!(
                                    "The clipboard could not be restored: {}.",
                                    error.english()
                                ),
                            ),
                        );
                    }
                }
            }
            Err(error) => {
                dictation.pasted = Pasted::Failed;
                self.feedback.play(Sound::Error);
                self.dictation_problem(
                    dictation,
                    Note::new(
                        format!("Einfügen fehlgeschlagen: {error}. Text steht im Verlauf."),
                        format!(
                            "Inserting failed: {}. The text is in the history.",
                            error.english()
                        ),
                    ),
                );
            }
        }
    }

    /// Das Diktat ist fertig: Verlauf, bei offenem Panel in die
    /// Zwischenablage, und eine Meldung aus allem, was mit ihm geschah.
    fn finish_dictation(&mut self, dictation: &Dictation) {
        if dictation.text.is_empty() {
            self.feedback.play(Sound::Nothing);
            self.note(dictation.with_problems(Some(&Note::new(
                "Keine Sprache erkannt.",
                "No speech recognised.",
            ))));
            return;
        }
        self.history.push(&dictation.text);
        self.platform.show(UiUpdate::History(self.history.items()));
        let outcome = match dictation.destination {
            Destination::Clipboard => Some(self.copy_dictation(&dictation.text)),
            Destination::Cursor => match dictation.pasted {
                // Die Fehlermeldungen sagen schon, was fehlt und wo der Text steht.
                Pasted::Failed => None,
                Pasted::Nothing | Pasted::All => {
                    let words = Words::of(&dictation.text);
                    Some(Note::new(
                        format!("{} eingefügt.", words.de()),
                        format!("{} inserted.", words.en()),
                    ))
                }
            },
        };
        self.note(dictation.with_problems(outcome.as_ref()));
    }

    /// Legt ein Diktat in die Zwischenablage, weil gerade das Panel den
    /// Fokus hat und es dort nirgends hin kann; liefert die Meldung dazu.
    fn copy_dictation(&self, text: &str) -> Note {
        let words = Words::of(text);
        match self.platform.copy(text) {
            Ok(()) => Note::new(
                format!("{} in der Zwischenablage – mit ⌘V einfügen.", words.de()),
                format!("{} on the clipboard – paste with ⌘V.", words.en()),
            ),
            Err(error) => {
                self.feedback.play(Sound::Error);
                Note::new(
                    format!("Kopieren fehlgeschlagen: {error}. Text steht im Verlauf."),
                    format!(
                        "Copying failed: {}. The text is in the history.",
                        error.english()
                    ),
                )
            }
        }
    }

    /// Passt das Diktat an den Text um die Schreibmarke und an die App an
    /// (Kleinschreibung mitten im Satz, Leerzeichen, Schlusspunkt; je App
    /// ohne Schlusspunkt oder als vollständiger Satz). Ist der Text dort
    /// nicht lesbar, dient `previous` (das Ende des vorigen Teils desselben
    /// Diktats) als Text links der Schreibmarke.
    fn fit_to_cursor(&self, dictated: &str, language: Language, previous: Option<&str>) -> String {
        let (style, read) = match self.platform.cursor() {
            Ok(Cursor {
                app: Some(app),
                text,
            }) => {
                let style = AppStyle::of(&app);
                self.feedback.log(format!("App {app}: {}", style.label()));
                (style, text)
            }
            Ok(Cursor { app: None, text }) => {
                let style = AppStyle::Adaptive;
                self.feedback
                    .log(format!("App unbekannt: {}", style.label()));
                (style, text)
            }
            Err(error) => (
                AppStyle::Adaptive,
                CursorText::Unknown {
                    reason: error.to_string(),
                },
            ),
        };
        let surroundings = match read {
            CursorText::Known(surroundings) => {
                // Nur Längen und Zeichenart: Der Text gehört dem Nutzer.
                self.feedback.log(format!(
                    "Schreibmarke: links {} Zeichen ({}), rechts {} Zeichen ({})",
                    surroundings.before.chars().count(),
                    char_kind(surroundings.before.chars().next_back()),
                    surroundings.after.chars().count(),
                    char_kind(surroundings.after.chars().next()),
                ));
                surroundings
            }
            CursorText::Unknown { reason } => {
                let source = if previous.is_some() {
                    " – Ende des vorigen Teils als Umgebung"
                } else {
                    ""
                };
                self.feedback
                    .log(format!("Schreibmarke nicht lesbar ({reason}){source}"));
                text::Surroundings {
                    before: previous.unwrap_or_default().to_owned(),
                    after: String::new(),
                }
            }
        };
        let fitted = text::fit(dictated, &surroundings, language, style);
        self.feedback.log(if fitted == dictated {
            "Diktat unverändert eingefügt"
        } else {
            "Diktat angepasst eingefügt"
        });
        fitted
    }
}

/// Art des Zeichens direkt an der Schreibmarke, fürs Protokoll.
fn char_kind(next_to_cursor: Option<char>) -> &'static str {
    match next_to_cursor {
        None => "nichts",
        Some('\n') => "Zeilenumbruch",
        Some(c) if c.is_whitespace() => "Leerraum",
        Some('.' | '!' | '?' | ':') => "Satzende",
        Some(c) if c.is_alphabetic() => "Buchstabe",
        Some(c) if c.is_numeric() => "Ziffer",
        Some(_) => "Zeichen",
    }
}

/// Die letzten `count` Zeichen von `text`.
fn tail_chars(text: &str, count: usize) -> String {
    let skip = text.chars().count().saturating_sub(count);
    text.chars().skip(skip).collect()
}
