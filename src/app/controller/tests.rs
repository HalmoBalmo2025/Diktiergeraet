//! Abläufe der Steuerung mit einem nachgebauten Mac: Mikrofon,
//! Zwischenablage und Anzeige zeichnen nur auf, was passiert.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use super::*;
use crate::app::source::Origin;
use crate::app::status::{ClipboardLink, Percent};
use crate::app::worker::{PartId, PartKind, TranscriptId};
use crate::audio::chunk::PartTiming;
use crate::audio::record::{Captured, Interrupted, RecordError};
use crate::audio::{self, Pcm16k};
use crate::context::{Cursor, CursorText};
use crate::engine::Unfinished;
use crate::helper::HelperError;
use crate::note::UserText;
use crate::paste::PasteError;
use crate::polish::{PolishError, Polished, Translated, TranslationOutcome, Untranslated};
use crate::text::{Chapter, ChapterHeadings};

/// Eine Sekunde Stille.
fn second() -> Pcm16k {
    Pcm16k::from_samples(vec![0.0; 16_000])
}

/// Der nachgebaute Mac.
#[derive(Default)]
struct FakeMac {
    /// Teile, die das nächste Mikrofon während der Aufnahme liefert.
    parts: RefCell<VecDeque<Pcm16k>>,
    /// Was eingefügt wurde.
    pasted: RefCell<Vec<String>>,
    /// Was in die Zwischenablage gelegt wurde.
    copied: RefCell<Vec<String>>,
    /// Die Meldungen im Panel.
    notes: RefCell<Vec<Note>>,
    /// Bundle-ID der App vorn.
    app: RefCell<String>,
    /// Die angezeigten Pegelstufen.
    levels: RefCell<Vec<Level>>,
    /// Die Antworten auf `check_link`.
    links: RefCell<Vec<ClipboardLink>>,
    /// Wann das nächste Mikrofon ausfällt.
    microphone: Cell<MicrophoneFailure>,
    /// Das Mikrofon lässt sich nicht öffnen.
    opening: Cell<Outcome>,
    /// Was das Lesen der Schreibmarke liefert.
    cursor: RefCell<CursorState>,
    /// Wie oft die Meldung im Panel geleert wurde.
    cleared: Cell<usize>,
    /// Wie lange das Mikrofon schon ungeteilt aufgenommen hat.
    unsent: Cell<Duration>,
    /// Einfügen scheitert.
    paste: Cell<Outcome>,
    /// Kopieren in die Zwischenablage scheitert.
    copy: Cell<Outcome>,
    /// Lesen der Zwischenablage scheitert.
    clipboard: Cell<Outcome>,
    /// Text in der Zwischenablage.
    clipboard_text: RefCell<Option<String>>,
}

/// Wann ein nachgebautes Mikrofon ausfällt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum MicrophoneFailure {
    /// Nie.
    #[default]
    Never,
    /// Sobald die vorbereiteten Teile geliefert sind, beim nächsten Takt.
    AfterTheParts,
    /// Beim Beenden der Aufnahme.
    AtTheEnd,
}

/// Was das nachgebaute Lesen der Schreibmarke liefert.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum CursorState {
    /// Wie in Mail: Die App verrät die Schreibmarke nicht.
    #[default]
    Hidden,
    /// Der Text links und rechts.
    Shows {
        /// Links.
        before: &'static str,
        /// Rechts.
        after: &'static str,
    },
    /// Das Hilfsprogramm antwortet nicht.
    Fails,
}

/// Ob ein nachgebauter Dienst gelingt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Outcome {
    /// Gelingt.
    #[default]
    Works,
    /// Scheitert.
    Fails,
    /// Gelingt halb (beim Einfügen: eingefügt, Zwischenablage nicht
    /// zurückgeholt).
    HalfWorks,
}

impl Platform for FakeMac {
    fn record(&self) -> Result<Box<dyn Microphone>, RecordError> {
        match self.opening.get() {
            Outcome::Works | Outcome::HalfWorks => {}
            Outcome::Fails => return Err(RecordError::NoInputDevice),
        }
        Ok(Box::new(FakeMicrophone {
            parts: self.parts.take(),
            failure: self.microphone.get(),
            unsent: self.unsent.get(),
        }))
    }

    fn paste(&self, text: &str) -> Result<crate::paste::Pasted, PasteError> {
        match self.paste.get() {
            Outcome::Works => {
                self.pasted.borrow_mut().push(text.to_owned());
                Ok(crate::paste::Pasted::Restored)
            }
            Outcome::Fails => Err(PasteError::KeyEvent),
            Outcome::HalfWorks => {
                self.pasted.borrow_mut().push(text.to_owned());
                Ok(crate::paste::Pasted::NotRestored(PasteError::Clipboard(
                    arboard::Error::ClipboardOccupied,
                )))
            }
        }
    }

    fn copy(&self, text: &str) -> Result<(), PasteError> {
        match self.copy.get() {
            Outcome::Works | Outcome::HalfWorks => {
                self.copied.borrow_mut().push(text.to_owned());
                Ok(())
            }
            Outcome::Fails => Err(PasteError::Clipboard(arboard::Error::ClipboardOccupied)),
        }
    }

    fn clipboard_text(&self) -> Result<Option<String>, PasteError> {
        match self.clipboard.get() {
            Outcome::Works | Outcome::HalfWorks => Ok(self.clipboard_text.borrow().clone()),
            Outcome::Fails => Err(PasteError::Clipboard(arboard::Error::ClipboardOccupied)),
        }
    }

    fn clipboard_files(&self) -> Result<Vec<PathBuf>, PasteError> {
        match self.clipboard.get() {
            Outcome::Works | Outcome::HalfWorks => Ok(Vec::new()),
            Outcome::Fails => Err(PasteError::Clipboard(arboard::Error::ClipboardOccupied)),
        }
    }

    fn cursor(&self) -> Result<Cursor, HelperError> {
        let text = match *self.cursor.borrow() {
            CursorState::Hidden => CursorText::Unknown {
                reason: "Test".to_owned(),
            },
            CursorState::Shows { before, after } => CursorText::Known(text::Surroundings {
                before: before.to_owned(),
                after: after.to_owned(),
            }),
            CursorState::Fails => {
                return Err(HelperError::TimedOut(crate::helper::Helper::Context));
            }
        };
        Ok(Cursor {
            app: Some(self.app.borrow().clone()),
            text,
        })
    }

    fn show(&self, update: UiUpdate) {
        match update {
            UiUpdate::Note(note) => self.notes.borrow_mut().push(note),
            UiUpdate::Level(level) => self.levels.borrow_mut().push(level),
            UiUpdate::ClipboardLink(link) => self.links.borrow_mut().push(link),
            UiUpdate::ClearNote => self.cleared.set(self.cleared.get() + 1),
            UiUpdate::Setup(_)
            | UiUpdate::Status(_)
            | UiUpdate::Transcript(_)
            | UiUpdate::History(_)
            | UiUpdate::Autostart(_)
            | UiUpdate::Settings(_)
            | UiUpdate::Dictionary(_)
            | UiUpdate::Quit => {}
        }
    }
}

/// Ein Mikrofon, das vorbereitete Teile liefert und am Ende eine Sekunde;
/// auf Wunsch fällt es aus.
struct FakeMicrophone {
    parts: VecDeque<Pcm16k>,
    failure: MicrophoneFailure,
    unsent: Duration,
}

impl Microphone for FakeMicrophone {
    fn next_part(&mut self, timing: PartTiming) -> Result<Option<Pcm16k>, RecordError> {
        let part = match timing {
            PartTiming::After(_) => self.parts.pop_front(),
            PartTiming::AtEnd => None,
        };
        match (part, self.failure) {
            (None, MicrophoneFailure::AfterTheParts) => Err(RecordError::NoInputDevice),
            (
                part,
                MicrophoneFailure::Never
                | MicrophoneFailure::AfterTheParts
                | MicrophoneFailure::AtTheEnd,
            ) => Ok(part),
        }
    }

    fn level(&self) -> f32 {
        0.01
    }

    fn unsent(&self) -> Duration {
        self.unsent
    }

    fn finish(self: Box<Self>) -> Result<Captured, Interrupted> {
        let captured = Captured {
            pcm: second(),
            device: audio::record::Input::BuiltIn,
            glitches: 0,
        };
        match self.failure {
            MicrophoneFailure::Never => Ok(captured),
            MicrophoneFailure::AfterTheParts | MicrophoneFailure::AtTheEnd => Err(Interrupted {
                error: RecordError::NoInputDevice,
                captured,
            }),
        }
    }
}

/// Eine Steuerung mit nachgebautem Mac und den Ausgängen zu Modell- und
/// Überarbeiten-Thread.
/// Die Einstellungen, mit denen die meisten Tests beginnen.
const fn usual_settings() -> Live {
    Live {
        secondary: Language::English,
        timestamps: Timestamps::Off,
        chapters: ChapterHeadings::On,
        parts: PartTiming::After(audio::chunk::PartSeconds::DEFAULT),
        language: UiLanguage::German,
    }
}

struct Harness {
    controller: Controller<FakeMac>,
    jobs: Receiver<Job>,
    polish: Receiver<PolishRequest>,
    escape: Arc<AtomicBool>,
    _events: Receiver<ControlEvent>,
}

impl Harness {
    /// Das Modell lädt noch.
    fn loading() -> Self {
        let (events, events_receiver) = mpsc::channel();
        let (jobs, jobs_receiver) = mpsc::channel();
        let (polish, polish_receiver) = mpsc::channel();
        let escape = Arc::new(AtomicBool::new(false));
        let controller = Controller::new(
            Connections {
                events,
                jobs,
                polish,
                escape_capture: Arc::clone(&escape),
            },
            Feedback::silent(),
            usual_settings(),
            FakeMac::default(),
        );
        Self {
            controller,
            jobs: jobs_receiver,
            polish: polish_receiver,
            escape,
            _events: events_receiver,
        }
    }

    /// Das Modell ist bereit.
    fn ready() -> Self {
        let mut harness = Self::loading();
        harness.send(ControlEvent::Worker(WorkerEvent::Ready));
        harness
    }

    fn send(&mut self, event: ControlEvent) {
        self.controller.process(event);
    }

    /// Die nächste Aufnahme liefert während des Sprechens einen Teil.
    fn speak_a_part(&self) {
        self.controller
            .platform
            .parts
            .borrow_mut()
            .push_back(second());
    }

    fn tap(&mut self, mode: Mode) {
        self.send(ControlEvent::Hotkey(HotkeyEvent::Tap(Tap {
            slot: Slot::Primary,
            mode,
        })));
    }

    fn escape(&mut self) {
        self.send(ControlEvent::Hotkey(HotkeyEvent::Escape));
    }

    /// Der nächste Teil, der ans Modell ging.
    fn sent_part(&self) -> PartId {
        self.jobs
            .try_iter()
            .find_map(|job| match job {
                Job::Dictation {
                    part,
                    pcm: _,
                    language: _,
                } => Some(part),
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
                | Job::Shutdown => None,
            })
            .expect("kein Teil ans Modell geschickt")
    }

    /// Das Modell hat in `part` den Text `text` erkannt.
    fn recognised(&mut self, part: PartId, text: &str) {
        self.send(ControlEvent::Worker(WorkerEvent::Dictated {
            part,
            result: Ok(Some(text.to_owned())),
        }));
    }

    /// Der nächste Auftrag zum Überarbeiten: Nummer des Diktats und Text.
    fn polish_request(&self) -> (DictationId, String) {
        self.polish
            .try_iter()
            .find_map(|request| match request {
                PolishRequest::Polish {
                    dictation,
                    text,
                    language: _,
                } => Some((dictation, text)),
                PolishRequest::Warmup
                | PolishRequest::UseBackend(_)
                | PolishRequest::Translate(_)
                | PolishRequest::Shutdown => None,
            })
            .expect("nichts zum Überarbeiten geschickt")
    }

    fn polished(&mut self, dictation: DictationId, result: Result<Polished, PolishError>) {
        self.send(ControlEvent::Polished(PolishDone { dictation, result }));
    }

    /// Ein ganzes Diktat mit fn + ⌥ bis zum Überarbeiten; liefert die
    /// Nummer des Diktats.
    fn dictate_for_polishing(&mut self, text: &str) -> DictationId {
        self.tap(Mode::Polish);
        self.tap(Mode::Polish);
        let rest = self.sent_part();
        self.recognised(rest, text);
        let (dictation, sent) = self.polish_request();
        assert_eq!(sent, text);
        dictation
    }

    /// Das Modell scheiterte in `part`, nachdem es `text` erkannte.
    fn failed(&mut self, part: PartId, text: &str) {
        self.send(ControlEvent::Worker(WorkerEvent::Dictated {
            part,
            result: Err(Unfinished {
                text: text.to_owned(),
                error: EngineError::ModelMissing {
                    path: "/x.gguf".to_owned(),
                },
            }),
        }));
    }

    /// Das Modell erkannte in `part` keine Sprache.
    fn silent(&mut self, part: PartId) {
        self.send(ControlEvent::Worker(WorkerEvent::Dictated {
            part,
            result: Ok(None),
        }));
    }

    /// Texte im Verlauf, neuester zuerst.
    fn history(&self) -> Vec<String> {
        self.controller
            .history
            .items()
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    fn pasted(&self) -> Vec<String> {
        self.controller.platform.pasted.borrow().clone()
    }

    fn copied(&self) -> Vec<String> {
        self.controller.platform.copied.borrow().clone()
    }

    fn last_note(&self) -> String {
        self.last_note_in_both().de().to_owned()
    }

    /// Die letzte Meldung in beiden Sprachen.
    fn last_note_in_both(&self) -> Note {
        self.controller
            .platform
            .notes
            .borrow()
            .last()
            .cloned()
            .expect("keine Meldung")
    }

    fn is_ready(&self) -> bool {
        matches!(self.controller.state, State::Ready)
    }

    fn is_recording(&self) -> bool {
        matches!(
            self.controller.state,
            State::Recording {
                recording: _,
                dictation: _
            }
        )
    }

    fn is_polishing(&self) -> bool {
        matches!(self.controller.state, State::Polishing(_))
    }

    fn captures_escape(&self) -> bool {
        self.escape.load(Ordering::Relaxed)
    }
}

#[test]
fn pastes_parts_while_recording_and_the_rest_at_the_end() {
    let mut harness = Harness::ready();
    harness.speak_a_part();
    harness.tap(Mode::Raw);
    assert!(
        harness.is_recording(),
        "Aufnahme läuft nach dem ersten Tippen"
    );
    assert!(
        harness.captures_escape(),
        "Escape wird während der Aufnahme abgefangen"
    );

    harness.send(ControlEvent::Tick);
    let first = harness.sent_part();
    assert_eq!(first.kind, PartKind::Partial);
    harness.recognised(first, "Erster Satz.");
    assert_eq!(harness.pasted(), ["Erster Satz."]);

    harness.tap(Mode::Raw);
    assert!(
        harness.captures_escape(),
        "Escape bricht auch die Verarbeitung ab"
    );
    let rest = harness.sent_part();
    assert_eq!(
        rest,
        PartId {
            dictation: first.dictation,
            kind: PartKind::Last
        }
    );
    // Die App verrät die Schreibmarke nicht: Das Ende des ersten Teils
    // dient als Umgebung, also kommt ein Leerzeichen davor.
    harness.recognised(rest, "Zweiter Satz.");
    assert_eq!(harness.pasted(), ["Erster Satz.", " Zweiter Satz."]);
    assert!(
        harness.is_ready(),
        "nach dem Einfügen des Rests wieder bereit"
    );

    let history = harness.controller.history.items();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].label, "Erster Satz. Zweiter Satz.");
}

#[test]
fn parts_wait_until_the_end_when_set_so() {
    let mut harness = Harness::ready();
    harness.send(ControlEvent::Panel(PanelCommand::Parts(PartTiming::AtEnd)));
    harness.speak_a_part();
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Tick);
    assert!(
        harness.pasted().is_empty(),
        "während der Aufnahme wird nichts eingefügt"
    );
    harness.tap(Mode::Raw);
    let only = harness.sent_part();
    assert_eq!(only.kind, PartKind::Last);
    harness.recognised(only, "Ein ganzer Satz.");
    assert_eq!(harness.pasted(), ["Ein ganzer Satz."]);
}

#[test]
fn escape_discards_the_recording_and_late_results() {
    let mut harness = Harness::ready();
    harness.speak_a_part();
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Tick);
    let part = harness.sent_part();

    harness.escape();
    assert!(harness.is_ready(), "nach Escape sofort wieder bereit");
    assert!(
        !harness.captures_escape(),
        "Escape nach dem Verwerfen wieder freigegeben"
    );

    harness.recognised(part, "Zu spät.");
    assert!(
        harness.pasted().is_empty(),
        "verspätetes Ergebnis einer verworfenen Aufnahme nicht eingefügt"
    );
    assert!(
        harness.is_ready(),
        "verspätetes Ergebnis ändert den Zustand nicht"
    );
}

#[test]
fn records_while_the_model_is_still_loading() {
    let mut harness = Harness::loading();
    harness.tap(Mode::Raw);
    assert!(
        harness.is_recording(),
        "Aufnahme startet, obwohl das Modell noch lädt"
    );

    harness.send(ControlEvent::Worker(WorkerEvent::Ready));
    assert!(
        harness.is_recording(),
        "Aufnahme läuft weiter, wenn das Modell fertig geladen ist"
    );

    harness.tap(Mode::Raw);
    let rest = harness.sent_part();
    harness.recognised(rest, "Hallo.");
    assert_eq!(harness.pasted(), ["Hallo."]);
    assert!(harness.is_ready(), "nach dem Einfügen wieder bereit");
}

#[test]
fn starting_a_recording_warms_up_the_models() {
    let mut harness = Harness::ready();
    harness.tap(Mode::Polish);
    assert!(
        matches!(harness.jobs.try_recv(), Ok(Job::Warmup)),
        "Aufnahmestart weckt das Erkennungsmodell auf"
    );
    assert!(
        matches!(harness.polish.try_recv(), Ok(PolishRequest::Warmup)),
        "Aufnahmestart weckt das Sprachmodell zum Überarbeiten auf"
    );
}

#[test]
fn pastes_only_the_polished_text() {
    let mut harness = Harness::ready();
    let dictation = harness.dictate_for_polishing("ich glaube das es regnet");
    assert!(
        harness.pasted().is_empty(),
        "roher Text vor dem Überarbeiten nicht eingefügt"
    );
    assert!(
        harness.is_polishing(),
        "wartet nach der Aufnahme auf das Überarbeiten"
    );
    assert!(
        harness.captures_escape(),
        "Escape während des Überarbeitens abgefangen"
    );

    harness.polished(
        dictation,
        Ok(Polished {
            text: "Ich glaube, dass es regnet.".to_owned(),
            fallback: None,
        }),
    );
    assert_eq!(harness.pasted(), ["Ich glaube, dass es regnet."]);
    assert!(
        harness.is_ready(),
        "nach dem Einfügen des überarbeiteten Texts wieder bereit"
    );
    assert!(
        !harness.captures_escape(),
        "Escape nach dem Überarbeiten wieder freigegeben"
    );
}

#[test]
fn escape_while_polishing_pastes_the_raw_text() {
    let mut harness = Harness::ready();
    let dictation = harness.dictate_for_polishing("roher Text");

    harness.escape();
    assert_eq!(harness.pasted(), ["roher Text"]);
    assert!(
        harness.is_ready(),
        "nach Escape beim Überarbeiten wieder bereit"
    );
    assert!(
        !harness.captures_escape(),
        "Escape nach dem Abbruch des Überarbeitens wieder freigegeben"
    );

    // Die späte Antwort wird verworfen.
    harness.polished(
        dictation,
        Ok(Polished {
            text: "Überarbeitet.".to_owned(),
            fallback: None,
        }),
    );
    assert_eq!(harness.pasted(), ["roher Text"]);
    assert!(harness.is_ready(), "späte Antwort ändert den Zustand nicht");
}

#[test]
fn failed_polishing_pastes_the_raw_text() {
    let mut harness = Harness::ready();
    let dictation = harness.dictate_for_polishing("roher Text");
    harness.polished(dictation, Err(PolishError::Empty));
    assert_eq!(harness.pasted(), ["roher Text"]);
    assert!(
        harness.last_note().starts_with("Nicht überarbeitet"),
        "Hinweis nennt das gescheiterte Überarbeiten"
    );
    assert!(
        harness.is_ready(),
        "nach dem gescheiterten Überarbeiten wieder bereit"
    );
}

#[test]
fn open_panel_copies_instead_of_pasting() {
    let mut harness = Harness::ready();
    harness.send(ControlEvent::Panel(PanelCommand::Visibility(
        PanelState::Open,
    )));
    harness.tap(Mode::Raw);
    harness.tap(Mode::Raw);
    let rest = harness.sent_part();
    harness.recognised(rest, "Notiz.");
    assert!(
        harness.pasted().is_empty(),
        "bei offenem Panel wird nichts eingefügt"
    );
    assert_eq!(harness.copied(), ["Notiz."]);
}

#[test]
fn rejects_a_dropped_folder_and_an_empty_clipboard() {
    let mut harness = Harness::ready();
    harness.send(ControlEvent::Panel(PanelCommand::File {
        language: Language::German,
        paths: vec![std::env::temp_dir()],
        range: RangeInput::default(),
    }));
    assert!(
        harness.last_note().ends_with("ist keine Datei."),
        "Hinweis: ein Ordner ist keine Datei"
    );
    harness.send(ControlEvent::Panel(PanelCommand::File {
        language: Language::German,
        paths: Vec::new(),
        range: RangeInput::default(),
    }));
    assert!(
        harness.last_note().starts_with("Keine Datei"),
        "Hinweis: leere Zwischenablage enthält keine Datei"
    );
    assert!(
        matches!(harness.controller.transcript, TranscriptJob::Idle),
        "ohne gültige Datei startet kein Transkript"
    );
}

#[test]
fn chats_get_no_final_period_and_mail_full_sentences() {
    let mut harness = Harness::ready();
    *harness.controller.platform.app.borrow_mut() = "com.apple.MobileSMS".to_owned();
    harness.speak_a_part();
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Tick);
    let first = harness.sent_part();
    harness.recognised(first, "Bin gleich da.");
    harness.tap(Mode::Raw);
    let rest = harness.sent_part();
    harness.recognised(rest, "Und bringe Brot mit.");
    assert_eq!(harness.pasted(), ["Bin gleich da", " und bringe Brot mit"]);

    *harness.controller.platform.app.borrow_mut() = "com.apple.mail".to_owned();
    harness.tap(Mode::Raw);
    harness.tap(Mode::Raw);
    let mail = harness.sent_part();
    harness.recognised(mail, "Viele Grüße.");
    assert_eq!(harness.pasted()[2], "Viele Grüße.");
}

#[test]
fn shows_the_level_only_while_recording_and_when_it_changes() {
    let mut harness = Harness::ready();
    harness.send(ControlEvent::Meter);
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Meter);
    harness.send(ControlEvent::Meter);
    // Das nachgebaute Mikrofon liefert −40 dB: Stufe 3, nur einmal gemeldet.
    assert_eq!(
        *harness.controller.platform.levels.borrow(),
        [audio::meter(0.01)]
    );
    assert!(
        harness.is_recording(),
        "Pegelmeldungen beenden die Aufnahme nicht"
    );
}

#[test]
fn queues_several_files_and_cancel_clears_the_queue() {
    let dir = crate::testing::TempDir::new("queue");
    let files: Vec<PathBuf> = ["Eins.m4a", "Zwei.m4a", "Drei.m4a"]
        .iter()
        .map(|name| dir.file(name, "kein Ton"))
        .collect();
    let mut harness = Harness::ready();
    harness.send(ControlEvent::Panel(PanelCommand::File {
        language: Language::German,
        paths: files[..2].to_vec(),
        range: RangeInput::default(),
    }));
    assert!(
        matches!(
            harness.controller.transcript,
            TranscriptJob::Downloading {
                origin: _,
                title: _,
                cancel: _,
                thread: _,
                translate: _
            }
        ),
        "erste Datei wird geladen"
    );
    assert_eq!(harness.controller.waiting.len(), 1);
    assert!(
        harness
            .last_note()
            .contains("wartet – 1 in der Warteschlange"),
        "Hinweis nennt die zweite Datei als wartend"
    );

    // Die erste Datei lässt sich nicht lesen: Die zweite kommt dran.
    harness.send(ControlEvent::SourceLoaded(Err(LoadError::NoAudio(
        UserText::new("Eins"),
    ))));
    assert!(
        matches!(
            &harness.controller.transcript,
            TranscriptJob::Downloading { title: Some(title), origin: _, cancel: _, thread: _, translate: _ } if title.shown() == "Zwei"
        ),
        "nach dem Fehler der ersten Datei wird die zweite geladen"
    );
    assert!(
        harness.controller.waiting.is_empty(),
        "Warteschlange nach dem Nachrücken leer"
    );

    harness.send(ControlEvent::Panel(PanelCommand::File {
        language: Language::German,
        paths: files[2..].to_vec(),
        range: RangeInput::default(),
    }));
    assert_eq!(harness.controller.waiting.len(), 1);
    harness.send(ControlEvent::Panel(PanelCommand::CancelTranscript));
    assert!(
        harness.controller.waiting.is_empty(),
        "Abbrechen leert die Warteschlange"
    );
    assert!(
        harness.last_note().contains("1 wartende verworfen"),
        "Hinweis nennt die verworfene wartende Datei"
    );
}

#[test]
fn a_range_needs_a_single_file_and_a_readable_time() {
    let dir = crate::testing::TempDir::new("range");
    let files: Vec<PathBuf> = ["Eins.m4a", "Zwei.m4a"]
        .iter()
        .map(|name| dir.file(name, "kein Ton"))
        .collect();
    let range = |from: &str, to: &str| RangeInput {
        from: from.to_owned(),
        to: to.to_owned(),
    };
    let mut harness = Harness::ready();

    harness.send(ControlEvent::Panel(PanelCommand::File {
        language: Language::German,
        paths: files.clone(),
        range: range("12:00", "20:00"),
    }));
    assert!(
        harness
            .last_note()
            .starts_with("Ein Bereich geht nur bei einer einzelnen Datei"),
        "Hinweis: Bereich nur bei einer einzelnen Datei"
    );
    assert!(
        matches!(harness.controller.transcript, TranscriptJob::Idle),
        "mehrere Dateien mit Bereich starten kein Transkript"
    );

    harness.send(ControlEvent::Panel(PanelCommand::File {
        language: Language::German,
        paths: files[..1].to_vec(),
        range: range("12:x", ""),
    }));
    assert_eq!(
        harness.last_note(),
        "Bereich: „12:x“ ist keine Zeit – so schreiben: 12:30 oder 1:02:05"
    );
    assert!(
        matches!(harness.controller.transcript, TranscriptJob::Idle),
        "unlesbare Zeit startet kein Transkript"
    );

    harness.send(ControlEvent::Panel(PanelCommand::File {
        language: Language::German,
        paths: files[..1].to_vec(),
        range: range("12:00", "20:00"),
    }));
    assert!(
        matches!(
            &harness.controller.transcript,
            TranscriptJob::Downloading { title: Some(title), origin: _, cancel: _, thread: _, translate: _ } if title.shown() == "Eins (12:00–20:00)"
        ),
        "einzelne Datei mit gültigem Bereich wird geladen, Bereich im Titel"
    );
}

/// Ein spanisches Video, fertig transkribiert, das ins Deutsche soll.
fn transcribed_spanish_video(harness: &mut Harness, translate: Option<Language>) {
    harness.controller.transcript = TranscriptJob::Transcribing {
        id: TranscriptId(0),
        origin: Origin::Youtube,
        cancel: Arc::new(AtomicBool::new(false)),
        title: UserText::new("Hola"),
        percent: Percent::of(100, 100),
        language: Language::Spanish,
        translate,
    };
    harness.send(ControlEvent::Worker(transcript_finished(
        TranscriptOutcome::Finished("[0:05] Hola a todos.".to_owned()),
    )));
}

/// Der Auftrag zum Übersetzen, falls einer geschickt wurde.
fn translation_request(harness: &Harness) -> Option<crate::polish::TranslationRequest> {
    harness.polish.try_iter().find_map(|request| match request {
        PolishRequest::Translate(request) => Some(request),
        PolishRequest::Warmup
        | PolishRequest::UseBackend(_)
        | PolishRequest::Polish {
            dictation: _,
            text: _,
            language: _,
        }
        | PolishRequest::Shutdown => None,
    })
}

#[test]
fn translates_a_finished_transcript_before_copying_it() {
    let mut harness = Harness::ready();
    transcribed_spanish_video(&mut harness, Some(Language::German));
    let request = translation_request(&harness).expect("keine Übersetzung beauftragt");
    assert_eq!(
        (request.from, request.to, request.text.as_str()),
        (Language::Spanish, Language::German, "[0:05] Hola a todos.")
    );
    assert!(
        harness.copied().is_empty(),
        "vor der Übersetzung nichts kopiert"
    );
    assert!(
        harness.controller.status()
            == Status::Translating {
                percent: Percent::of(0, 100)
            },
        "Übersetzung beginnt bei 0 %"
    );

    harness.send(ControlEvent::Translation(TranslationEvent::Progress {
        done: 1,
        total: 2,
    }));
    assert!(
        harness.controller.status()
            == Status::Translating {
                percent: Percent::of(50, 100)
            },
        "nach einem von zwei Abschnitten bei 50 %"
    );

    harness.send(translation_done(Translated {
        title: "Hallo".to_owned(),
        text: "[0:05] Hallo zusammen.".to_owned(),
        total: 2,
        untranslated: None,
        fallback: None,
    }));
    assert_eq!(
        harness.copied(),
        ["Hallo (aus dem Spanischen)\n\n[0:05] Hallo zusammen."]
    );
    assert!(
        harness
            .last_note()
            .starts_with("Ins Deutsche übersetzt und kopiert"),
        "Hinweis meldet Übersetzung und Kopieren"
    );
    assert!(
        matches!(harness.controller.transcript, TranscriptJob::Idle),
        "nach dem Kopieren der Übersetzung kein Transkript mehr aktiv"
    );
}

#[test]
fn a_video_already_in_the_target_language_is_not_translated() {
    let mut harness = Harness::ready();
    transcribed_spanish_video(&mut harness, Some(Language::Spanish));
    assert!(
        translation_request(&harness).is_none(),
        "Video schon in der Zielsprache: keine Übersetzung beauftragt"
    );
    assert_eq!(harness.copied(), ["Hola\n\n[0:05] Hola a todos."]);
}

#[test]
fn a_cancelled_or_failed_translation_copies_the_original() {
    let mut harness = Harness::ready();
    transcribed_spanish_video(&mut harness, Some(Language::German));
    harness.send(ControlEvent::Panel(PanelCommand::CancelTranscript));
    let request = translation_request(&harness).unwrap();
    assert!(
        request.cancel.load(Ordering::Relaxed),
        "Abbrechen setzt das Abbruchsignal der Übersetzung"
    );
    harness.send(ControlEvent::Translation(TranslationEvent::Done(
        TranslationOutcome::Cancelled,
    )));
    assert_eq!(harness.copied(), ["Hola\n\n[0:05] Hola a todos."]);
    assert!(
        harness
            .last_note()
            .starts_with("Übersetzung abgebrochen, Original kopiert"),
        "Hinweis meldet Abbruch und kopiertes Original"
    );

    transcribed_spanish_video(&mut harness, Some(Language::German));
    harness.send(translation_done(Translated {
        title: "Hola".to_owned(),
        text: "[0:05] Hola a todos.".to_owned(),
        total: 2,
        untranslated: Some(Untranslated {
            count: 2,
            first_error: PolishError::Empty,
        }),
        fallback: None,
    }));
    assert!(
        harness.last_note().starts_with(
            "Nicht übersetzt (das Sprachmodell lieferte keinen Text), Original kopiert"
        ),
        "Hinweis nennt den Grund und das kopierte Original"
    );
}

#[test]
fn a_translation_result_without_a_running_translation_is_dropped() {
    let mut harness = Harness::ready();
    harness.send(translation_done(Translated {
        title: "Hallo".to_owned(),
        text: "Spät.".to_owned(),
        total: 1,
        untranslated: None,
        fallback: None,
    }));
    assert!(
        harness.copied().is_empty(),
        "Ergebnis ohne laufende Übersetzung nicht kopiert"
    );
    assert!(
        matches!(harness.controller.transcript, TranscriptJob::Idle),
        "Ergebnis ohne laufende Übersetzung lässt das Transkript ruhen"
    );
}

/// Eine fertige Übersetzung vom Überarbeiten-Thread.
fn translation_done(translated: Translated) -> ControlEvent {
    ControlEvent::Translation(TranslationEvent::Done(TranslationOutcome::Finished(
        Box::new(translated),
    )))
}

#[test]
fn shutdown_ends_a_recording_and_cancels_the_transcript() {
    let mut harness = Harness::ready();
    let cancel = Arc::new(AtomicBool::new(false));
    harness.controller.transcript = TranscriptJob::Transcribing {
        id: TranscriptId(0),
        origin: Origin::Youtube,
        cancel: Arc::clone(&cancel),
        title: UserText::new("Hola"),
        percent: Percent::of(40, 100),
        language: Language::Spanish,
        translate: None,
    };
    harness.speak_a_part();
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Tick);
    let part = harness.sent_part();

    harness.send(ControlEvent::Shutdown);
    assert!(
        matches!(harness.controller.state, State::Unavailable { reason: _ }),
        "nach dem Beenden nimmt die Steuerung nichts mehr an"
    );
    assert!(
        !harness.captures_escape(),
        "Escape nach dem Beenden wieder freigegeben"
    );
    assert!(
        cancel.load(Ordering::Relaxed),
        "das laufende Transkript wird abgebrochen"
    );
    harness.recognised(part, "Zu spät.");
    assert!(
        harness.pasted().is_empty(),
        "nach dem Beenden wird nichts mehr eingefügt"
    );
}

#[test]
fn run_ends_with_shutdown() {
    let (events, receiver) = mpsc::channel();
    let (jobs, _jobs) = mpsc::channel();
    let (polish, _polish) = mpsc::channel();
    let controller = Controller::new(
        Connections {
            events: events.clone(),
            jobs,
            polish,
            escape_capture: Arc::new(AtomicBool::new(false)),
        },
        Feedback::silent(),
        usual_settings(),
        FakeMac::default(),
    );
    events.send(ControlEvent::Tick).unwrap();
    events.send(ControlEvent::Shutdown).unwrap();
    // Endete `run` nicht, hinge der Test: Die Steuerung hält selbst einen
    // Sender auf ihren Eingang.
    controller.run(&receiver);
}

#[test]
fn without_the_polish_thread_the_raw_text_is_pasted() {
    let mut harness = Harness::ready();
    // Der Überarbeiten-Thread ist weg.
    drop(std::mem::replace(&mut harness.polish, mpsc::channel().1));
    harness.tap(Mode::Polish);
    harness.tap(Mode::Polish);
    let rest = harness.sent_part();
    harness.recognised(rest, "roher Text");
    assert_eq!(harness.pasted(), ["roher Text"]);
    assert!(harness.is_ready(), "nach dem Einfügen wieder bereit");
    assert!(
        !harness.captures_escape(),
        "Escape ohne Überarbeiten nicht abgefangen"
    );
}

#[test]
fn without_the_model_thread_escape_is_released() {
    let mut harness = Harness::ready();
    harness.speak_a_part();
    harness.tap(Mode::Raw);
    assert!(
        harness.captures_escape(),
        "Escape während der Aufnahme abgefangen"
    );
    // Der Modell-Thread ist weg, bevor der erste Teil ankommt.
    drop(std::mem::replace(&mut harness.jobs, mpsc::channel().1));
    harness.send(ControlEvent::Tick);
    assert!(
        matches!(harness.controller.state, State::Unavailable { reason: _ }),
        "ohne Modell-Thread nicht mehr bereit"
    );
    assert!(
        !harness.captures_escape(),
        "Escape nach dem Ende der Aufnahme wieder freigegeben"
    );
}

#[test]
fn a_transcript_result_while_loading_is_dropped() {
    let mut harness = Harness::ready();
    let thread = std::thread::Builder::new().spawn(|| {}).unwrap();
    harness.controller.transcript = TranscriptJob::Downloading {
        origin: Origin::Youtube,
        title: None,
        cancel: Arc::new(AtomicBool::new(false)),
        thread,
        translate: None,
    };
    harness.send(ControlEvent::Worker(transcript_finished(
        TranscriptOutcome::Finished("fremd".to_owned()),
    )));
    assert!(
        matches!(
            harness.controller.transcript,
            TranscriptJob::Downloading {
                origin: _,
                title: _,
                cancel: _,
                thread: _,
                translate: _
            }
        ),
        "ein fremdes Ergebnis beendet das Laden nicht"
    );
    assert!(
        harness.copied().is_empty(),
        "fremdes Ergebnis nicht kopiert"
    );
}

#[test]
fn a_microphone_failing_at_the_end_keeps_the_recognised_text() {
    let mut harness = Harness::ready();
    harness
        .controller
        .platform
        .microphone
        .set(MicrophoneFailure::AtTheEnd);
    harness.speak_a_part();
    harness.tap(Mode::Polish);
    harness.send(ControlEvent::Tick);
    let first = harness.sent_part();
    harness.recognised(first, "Erster Satz.");
    // Das Mikrofon fällt beim Beenden aus; der Rest geht trotzdem ans Modell.
    harness.tap(Mode::Polish);
    let rest = harness.sent_part();
    assert_eq!(rest.kind, PartKind::Last);
    assert!(
        harness.last_note().starts_with("Aufnahme fehlgeschlagen"),
        "Meldung: {}",
        harness.last_note()
    );
    harness.silent(rest);
    let (dictation, text) = harness.polish_request();
    assert_eq!(text, "Erster Satz.");
    harness.polished(
        dictation,
        Ok(Polished {
            text: "Erster Satz!".to_owned(),
            fallback: None,
        }),
    );
    assert_eq!(harness.pasted(), ["Erster Satz!"]);
    assert_eq!(harness.history(), ["Erster Satz!"]);
    assert!(
        harness.last_note().starts_with("Aufnahme fehlgeschlagen"),
        "der Ausfall bleibt sichtbar: {}",
        harness.last_note()
    );
}

#[test]
fn a_microphone_failing_while_recording_finishes_the_dictation() {
    let mut harness = Harness::ready();
    harness
        .controller
        .platform
        .microphone
        .set(MicrophoneFailure::AfterTheParts);
    harness.speak_a_part();
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Tick);
    let first = harness.sent_part();
    harness.recognised(first, "Erster Satz.");
    // Der nächste Takt findet das Mikrofon ausgefallen.
    harness.send(ControlEvent::Tick);
    assert!(
        !harness.is_recording(),
        "nach dem Ausfall läuft keine Aufnahme mehr"
    );
    assert!(
        harness.captures_escape(),
        "Escape bricht auch die Verarbeitung des Rests ab"
    );
    let rest = harness.sent_part();
    assert_eq!(rest.kind, PartKind::Last, "der Rest geht ans Modell");
    harness.recognised(rest, "Zweiter Satz.");
    assert_eq!(harness.pasted(), ["Erster Satz.", " Zweiter Satz."]);
    assert_eq!(harness.history(), ["Erster Satz. Zweiter Satz."]);
    assert!(harness.is_ready(), "danach wieder bereit");
    assert!(
        harness.last_note().starts_with("Aufnahme fehlgeschlagen"),
        "Meldung: {}",
        harness.last_note()
    );
}

#[test]
fn a_failed_paste_is_not_reported_as_inserted() {
    let mut harness = Harness::ready();
    harness.controller.platform.paste.set(Outcome::Fails);
    harness.tap(Mode::Raw);
    harness.tap(Mode::Raw);
    let rest = harness.sent_part();
    harness.recognised(rest, "Ein Satz.");
    assert!(harness.pasted().is_empty(), "nichts eingefügt");
    assert!(
        harness.last_note().starts_with("Einfügen fehlgeschlagen"),
        "Meldung: {}",
        harness.last_note()
    );
    assert!(
        !harness.last_note().contains("eingefügt."),
        "Meldung: {}",
        harness.last_note()
    );
    assert_eq!(
        harness.history(),
        ["Ein Satz."],
        "der Text steht im Verlauf"
    );
}

#[test]
fn a_failed_polish_with_a_failed_paste_says_so() {
    let mut harness = Harness::ready();
    let dictation = harness.dictate_for_polishing("roher Text");
    harness.controller.platform.paste.set(Outcome::Fails);
    harness.polished(dictation, Err(PolishError::Empty));
    let note = harness.last_note();
    assert!(note.starts_with("Nicht überarbeitet"), "Meldung: {note}");
    assert!(note.contains("Einfügen fehlgeschlagen"), "Meldung: {note}");
    assert!(!note.contains("eingefügt."), "Meldung: {note}");
}

#[test]
fn a_failed_part_stays_in_the_note() {
    let mut harness = Harness::ready();
    harness.speak_a_part();
    harness.speak_a_part();
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Tick);
    let first = harness.sent_part();
    harness.failed(first, "");
    assert!(
        harness
            .last_note()
            .starts_with("Transkription fehlgeschlagen"),
        "„bisher“ ersetzt den Fehler nicht: {}",
        harness.last_note()
    );
    harness.send(ControlEvent::Tick);
    let second = harness.sent_part();
    harness.recognised(second, "Zweiter Satz.");
    assert!(
        harness
            .last_note()
            .starts_with("Transkription fehlgeschlagen"),
        "auch der nächste Teil nicht: {}",
        harness.last_note()
    );
    harness.tap(Mode::Raw);
    let rest = harness.sent_part();
    harness.silent(rest);
    assert!(
        harness
            .last_note()
            .starts_with("Transkription fehlgeschlagen"),
        "auch das Ende nicht: {}",
        harness.last_note()
    );
}

#[test]
fn a_part_failing_midway_keeps_what_it_recognised() {
    let mut harness = Harness::ready();
    harness.tap(Mode::Raw);
    harness.tap(Mode::Raw);
    let rest = harness.sent_part();
    harness.failed(rest, "Halber Satz.");
    assert_eq!(harness.pasted(), ["Halber Satz."]);
    assert_eq!(harness.history(), ["Halber Satz."]);
}

#[test]
fn escape_after_a_failed_paste_keeps_the_text_in_the_history() {
    let mut harness = Harness::ready();
    harness.controller.platform.paste.set(Outcome::Fails);
    harness.speak_a_part();
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Tick);
    let first = harness.sent_part();
    harness.recognised(first, "Erster Satz.");
    assert!(
        harness.last_note().contains("Text steht im Verlauf"),
        "Meldung: {}",
        harness.last_note()
    );
    harness.escape();
    assert_eq!(harness.history(), ["Erster Satz."]);
    assert!(harness.is_ready(), "nach Escape bereit");
}

#[test]
fn a_lost_model_thread_delivers_the_text_already_recognised() {
    let mut harness = Harness::ready();
    harness.speak_a_part();
    harness.speak_a_part();
    harness.tap(Mode::Polish);
    harness.send(ControlEvent::Tick);
    let first = harness.sent_part();
    harness.recognised(first, "Erster Satz.");
    // Der Modell-Thread ist weg, bevor der zweite Teil hingeht.
    drop(std::mem::replace(&mut harness.jobs, mpsc::channel().1));
    harness.send(ControlEvent::Tick);
    assert!(
        matches!(harness.controller.state, State::Unavailable { reason: _ }),
        "ohne Modell-Thread nicht mehr bereit"
    );
    assert_eq!(harness.pasted(), ["Erster Satz."], "roh eingefügt");
    assert_eq!(harness.history(), ["Erster Satz."]);
    assert!(
        harness.last_note().contains("Modell-Thread ist beendet"),
        "Meldung: {}",
        harness.last_note()
    );
}

#[test]
fn a_model_that_fails_to_load_ends_the_dictation_with_what_is_there() {
    let mut harness = Harness::loading();
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Worker(WorkerEvent::LoadFailed(
        EngineError::ModelMissing {
            path: "/x.gguf".to_owned(),
        },
    )));
    assert!(
        matches!(harness.controller.state, State::Unavailable { reason: _ }),
        "ohne Modell nicht verfügbar"
    );
    assert!(
        !harness.captures_escape(),
        "Escape nach dem Ende der Aufnahme freigegeben"
    );
    assert!(
        harness.last_note().starts_with("Modelldatei fehlt"),
        "Meldung: {}",
        harness.last_note()
    );
}

#[test]
fn a_failed_transcript_copies_the_part_so_far() {
    let mut harness = Harness::ready();
    harness.controller.transcript = TranscriptJob::Transcribing {
        id: TranscriptId(0),
        origin: Origin::Youtube,
        cancel: Arc::new(AtomicBool::new(false)),
        title: UserText::new("Hola"),
        percent: Percent::of(50, 100),
        language: Language::Spanish,
        translate: None,
    };
    harness.send(ControlEvent::Worker(transcript_finished(
        TranscriptOutcome::Failed {
            partial: "[0:05] Hola a todos.".to_owned(),
            error: EngineError::ModelMissing {
                path: "/x.gguf".to_owned(),
            },
        },
    )));
    assert_eq!(harness.copied(), ["Hola\n\n[0:05] Hola a todos."]);
    assert!(
        harness
            .last_note()
            .starts_with("Transkription fehlgeschlagen"),
        "Meldung: {}",
        harness.last_note()
    );
}

#[test]
fn escape_while_the_model_loads_keeps_showing_loading() {
    let mut harness = Harness::loading();
    harness.tap(Mode::Raw);
    harness.escape();
    assert!(
        matches!(harness.controller.status(), Status::Loading),
        "das Modell lädt noch"
    );
    harness.send(ControlEvent::Panel(PanelCommand::File {
        language: Language::German,
        paths: vec![PathBuf::from("/gibt/es/nicht.m4a")],
        range: RangeInput::default(),
    }));
    assert!(
        matches!(harness.controller.transcript, TranscriptJob::Idle),
        "kein Transkript, solange das Modell lädt"
    );
}

#[test]
fn a_transcript_is_refused_while_the_model_loads_even_during_a_dictation() {
    let dir = crate::testing::TempDir::new("loading");
    let file = dir.file("Vorlesung.m4a", "kein Ton");
    let mut harness = Harness::loading();
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Panel(PanelCommand::File {
        language: Language::German,
        paths: vec![file],
        range: RangeInput::default(),
    }));
    drop(dir);
    assert!(
        matches!(harness.controller.transcript, TranscriptJob::Idle),
        "kein Transkript, solange das Modell lädt"
    );
    assert_eq!(harness.last_note(), "Das Modell ist noch nicht bereit.");
}

#[test]
fn a_model_that_fails_to_load_ends_the_running_transcript() {
    let mut harness = Harness::ready();
    harness.controller.transcript = TranscriptJob::Transcribing {
        id: TranscriptId(0),
        origin: Origin::Youtube,
        cancel: Arc::new(AtomicBool::new(false)),
        title: UserText::new("Hola"),
        percent: Percent::of(0, 100),
        language: Language::Spanish,
        translate: None,
    };
    harness.send(ControlEvent::Worker(WorkerEvent::LoadFailed(
        EngineError::ModelMissing {
            path: "/x.gguf".to_owned(),
        },
    )));
    assert!(
        matches!(harness.controller.transcript, TranscriptJob::Idle),
        "das Transkript wartet nicht auf ein Modell, das nie kommt"
    );
}

#[test]
fn japanese_parts_are_joined_without_a_space() {
    let mut harness = Harness::ready();
    harness.send(ControlEvent::Panel(PanelCommand::SecondaryLanguage(
        Language::Japanese,
    )));
    harness.send(ControlEvent::Panel(PanelCommand::Visibility(
        PanelState::Open,
    )));
    harness.speak_a_part();
    let tap = |controlled: &mut Harness| {
        controlled.send(ControlEvent::Hotkey(HotkeyEvent::Tap(Tap {
            slot: Slot::Secondary,
            mode: Mode::Raw,
        })));
    };
    tap(&mut harness);
    harness.send(ControlEvent::Tick);
    let first = harness.sent_part();
    harness.recognised(first, "今日は");
    tap(&mut harness);
    let rest = harness.sent_part();
    harness.recognised(rest, "雨です。");
    assert_eq!(harness.copied(), ["今日は雨です。"]);
}

#[test]
fn escape_in_polish_mode_does_not_claim_inserted_text() {
    let mut harness = Harness::ready();
    harness.speak_a_part();
    harness.tap(Mode::Polish);
    harness.send(ControlEvent::Tick);
    let first = harness.sent_part();
    harness.recognised(first, "Erster Satz.");
    harness.escape();
    assert!(harness.pasted().is_empty(), "nichts eingefügt");
    assert!(
        !harness.last_note().contains("eingefügter"),
        "Meldung: {}",
        harness.last_note()
    );
}

#[test]
fn a_load_error_after_cancel_counts_as_cancelled() {
    let mut harness = Harness::ready();
    let thread = std::thread::Builder::new().spawn(|| {}).unwrap();
    harness.controller.transcript = TranscriptJob::Downloading {
        origin: Origin::Youtube,
        title: None,
        cancel: Arc::new(AtomicBool::new(true)),
        thread,
        translate: None,
    };
    harness.send(ControlEvent::SourceLoaded(Err(LoadError::NoAudio(
        UserText::new("Video"),
    ))));
    assert!(
        harness.last_note().ends_with("abgebrochen."),
        "Meldung: {}",
        harness.last_note()
    );
}

#[test]
fn closing_the_panel_during_a_dictation_delivers_every_part() {
    let mut harness = Harness::ready();
    harness.send(ControlEvent::Panel(PanelCommand::Visibility(
        PanelState::Open,
    )));
    harness.speak_a_part();
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Tick);
    let first = harness.sent_part();
    harness.recognised(first, "Erster Satz.");
    harness.send(ControlEvent::Panel(PanelCommand::Visibility(
        PanelState::Closed,
    )));
    harness.tap(Mode::Raw);
    let rest = harness.sent_part();
    harness.recognised(rest, "Zweiter Satz.");
    assert!(harness.pasted().is_empty(), "nichts an die Schreibmarke");
    assert_eq!(harness.copied(), ["Erster Satz. Zweiter Satz."]);
}

#[test]
fn opening_the_panel_during_a_dictation_copies_no_part_twice() {
    let mut harness = Harness::ready();
    harness.speak_a_part();
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Tick);
    let first = harness.sent_part();
    harness.recognised(first, "Erster Satz.");
    harness.send(ControlEvent::Panel(PanelCommand::Visibility(
        PanelState::Open,
    )));
    harness.tap(Mode::Raw);
    let rest = harness.sent_part();
    harness.recognised(rest, "Zweiter Satz.");
    assert_eq!(harness.pasted(), ["Erster Satz.", " Zweiter Satz."]);
    assert!(
        harness.copied().is_empty(),
        "nichts doppelt in der Zwischenablage"
    );
}

#[test]
fn opening_the_panel_while_polishing_still_pastes() {
    let mut harness = Harness::ready();
    let dictation = harness.dictate_for_polishing("roher Text");
    harness.send(ControlEvent::Panel(PanelCommand::Visibility(
        PanelState::Open,
    )));
    harness.polished(
        dictation,
        Ok(Polished {
            text: "Roher Text.".to_owned(),
            fallback: None,
        }),
    );
    assert_eq!(harness.pasted(), ["Roher Text."]);
    assert!(harness.copied().is_empty(), "nicht zusätzlich kopiert");
}

#[test]
fn escape_while_processing_discards_the_dictation() {
    let mut harness = Harness::ready();
    harness.speak_a_part();
    harness.tap(Mode::Polish);
    harness.send(ControlEvent::Tick);
    let first = harness.sent_part();
    harness.recognised(first, "Erster Satz.");
    harness.tap(Mode::Polish);
    let rest = harness.sent_part();
    assert!(harness.captures_escape(), "Esc gehört der Verarbeitung");
    harness.escape();
    assert!(harness.is_ready(), "nach Esc sofort bereit");
    assert!(!harness.captures_escape(), "Esc wieder frei");
    assert_eq!(harness.history(), ["Erster Satz."]);
    harness.recognised(rest, "Zu spät.");
    assert!(harness.pasted().is_empty(), "späte Antwort nicht eingefügt");
    assert!(harness.is_ready(), "späte Antwort ändert nichts");
}

#[test]
fn shutdown_keeps_the_text_of_a_running_dictation() {
    let mut harness = Harness::ready();
    let _dictation = harness.dictate_for_polishing("roher Text");
    harness.send(ControlEvent::Shutdown);
    assert_eq!(harness.history(), ["roher Text"]);
    assert_eq!(harness.copied(), ["roher Text"]);
}

#[test]
fn describes_events_for_the_log() {
    assert_eq!(describe_event(&ControlEvent::Tick), "Takt");
    let failed = ControlEvent::Worker(WorkerEvent::Dictated {
        part: PartId {
            dictation: DictationId::FIRST,
            kind: PartKind::Last,
        },
        result: Err(Unfinished {
            text: "Halber Satz.".to_owned(),
            error: EngineError::ModelMissing {
                path: "/x.gguf".to_owned(),
            },
        }),
    });
    assert_eq!(
        describe_event(&failed),
        "Transkriptionsfehler: Modelldatei fehlt: /x.gguf (12 Zeichen davor erkannt)"
    );
    let transcript = ControlEvent::Worker(transcript_finished(TranscriptOutcome::Failed {
        partial: "Hola a todos.".to_owned(),
        error: EngineError::ModelMissing {
            path: "/x.gguf".to_owned(),
        },
    }));
    assert_eq!(
        describe_event(&transcript),
        "Transkript-Fehler: Modelldatei fehlt: /x.gguf (3 Wörter davor)"
    );
}

#[test]
fn a_loaded_source_goes_to_the_model() {
    let mut harness = Harness::ready();
    let thread = std::thread::Builder::new().spawn(|| {}).unwrap();
    harness.controller.transcript = TranscriptJob::Downloading {
        origin: Origin::Youtube,
        title: None,
        cancel: Arc::new(AtomicBool::new(false)),
        thread,
        translate: None,
    };
    harness.send(ControlEvent::SourceLoaded(Ok(Loaded {
        title: UserText::new("Vorlesung"),
        language: Language::German,
        pcm: second(),
        begin: Duration::ZERO,
        chapters: Vec::new(),
    })));
    assert!(
        matches!(
            harness.controller.transcript,
            TranscriptJob::Transcribing {
                id: _,
                origin: _,
                cancel: _,
                title: _,
                percent: _,
                language: _,
                translate: _
            }
        ),
        "die Tonspur geht ans Modell"
    );
}

#[test]
fn chapter_headings_follow_the_setting() {
    for (setting, expected) in [(ChapterHeadings::On, 1), (ChapterHeadings::Off, 0)] {
        let mut harness = Harness::ready();
        harness.send(ControlEvent::Panel(PanelCommand::Chapters(setting)));
        let thread = std::thread::Builder::new().spawn(|| {}).unwrap();
        harness.controller.transcript = TranscriptJob::Downloading {
            origin: Origin::Youtube,
            title: None,
            cancel: Arc::new(AtomicBool::new(false)),
            thread,
            translate: None,
        };
        harness.send(ControlEvent::SourceLoaded(Ok(Loaded {
            title: UserText::new("Vorlesung"),
            language: Language::German,
            pcm: second(),
            begin: Duration::ZERO,
            chapters: vec![Chapter {
                start: Duration::ZERO,
                title: "Einleitung".to_owned(),
            }],
        })));
        let chapters: Vec<usize> = harness
            .jobs
            .try_iter()
            .filter_map(|job| match job {
                Job::Transcript {
                    chapters,
                    id: _,
                    pcm: _,
                    language: _,
                    begin: _,
                    timestamps: _,
                    cancel: _,
                } => Some(chapters.len()),
                Job::Dictation {
                    part: _,
                    pcm: _,
                    language: _,
                }
                | Job::Discard { dictation: _ }
                | Job::Warmup
                | Job::SetUnload(_)
                | Job::Shutdown => None,
            })
            .collect();
        assert_eq!(chapters, [expected], "{setting:?}");
    }
}

#[test]
fn a_transcript_failing_without_text_copies_nothing() {
    let mut harness = Harness::ready();
    harness.controller.transcript = TranscriptJob::Transcribing {
        id: TranscriptId(0),
        origin: Origin::Youtube,
        cancel: Arc::new(AtomicBool::new(false)),
        title: UserText::new("Hola"),
        percent: Percent::of(0, 100),
        language: Language::Spanish,
        translate: None,
    };
    harness.send(ControlEvent::Worker(transcript_finished(
        TranscriptOutcome::Failed {
            partial: String::new(),
            error: EngineError::ModelMissing {
                path: "/x.gguf".to_owned(),
            },
        },
    )));
    assert!(harness.copied().is_empty(), "nichts kopiert");
    assert!(
        harness
            .last_note()
            .starts_with("„Hola“: Transkription fehlgeschlagen"),
        "Meldung: {}",
        harness.last_note()
    );
}

/// Ein fertiges Transkript der Datei `file`.
fn finished_file_transcript(harness: &mut Harness, file: PathBuf) {
    harness.controller.transcript = TranscriptJob::Transcribing {
        id: TranscriptId(0),
        origin: Origin::File(file),
        cancel: Arc::new(AtomicBool::new(false)),
        title: UserText::new("Eins"),
        percent: Percent::of(100, 100),
        language: Language::German,
        translate: None,
    };
    harness.send(ControlEvent::Worker(transcript_finished(
        TranscriptOutcome::Finished("Ein Satz.".to_owned()),
    )));
}

#[test]
fn a_transcript_that_cannot_be_saved_is_reported_in_english() {
    let dir = crate::testing::TempDir::new("controller-too-many");
    let file = dir.file("Eins.m4a", "kein Ton");
    for _ in 0..99 {
        crate::app::source::save_beside(&file, "alt").unwrap();
    }
    let mut harness = Harness::ready();
    finished_file_transcript(&mut harness, file);
    let note = harness.last_note_in_both();
    assert!(
        note.en()
            .starts_with("Transcript copied but not saved (already 99 transcripts"),
        "englische Meldung: {}",
        note.en()
    );
    assert!(
        note.de()
            .starts_with("Transkript kopiert, aber nicht gespeichert (schon 99"),
        "{}",
        note.de()
    );
}

#[test]
fn a_failed_copy_still_says_where_the_transcript_was_saved() {
    let dir = crate::testing::TempDir::new("controller-copy-fails");
    let file = dir.file("Eins.m4a", "kein Ton");
    let mut harness = Harness::ready();
    harness.controller.platform.copy.set(Outcome::Fails);
    finished_file_transcript(&mut harness, file);
    let note = harness.last_note();
    assert!(
        note.starts_with("Transkript nicht kopiert (Zwischenablage:")
            && note.contains("und als „Eins.txt“ gespeichert"),
        "Meldung: {note}"
    );
    assert!(
        dir.path().join("Eins.txt").is_file(),
        "trotzdem gespeichert"
    );
}

#[test]
fn a_transcript_request_after_a_failed_load_names_the_reason() {
    let dir = crate::testing::TempDir::new("controller-unavailable");
    let file = dir.file("Vorlesung.m4a", "kein Ton");
    let mut harness = Harness::loading();
    harness.send(ControlEvent::Worker(WorkerEvent::LoadFailed(
        EngineError::ModelMissing {
            path: "/x.gguf".to_owned(),
        },
    )));
    harness.send(ControlEvent::Panel(PanelCommand::File {
        language: Language::German,
        paths: vec![file],
        range: RangeInput::default(),
    }));
    let note = harness.last_note();
    assert!(
        note.starts_with("Kein Transkript möglich: Modelldatei fehlt"),
        "statt „noch nicht bereit“ der Grund: {note}"
    );
}

#[test]
fn the_translation_note_counts_pieces_not_paragraphs() {
    let mut harness = Harness::ready();
    transcribed_spanish_video(&mut harness, Some(Language::German));
    harness.send(translation_done(Translated {
        title: "Hola".to_owned(),
        text: "[0:05] Hallo zusammen.".to_owned(),
        total: 2,
        untranslated: Some(Untranslated {
            count: 1,
            first_error: PolishError::Empty,
        }),
        fallback: None,
    }));
    let note = harness.last_note_in_both();
    assert!(
        note.de()
            .contains("1 von 2 Stücken (Titel, Überschriften, Absätze) im Original")
            && note
                .en()
                .contains("1 of 2 pieces (title, headings, paragraphs) left in the original"),
        "{note:?}"
    );
}

#[test]
fn an_unreadable_clipboard_is_not_reported_as_empty() {
    let mut harness = Harness::ready();
    harness.controller.platform.clipboard.set(Outcome::Fails);
    harness.send(ControlEvent::Panel(PanelCommand::Youtube {
        language: LanguageChoice::Automatic,
        range: RangeInput::default(),
        translate: None,
    }));
    assert!(
        harness
            .last_note()
            .starts_with("Zwischenablage nicht lesbar: Zwischenablage:"),
        "statt „kein Link“: {}",
        harness.last_note()
    );
    harness.send(ControlEvent::Panel(PanelCommand::File {
        language: Language::German,
        paths: Vec::new(),
        range: RangeInput::default(),
    }));
    assert!(
        harness
            .last_note()
            .starts_with("Zwischenablage nicht lesbar"),
        "statt „keine Datei“: {}",
        harness.last_note()
    );
}

#[test]
fn checking_the_link_answers_without_a_sound_or_note() {
    let mut harness = Harness::ready();
    let notes = harness.controller.platform.notes.borrow().len();
    *harness.controller.platform.clipboard_text.borrow_mut() =
        Some("https://youtu.be/abc123".to_owned());
    harness.send(ControlEvent::Panel(PanelCommand::CheckLink));
    *harness.controller.platform.clipboard_text.borrow_mut() = Some("kein Link".to_owned());
    harness.send(ControlEvent::Panel(PanelCommand::CheckLink));
    *harness.controller.platform.clipboard_text.borrow_mut() = None;
    harness.send(ControlEvent::Panel(PanelCommand::CheckLink));
    harness.controller.platform.clipboard.set(Outcome::Fails);
    harness.send(ControlEvent::Panel(PanelCommand::CheckLink));
    assert_eq!(
        *harness.controller.platform.links.borrow(),
        [
            ClipboardLink::Youtube,
            ClipboardLink::Missing,
            ClipboardLink::Missing,
            ClipboardLink::Unreadable
        ]
    );
    assert_eq!(
        harness.controller.platform.notes.borrow().len(),
        notes,
        "nur die Maus liegt auf dem Knopf: keine Meldung"
    );
}

#[test]
fn a_youtube_link_on_the_clipboard_starts_loading() {
    let mut harness = Harness::ready();
    *harness.controller.platform.clipboard_text.borrow_mut() =
        Some("https://youtu.be/abc123".to_owned());
    harness.send(ControlEvent::Panel(PanelCommand::Youtube {
        language: LanguageChoice::Automatic,
        range: RangeInput::default(),
        translate: None,
    }));
    assert!(
        matches!(
            harness.controller.transcript,
            TranscriptJob::Downloading {
                origin: _,
                title: _,
                cancel: _,
                thread: _,
                translate: _
            }
        ),
        "Meldung: {}",
        harness.last_note()
    );
    // Der Lade-Thread läuft mit yt-dlp (oder scheitert ohne); abbrechen.
    harness.send(ControlEvent::Panel(PanelCommand::CancelTranscript));
    harness.send(ControlEvent::Shutdown);
}

#[test]
fn the_log_names_no_titles_paths_or_typed_text() {
    let panel = describe_event(&ControlEvent::Panel(PanelCommand::File {
        language: Language::German,
        paths: vec![PathBuf::from("/Users/anna/Therapie/Sitzung 3.m4a")],
        range: RangeInput {
            from: "Mandant".to_owned(),
            to: String::new(),
        },
    }));
    assert!(
        !panel.contains("Sitzung") && !panel.contains("anna") && !panel.contains("Mandant"),
        "{panel}"
    );
    let loaded = describe_event(&ControlEvent::SourceLoaded(Ok(Loaded {
        title: UserText::new("Sitzung 3"),
        language: Language::German,
        pcm: second(),
        begin: Duration::ZERO,
        chapters: Vec::new(),
    })));
    assert!(!loaded.contains("Sitzung"), "{loaded}");
    let failed = describe_event(&ControlEvent::SourceLoaded(Err(LoadError::NoAudio(
        UserText::new("Sitzung 3"),
    ))));
    assert!(!failed.contains("Sitzung"), "{failed}");
}

#[test]
fn a_note_with_a_title_shows_it_but_does_not_log_it() {
    let mut harness = Harness::ready();
    harness.controller.transcript = TranscriptJob::Downloading {
        origin: Origin::Youtube,
        title: None,
        cancel: Arc::new(AtomicBool::new(false)),
        thread: std::thread::Builder::new().spawn(|| {}).unwrap(),
        translate: None,
    };
    harness.send(ControlEvent::SourceLoaded(Err(LoadError::NoAudio(
        UserText::new("Sitzung 3"),
    ))));
    let note = harness.last_note_in_both();
    assert_eq!(note.de(), "YouTube: „Sitzung 3“ enthält keine Tonspur");
    assert_eq!(note.en(), "YouTube: “Sitzung 3” has no audio track");
    assert_eq!(note.log_line(), "YouTube: „…“ enthält keine Tonspur");
}

/// Der Modell-Thread ist mit dem ersten langen Auftrag fertig.
fn transcript_finished(outcome: TranscriptOutcome) -> WorkerEvent {
    WorkerEvent::TranscriptFinished {
        id: TranscriptId(0),
        outcome,
    }
}

#[test]
fn a_result_of_another_transcript_is_dropped() {
    let mut harness = Harness::ready();
    harness.controller.transcript = TranscriptJob::Transcribing {
        id: TranscriptId(3),
        origin: Origin::Youtube,
        cancel: Arc::new(AtomicBool::new(false)),
        title: UserText::new("B"),
        percent: Percent::of(0, 100),
        language: Language::German,
        translate: None,
    };
    harness.send(ControlEvent::Worker(WorkerEvent::TranscriptProgress {
        id: TranscriptId(2),
        progress: crate::engine::Progress { done: 1, total: 2 },
    }));
    harness.send(ControlEvent::Worker(WorkerEvent::TranscriptFinished {
        id: TranscriptId(2),
        outcome: TranscriptOutcome::Finished("A".to_owned()),
    }));
    assert!(
        harness.copied().is_empty(),
        "das Ergebnis von A ist nicht das von B"
    );
    assert!(
        matches!(
            harness.controller.transcript,
            TranscriptJob::Transcribing { percent, id: _, origin: _, cancel: _, title: _, language: _, translate: _ } if percent == Percent::ZERO
        ),
        "B läuft weiter, ohne fremden Fortschritt"
    );
}

#[test]
fn a_finished_transcript_after_cancel_counts_as_cancelled() {
    let mut harness = Harness::ready();
    let cancel = Arc::new(AtomicBool::new(false));
    harness.controller.transcript = TranscriptJob::Transcribing {
        id: TranscriptId(0),
        origin: Origin::Youtube,
        cancel: Arc::clone(&cancel),
        title: UserText::new("Hola"),
        percent: Percent::of(99, 100),
        language: Language::Spanish,
        translate: Some(Language::German),
    };
    harness.send(ControlEvent::Panel(PanelCommand::CancelTranscript));
    harness.send(ControlEvent::Worker(transcript_finished(
        TranscriptOutcome::Finished("Hola.".to_owned()),
    )));
    assert!(
        translation_request(&harness).is_none(),
        "keine Übersetzung nach Abbrechen"
    );
    assert!(
        harness
            .last_note()
            .starts_with("Abgebrochen, bisheriger Teil kopiert"),
        "{}",
        harness.last_note()
    );
}

#[test]
fn escape_tells_the_model_thread_to_drop_waiting_parts() {
    let mut harness = Harness::ready();
    harness.speak_a_part();
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Tick);
    harness.escape();
    let discarded = harness
        .jobs
        .try_iter()
        .any(|job| matches!(job, Job::Discard { dictation: _ }));
    assert!(discarded, "Esc meldet das verworfene Diktat");
}

#[test]
fn an_inserted_dictation_whose_clipboard_was_not_restored_says_both() {
    let mut harness = Harness::ready();
    harness.controller.platform.paste.set(Outcome::HalfWorks);
    harness.tap(Mode::Raw);
    harness.tap(Mode::Raw);
    let rest = harness.sent_part();
    harness.recognised(rest, "Ein Satz.");
    let note = harness.last_note();
    assert!(
        note.starts_with("Die Zwischenablage ließ sich nicht wiederherstellen")
            && note.ends_with("2 Wörter eingefügt."),
        "{note}"
    );
}

#[test]
fn a_recording_at_the_end_stops_after_ten_minutes_and_keeps_its_text() {
    let mut harness = Harness::ready();
    harness.send(ControlEvent::Panel(PanelCommand::Parts(PartTiming::AtEnd)));
    harness
        .controller
        .platform
        .unsent
        .set(dictation::MAX_UNSENT);
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Tick);
    let only = harness.sent_part();
    assert_eq!(only.kind, PartKind::Last, "die Aufnahme endet von selbst");
    harness.recognised(only, "Ein langer Vortrag.");
    assert_eq!(harness.pasted(), ["Ein langer Vortrag."]);
    let note = harness.last_note();
    assert!(
        note.starts_with("Aufnahme nach 10 Minuten beendet.") && note.ends_with("eingefügt."),
        "{note}"
    );
}

#[test]
fn a_recording_at_the_end_runs_on_below_ten_minutes() {
    let mut harness = Harness::ready();
    harness.send(ControlEvent::Panel(PanelCommand::Parts(PartTiming::AtEnd)));
    harness.controller.platform.unsent.set(
        dictation::MAX_UNSENT
            .checked_sub(Duration::from_secs(1))
            .unwrap(),
    );
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Tick);
    assert!(harness.is_recording());
}

#[test]
fn a_loaded_source_without_model_thread_makes_diktator_unavailable() {
    let mut harness = Harness::ready();
    let thread = std::thread::Builder::new().spawn(|| {}).unwrap();
    harness.controller.transcript = TranscriptJob::Downloading {
        origin: Origin::Youtube,
        title: None,
        cancel: Arc::new(AtomicBool::new(false)),
        thread,
        translate: None,
    };
    drop(std::mem::replace(&mut harness.jobs, mpsc::channel().1));
    harness.send(ControlEvent::SourceLoaded(Ok(Loaded {
        title: UserText::new("Vorlesung"),
        language: Language::German,
        pcm: second(),
        begin: Duration::ZERO,
        chapters: Vec::new(),
    })));
    assert!(
        matches!(harness.controller.state, State::Unavailable { reason: _ }),
        "ohne Modell-Thread schlägt das nächste fn nicht ein zweites Mal fehl"
    );
    assert_eq!(
        harness.last_note(),
        "Der Modell-Thread ist beendet. Diktator bitte neu starten."
    );
}

#[test]
fn a_running_recording_keeps_its_part_timing() {
    let mut harness = Harness::ready();
    harness.speak_a_part();
    harness.tap(Mode::Raw);
    harness.send(ControlEvent::Panel(PanelCommand::Parts(PartTiming::AtEnd)));
    harness.send(ControlEvent::Tick);
    assert_eq!(
        harness.sent_part().kind,
        PartKind::Partial,
        "die Aufnahme schneidet weiter wie bei ihrem Start"
    );
}

#[test]
fn the_settings_at_start_apply_to_the_first_dictation() {
    let (events, _events) = mpsc::channel();
    let (jobs, _jobs) = mpsc::channel();
    let (polish, _polish) = mpsc::channel();
    let controller = Controller::new(
        Connections {
            events,
            jobs,
            polish,
            escape_capture: Arc::new(AtomicBool::new(false)),
        },
        Feedback::silent(),
        Live {
            secondary: Language::French,
            timestamps: Timestamps::On,
            chapters: ChapterHeadings::Off,
            parts: PartTiming::AtEnd,
            language: UiLanguage::English,
        },
        FakeMac::default(),
    );
    assert_eq!(controller.secondary, Language::French);
    assert_eq!(controller.timestamps, Timestamps::On);
    assert_eq!(controller.chapters, ChapterHeadings::Off);
    assert_eq!(controller.parts, PartTiming::AtEnd);
    assert_eq!(controller.language, UiLanguage::English);
}

#[test]
fn a_new_recording_clears_the_last_note() {
    let mut harness = Harness::ready();
    harness.tap(Mode::Raw);
    assert_eq!(harness.controller.platform.cleared.get(), 1);
}

#[test]
fn a_microphone_that_does_not_open_leaves_diktator_ready() {
    let mut harness = Harness::ready();
    harness.controller.platform.opening.set(Outcome::Fails);
    harness.tap(Mode::Raw);
    assert!(harness.is_ready());
    assert!(!harness.captures_escape());
    assert!(
        harness.last_note().starts_with("Aufnahme nicht möglich"),
        "{}",
        harness.last_note()
    );
    // Das nächste fn versucht es wieder.
    harness.controller.platform.opening.set(Outcome::Works);
    harness.tap(Mode::Raw);
    assert!(harness.is_recording());
}

#[test]
fn a_readable_cursor_shapes_the_inserted_text() {
    let mut harness = Harness::ready();
    *harness.controller.platform.cursor.borrow_mut() = CursorState::Shows {
        before: "",
        after: " und",
    };
    harness.tap(Mode::Raw);
    harness.tap(Mode::Raw);
    let rest = harness.sent_part();
    harness.recognised(rest, "Das ist gut.");
    assert_eq!(
        harness.pasted(),
        ["Das ist gut"],
        "vor „ und“ fällt der Punkt weg"
    );
}

#[test]
fn an_unreadable_cursor_still_inserts() {
    let mut harness = Harness::ready();
    *harness.controller.platform.cursor.borrow_mut() = CursorState::Fails;
    harness.tap(Mode::Raw);
    harness.tap(Mode::Raw);
    let rest = harness.sent_part();
    harness.recognised(rest, "Das ist gut.");
    assert_eq!(harness.pasted(), ["Das ist gut."]);
}

/// Ob Diktator noch Diktate annimmt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Availability {
    /// Ja (auch während das Modell lädt).
    Usable,
    /// „Nicht verfügbar“.
    Gone,
}

/// Was in einer zufälligen Folge passieren kann.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// fn roh oder mit ⌥.
    Tap(Mode),
    /// Esc.
    Escape,
    /// Taktgeber (mit einem Teil, der gerade fertig wird).
    Tick,
    /// Pegeltakt.
    Meter,
    /// Das Modell beantwortet alle wartenden Teile.
    Recognise,
    /// Das Überarbeiten beantwortet alle wartenden Aufträge.
    Polish(Outcome),
    /// Das Modell ist geladen.
    ModelReady,
    /// Das Modell lädt nicht.
    ModelFails,
    /// Einfügen gelingt ab jetzt oder nicht.
    Paste(Outcome),
    /// Das Mikrofon öffnet ab jetzt oder nicht.
    Opening(Outcome),
    /// Panel auf oder zu.
    Panel(PanelState),
}

fn any_outcome() -> impl proptest::strategy::Strategy<Value = Outcome> {
    proptest::prop_oneof![
        proptest::strategy::Just(Outcome::Works),
        proptest::strategy::Just(Outcome::Fails),
        proptest::strategy::Just(Outcome::HalfWorks),
    ]
}

fn any_step() -> impl proptest::strategy::Strategy<Value = Step> {
    use proptest::strategy::{Just, Strategy};
    proptest::prop_oneof![
        4 => proptest::prop_oneof![Just(Mode::Raw), Just(Mode::Polish)].prop_map(Step::Tap),
        1 => Just(Step::Escape),
        3 => Just(Step::Tick),
        1 => Just(Step::Meter),
        3 => Just(Step::Recognise),
        2 => any_outcome().prop_map(Step::Polish),
        1 => Just(Step::ModelReady),
        1 => Just(Step::ModelFails),
        1 => any_outcome().prop_map(Step::Paste),
        1 => any_outcome().prop_map(Step::Opening),
        1 => proptest::prop_oneof![Just(PanelState::Open), Just(PanelState::Closed)]
            .prop_map(Step::Panel),
    ]
}

impl Harness {
    /// Führt einen zufälligen Schritt aus.
    fn step(&mut self, step: Step) {
        match step {
            Step::Tap(mode) => self.tap(mode),
            Step::Escape => self.escape(),
            Step::Tick => {
                self.speak_a_part();
                self.send(ControlEvent::Tick);
            }
            Step::Meter => self.send(ControlEvent::Meter),
            Step::Recognise => self.recognise_waiting(),
            Step::Polish(outcome) => self.polish_waiting(outcome),
            Step::ModelReady => self.send(ControlEvent::Worker(WorkerEvent::Ready)),
            Step::ModelFails => self.send(ControlEvent::Worker(WorkerEvent::LoadFailed(
                EngineError::ModelMissing {
                    path: "/x.gguf".to_owned(),
                },
            ))),
            Step::Paste(outcome) => self.controller.platform.paste.set(outcome),
            Step::Opening(outcome) => self.controller.platform.opening.set(outcome),
            Step::Panel(panel) => self.send(ControlEvent::Panel(PanelCommand::Visibility(panel))),
        }
    }

    /// Erkennt alle wartenden Teile als „Ein Satz.“.
    fn recognise_waiting(&mut self) {
        let parts: Vec<PartId> = self
            .jobs
            .try_iter()
            .filter_map(|job| match job {
                Job::Dictation {
                    part,
                    pcm: _,
                    language: _,
                } => Some(part),
                Job::Warmup
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
                | Job::SetUnload(_)
                | Job::Shutdown => None,
            })
            .collect();
        for part in parts {
            self.recognised(part, "Ein Satz.");
        }
    }

    /// Beantwortet alle wartenden Aufträge zum Überarbeiten mit `outcome`.
    fn polish_waiting(&mut self, outcome: Outcome) {
        let requests: Vec<DictationId> = self
            .polish
            .try_iter()
            .filter_map(|request| match request {
                PolishRequest::Polish {
                    dictation,
                    text: _,
                    language: _,
                } => Some(dictation),
                PolishRequest::Warmup
                | PolishRequest::UseBackend(_)
                | PolishRequest::Translate(_)
                | PolishRequest::Shutdown => None,
            })
            .collect();
        for dictation in requests {
            let result = match outcome {
                Outcome::Works | Outcome::HalfWorks => Ok(Polished {
                    text: "Ein Satz.".to_owned(),
                    fallback: None,
                }),
                Outcome::Fails => Err(PolishError::Empty),
            };
            self.polished(dictation, result);
        }
    }

    /// Ob Diktator vor dem Schritt „nicht verfügbar“ war.
    fn availability(&self) -> Availability {
        match self.controller.state {
            State::Unavailable { reason: _ } => Availability::Gone,
            State::Loading
            | State::Ready
            | State::Recording {
                recording: _,
                dictation: _,
            }
            | State::Processing(_)
            | State::Polishing(_) => Availability::Usable,
        }
    }

    /// Was nach jedem Schritt gelten muss; `before` ist der Stand davor.
    fn check(&self, before: Availability) {
        let state = &self.controller.state;
        assert_eq!(
            self.captures_escape(),
            state.captures_escape(),
            "Esc gehört Diktator genau während eines Diktats ({})",
            state.name()
        );
        assert!(
            before == Availability::Usable || self.availability() == Availability::Gone,
            "aus „nicht verfügbar“ führt kein Weg zurück"
        );
        match state {
            State::Recording {
                dictation,
                recording: _,
            } => assert_eq!(
                self.controller.status(),
                Status::Recording {
                    language: dictation.language,
                    mode: dictation.mode,
                }
            ),
            State::Loading
            | State::Ready
            | State::Processing(_)
            | State::Polishing(_)
            | State::Unavailable { reason: _ } => {}
        }
    }
}

proptest::proptest! {
    #[test]
    fn random_steps_keep_the_controller_consistent(
        steps in proptest::collection::vec(any_step(), 1..40)
    ) {
        let mut harness = Harness::loading();
        for step in steps {
            let before = harness.availability();
            harness.step(step);
            harness.check(before);
        }
    }
}
