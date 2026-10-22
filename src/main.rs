//! Diktator – Menüleisten-App.
//!
//! fn antippen: Aufnahme auf Deutsch, fn + ⇧ (Umschalttaste): in der
//! zweiten Sprache (Englisch, wählbar), erneut fn: transkribieren und an der
//! Schreibmarke einfügen, Esc: Abbruch. Taste, zweite Sprache, Klänge und
//! Modell stehen in `settings.toml`.
//! `YouTube`-Videos und Dateien: Link bzw. Datei kopieren, im Panel den
//! passenden Reiter wählen; das Transkript landet in der Zwischenablage.
//!
//! Ohne Argument ist dieser Prozess nur der Wächter
//! ([`watchdog`]): Er startet die eigentliche App als Kindprozess
//! (`--kind`) und startet sie nach einem Absturz neu.
//!
//! Menüleisten-Symbol und Panel zeichnet das Swift-Hilfsprogramm
//! `diktator-ui` (siehe [`app::ui`]). Der Hauptthread startet die
//! übrigen Threads und liest dann die Befehle aus dem Panel. Endet das
//! Hilfsprogramm („Beenden“), endet das Programm samt allen Threads.

mod app;
mod audio;
mod clock;
mod context;
mod dictation_id;
mod engine;
mod feedback;
mod helper;
mod hotkey;
mod language;
mod launch;
mod note;
mod numeric;
mod paste;
mod paths;
mod polish;
mod process;
mod settings;
#[cfg(test)]
mod testing;
mod text;
mod textfile;
mod watchdog;
mod youtube;

use std::error::Error;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::app::controller::{
    Connections, ControlEvent, Controller, Live, Mac, PanelCommand, PanelState,
};
use crate::app::source::LanguageChoice;
use crate::app::status::{
    LanguageOption, PROTOCOL_VERSION, PolishModelOption, SettingsView, Setup, UiUpdate,
};
use crate::app::ui::{self, Incoming, Rejected, Ui, UiCommand, UiError};
use crate::app::worker::{self, Job, ModelThread};
use crate::clock::{Clock as _, SystemClock};
use crate::feedback::{Feedback, FeedbackThread, Logging};
use crate::hotkey::TriggerKey;
use crate::launch::Autostart;
use crate::note::{Explain, Note, UiLanguage};
use crate::polish::llama;
use crate::polish::{Backend, PolishRequest, PolishThread};
use crate::settings::{PolishBackend, Restart, Settings, SettingsEntries};
use crate::text::dictionary::{DictionaryEntries, DictionaryError};
use crate::watchdog::Ending;

/// Abstand der Takte; jeder fünfte sieht statt nach dem Pegel nach fertigen
/// Teilen (also alle halbe Sekunde).
const METER_TICK: Duration = Duration::from_millis(100);

fn main() -> ExitCode {
    install_panic_hook();
    let child = std::env::args_os().any(|argument| argument == watchdog::CHILD_ARGUMENT);
    if child {
        match run() {
            Ok(Outcome::Quit) => ExitCode::SUCCESS,
            Ok(Outcome::Restart) => ExitCode::from(watchdog::RESTART_CODE),
            Ok(Outcome::UiGone) => ExitCode::FAILURE,
            // Ein Neustart scheiterte genauso: Der Wächter meldet es und
            // startet nicht neu.
            Err(error) => {
                log_now(&format!("Diktator startet nicht: {error}"));
                ExitCode::from(watchdog::START_FAILED_CODE)
            }
        }
    } else {
        supervise().unwrap_or_else(|error| {
            log_now(&format!("Wächter: {error}"));
            ExitCode::FAILURE
        })
    }
}

/// Wie die App endet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// „Beenden“.
    Quit,
    /// „Jetzt neu starten“: Der Wächter startet sie sofort wieder.
    Restart,
    /// Die Oberfläche ist unerwartet weg; zählt beim Wächter als Absturz.
    UiGone,
}

/// Die Einstellungen, wie sie gerade in der Datei stehen; `None`, wenn sie
/// fehlt oder unlesbar ist (das meldet die App selbst beim Start).
fn current_settings() -> Option<Settings> {
    let path = paths::settings_path().ok()?;
    Settings::load(&path).ok().map(|read| read.value)
}

/// Schreibt eine Zeile ins Protokoll, wenn der Schalter in der Datei es
/// gerade erlaubt, sonst nach stderr; für den Wächter und die Panik, die
/// keinen Rückmelde-Thread haben. Ist die Datei unlesbar, wird
/// protokolliert: Das ist die Voreinstellung, und gerade dann hilft das
/// Protokoll.
fn log_now(line: &str) {
    let logging = current_settings().map_or(Logging::On, |settings| settings.logging);
    match logging {
        Logging::On => {
            if let Err(error) = feedback::write_log_line(line) {
                eprintln!("{line} (Protokoll nicht schreibbar: {error})");
            }
        }
        Logging::Off => eprintln!("{line}"),
    }
}

/// Im Release-Build beendet jede Panik das Programm sofort (`panic =
/// "abort"`), und die Meldung auf stderr sieht niemand. Deshalb kommen Ort
/// und Thread ins Protokoll – nicht die Meldung selbst, die Text des
/// Nutzers enthalten kann. Danach läuft die übliche Ausgabe auf stderr.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log_now(&panic_line(info.location(), thread::current().name()));
        previous(info);
    }));
}

/// Die Zeile fürs Protokoll zu einer Panik an `location` im Thread
/// `thread`.
fn panic_line(location: Option<&std::panic::Location<'_>>, thread: Option<&str>) -> String {
    let place = location.map_or_else(
        || "unbekannter Stelle".to_owned(),
        |location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        },
    );
    format!(
        "Panik im Thread „{}“ an {place}",
        thread.unwrap_or("ohne Namen")
    )
}

/// Der Wächter: startet die App als Kindprozess und nach Abstürzen neu.
fn supervise() -> Result<ExitCode, Box<dyn Error>> {
    let executable = std::env::current_exe()?;
    // Schalter und Sprache erst beim Melden lesen: Der Wächter läuft die
    // ganze Sitzung, beides kann sich inzwischen geändert haben. Ist die
    // Datei unlesbar, gilt Deutsch, die Voreinstellung.
    let notify = |message: &Note| {
        let language = current_settings().map_or(UiLanguage::German, |settings| settings.language);
        if let Err(error) = feedback::show_notification(message.text(language)) {
            log_now(&format!("Mitteilung nicht möglich: {error}"));
        }
    };
    Ok(
        match watchdog::supervise(&executable, &SystemClock, log_now, notify)? {
            Ending::Quit => ExitCode::SUCCESS,
            Ending::GaveUp | Ending::StartFailed => ExitCode::FAILURE,
        },
    )
}

/// Die eigentliche App (im Kindprozess des Wächters).
fn run() -> Result<Outcome, Box<dyn Error>> {
    // Ein Sprachmodell-Server, den ein abgestürzter Lauf hinterlassen hat.
    llama::clean_up_stale();
    let settings_path = paths::settings_path()?;
    for path in [settings_path.clone(), paths::dictionary_path()?] {
        // Was ein abgebrochenes Speichern hinterließ. Bleibt es liegen,
        // überschreibt das nächste Speichern es ohnehin; scheitert jenes,
        // meldet es den Fehler.
        match textfile::remove_stale(&path) {
            Ok(()) | Err(_) => {}
        }
    }
    let (settings, settings_problem) = match Settings::load(&settings_path) {
        Ok(read) => {
            let adjusted = read.note();
            (read.value, adjusted)
        }
        Err(error) => (Settings::default(), Some(error.note())),
    };

    let (updates, update_receiver) = ui::channel();
    send(&updates, UiUpdate::Setup(setup(&settings)));
    // Ab hier baut `background` auf jedem Weg ab, auch bei einem Fehler.
    let mut background = start_background(&updates, &settings, settings_problem.as_ref())?;
    // Endet der Wächter, endet auch die App: Mit Oberfläche schließt sie
    // sich, ohne endet die Steuerung.
    let quit_updates = updates.clone();
    let quit_commands = background.commands.clone();
    background.watch = Some(watchdog::watch_parent(move || {
        send(&quit_updates, UiUpdate::Quit);
        stop_controller(&quit_commands);
    })?);
    let preferences = Preferences {
        running: settings,
        updates: updates.clone(),
        commands: background.commands.clone(),
        polish: background.polish.requests(),
        jobs: background.model.jobs(),
        feedback: background.feedback.clone(),
    };
    preferences.apply_live(&preferences.running);
    preferences.show();

    let (outcome, panel, stop) = match Ui::spawn(updates, update_receiver) {
        Ok(mut panel) => (
            Ok(serve_panel(&mut panel, &background, &preferences)),
            Some(panel),
            Stop::Now,
        ),
        Err(error @ UiError::MissingHelper) => {
            // Etwa bei `cargo run`: Tasten gehen trotzdem; Ende mit Ctrl-C
            // oder mit dem Wächter.
            background.feedback.log(error.to_string());
            (Ok(Outcome::Quit), None, Stop::WhenControllerEnds)
        }
        // Meldet der Wächter (Code für einen gescheiterten Start).
        Err(error) => (Err(error), None, Stop::Now),
    };
    background.shut_down(panel, stop);
    outcome.map_err(Into::into)
}

/// Wie der Abbau beginnt.
#[derive(Debug, Clone, Copy)]
enum Stop {
    /// Die Steuerung sofort beenden.
    Now,
    /// Warten, bis die Steuerung von selbst endet (ohne Oberfläche: wenn
    /// der Wächter endet).
    WhenControllerEnds,
}

/// Bittet die Steuerung, zu enden.
fn stop_controller(commands: &Sender<ControlEvent>) {
    // Scheitert nur, wenn sie schon beendet ist.
    match commands.send(ControlEvent::Shutdown) {
        Ok(()) | Err(mpsc::SendError(_)) => {}
    }
}

/// Was mit einem Befehl aus dem Panel geschieht.
#[derive(Debug)]
enum Step {
    /// Geht an die Steuerung.
    Forward(PanelCommand),
    /// Ist hier schon erledigt.
    Handled,
    /// Beendet die App.
    End(Outcome),
}

/// Arbeitet die Befehle aus dem Panel ab, bis es beendet wird.
fn serve_panel(panel: &mut Ui, background: &Background, preferences: &Preferences) -> Outcome {
    let log = |line: String| background.feedback.log(line);
    while let Some(incoming) = panel.next_command(log) {
        let command = match incoming {
            Incoming::Command(command) => command,
            Incoming::Rejected(rejected) => {
                preferences.reject(rejected);
                continue;
            }
        };
        match step(command, background, preferences) {
            Step::Forward(command) => forward(background, command),
            Step::Handled => {}
            Step::End(outcome) => return outcome,
        }
    }
    // Die Oberfläche endete ohne „Beenden“: abgestürzt, oder der Wächter
    // ist weg. Im ersten Fall startet der Wächter neu.
    background.feedback.log("Oberfläche unerwartet beendet");
    Outcome::UiGone
}

/// Erledigt einen Befehl aus dem Panel, der hier erledigt wird; die
/// übrigen gehen an die Steuerung.
fn step(command: UiCommand, background: &Background, preferences: &Preferences) -> Step {
    let updates = &preferences.updates;
    match command {
        UiCommand::Youtube {
            language,
            range,
            translate,
        } => Step::Forward(PanelCommand::Youtube {
            language: language.map_or(LanguageChoice::Automatic, LanguageChoice::Fixed),
            range,
            translate,
        }),
        UiCommand::File {
            language,
            paths,
            range,
        } => Step::Forward(PanelCommand::File {
            language,
            paths,
            range,
        }),
        UiCommand::CancelTranscript => Step::Forward(PanelCommand::CancelTranscript),
        UiCommand::CheckLink => Step::Forward(PanelCommand::CheckLink),
        UiCommand::CopyLastTranscript { source } => {
            Step::Forward(PanelCommand::CopyLastTranscript(source))
        }
        UiCommand::OpenDictionary => Step::Forward(PanelCommand::OpenDictionary),
        UiCommand::CopyHistory { serial } => Step::Forward(PanelCommand::CopyHistory(serial)),
        UiCommand::Autostart { on } => {
            set_autostart(on, updates);
            Step::Handled
        }
        UiCommand::SetSettings { settings } => {
            preferences.save_settings(&settings);
            Step::Handled
        }
        UiCommand::SetDictionary { dictionary } => {
            preferences.save_dictionary(&dictionary);
            Step::Handled
        }
        UiCommand::Log { message } => {
            background.feedback.log(format!("Oberfläche: {message}"));
            Step::Handled
        }
        UiCommand::ClearLog => {
            clear_log(updates);
            Step::Handled
        }
        UiCommand::Panel { visible: true } => {
            // Die Dateien könnten von Hand geändert worden sein.
            preferences.show();
            Step::Forward(PanelCommand::Visibility(PanelState::Open))
        }
        UiCommand::Panel { visible: false } => {
            Step::Forward(PanelCommand::Visibility(PanelState::Closed))
        }
        UiCommand::Restart => Step::End(Outcome::Restart),
        UiCommand::Quit => Step::End(Outcome::Quit),
    }
}

/// Gibt einen Befehl an die Steuerung weiter.
fn forward(background: &Background, command: PanelCommand) {
    if background
        .commands
        .send(ControlEvent::Panel(command))
        .is_err()
    {
        background
            .feedback
            .log("Steuerung beendet; Befehl verworfen");
    }
}

/// Leert das Protokoll und sagt, ob es ging.
fn clear_log(updates: &Sender<UiUpdate>) {
    let note = match feedback::clear_log() {
        Ok(()) => Note::new("Protokoll gelöscht.", "Log cleared."),
        Err(error) => Note::new(
            format!("Protokoll nicht löschbar: {error}"),
            format!("Cannot clear the log: {}", error.english()),
        ),
    };
    send(updates, UiUpdate::Note(note));
}

/// Einstellungen und Wörterbuch, wie das Panel sie bearbeitet.
struct Preferences {
    /// Die Einstellungen, mit denen Diktator läuft.
    running: Settings,
    /// Weg zur Oberfläche.
    updates: Sender<UiUpdate>,
    /// Eingang der Steuerung (für die zweite Sprache, die sofort gilt).
    commands: Sender<ControlEvent>,
    /// Eingang des Überarbeiten-Threads (für das Modell, das sofort gilt).
    polish: Sender<PolishRequest>,
    /// Eingang des Modell-Threads (für die Entladezeit, die sofort gilt).
    jobs: Sender<Job>,
    /// Rückmeldung (für Protokoll und Klänge, die sofort gelten).
    feedback: Feedback,
}

impl Preferences {
    /// Schickt beide Reiter neu an das Panel, so wie die Dateien sind.
    fn show(&self) {
        self.show_settings();
        self.show_dictionary();
    }

    /// Reiter „Einstellungen“ aus der Datei. Was ohne Neustart gilt, gilt
    /// ab jetzt so, wie die Datei es sagt – auch von Hand Geändertes –,
    /// damit das Panel nie etwas als wirksam zeigt, das nicht wirkt.
    fn show_settings(&self) {
        let entries = paths::settings_path()
            .map_err(settings::SettingsError::from)
            .and_then(|path| SettingsEntries::read(&path))
            // Ersetzte Werte einer älteren Datei meldet schon der Start.
            .map(|read| read.value);
        let restart_needed = restart_for(entries.as_ref().ok(), &self.running);
        let entries = match entries {
            Ok(entries) => {
                self.apply_live(&entries.settings());
                entries
            }
            Err(error) => {
                send(&self.updates, UiUpdate::Note(error.note()));
                SettingsEntries::default()
            }
        };
        let models = settings::available_models().unwrap_or_else(|error| {
            send(&self.updates, UiUpdate::Note(error.note()));
            Vec::new()
        });
        let view = SettingsView {
            restart_needed,
            models,
            languages: LanguageOption::all(),
            polish_models: PolishModelOption::available(),
            llama_installed: llama::installed(),
            entries,
        };
        send(&self.updates, UiUpdate::Settings(view));
    }

    /// Reiter „Wörterbuch“ aus der Datei.
    fn show_dictionary(&self) {
        let entries = paths::dictionary_path()
            .map_err(DictionaryError::from)
            .and_then(|path| DictionaryEntries::read(&path));
        match entries {
            Ok(entries) => send(&self.updates, UiUpdate::Dictionary(entries)),
            Err(error) => send(&self.updates, UiUpdate::Note(error.note())),
        }
    }

    /// Gibt den Threads alles, was ohne Neustart gilt: Protokoll, Klänge,
    /// zweite Sprache, Zeitmarken, Teile beim Diktieren, Sprache der
    /// Oberfläche, Entladezeit und womit überarbeitet wird.
    fn apply_live(&self, settings: &Settings) {
        self.feedback.set_logging(settings.logging);
        self.feedback.set_sounds(settings.sounds);
        self.feedback.set_language(settings.language);
        let secondary = PanelCommand::SecondaryLanguage(settings.secondary);
        let timestamps = PanelCommand::Timestamps(settings.timestamps);
        let chapters = PanelCommand::Chapters(settings.chapters);
        let parts = PanelCommand::Parts(settings.parts);
        let language = PanelCommand::Language(settings.language);
        let delivered = self.commands.send(ControlEvent::Panel(secondary)).is_ok()
            && self.commands.send(ControlEvent::Panel(timestamps)).is_ok()
            && self.commands.send(ControlEvent::Panel(chapters)).is_ok()
            && self.commands.send(ControlEvent::Panel(parts)).is_ok()
            && self.commands.send(ControlEvent::Panel(language)).is_ok()
            && self.jobs.send(Job::SetUnload(settings.unload)).is_ok();
        if !delivered {
            self.feedback
                .log("Steuerung oder Modell beendet; Einstellung verworfen");
        }
        let backend = match settings.polish.backend {
            PolishBackend::Apple => Backend::Apple,
            PolishBackend::Local => match settings.polish.model_path() {
                Ok(model) => Backend::Local {
                    model,
                    unload: settings.polish.unload,
                },
                Err(error) => {
                    send(&self.updates, UiUpdate::Note(error.note()));
                    return;
                }
            },
        };
        if self
            .polish
            .send(PolishRequest::UseBackend(backend))
            .is_err()
        {
            self.feedback
                .log("Überarbeiten beendet; Einstellung verworfen");
        }
    }

    /// Speichert geänderte Einstellungen – aber nicht über eine Datei, die
    /// sich nicht lesen lässt: Das Panel zeigt dann Standardwerte, und
    /// gespeichert wären alle übrigen Einstellungen verloren.
    /// Das Panel schickte ungültige Werte: melden und den gespeicherten
    /// Stand zurückschicken, damit es nichts als gespeichert zeigt, was
    /// nicht gespeichert ist.
    fn reject(&self, rejected: Rejected) {
        let note = match rejected {
            Rejected::Settings => Note::new(
                "Einstellungen nicht gespeichert: ungültiger Wert.",
                "Settings not saved: invalid value.",
            ),
            Rejected::Dictionary => Note::new(
                "Wörterbuch nicht gespeichert: ungültiger Eintrag.",
                "Dictionary not saved: invalid entry.",
            ),
        };
        self.feedback.log(note.log_line());
        send(&self.updates, UiUpdate::Note(note));
        match rejected {
            Rejected::Settings => self.show_settings(),
            Rejected::Dictionary => self.show_dictionary(),
        }
    }

    fn save_settings(&self, entries: &SettingsEntries) {
        let saved = paths::settings_path()
            .map_err(|error| SaveError::Failed(settings::SettingsError::from(error)))
            .and_then(|path| {
                SettingsEntries::read(&path).map_err(SaveError::Unreadable)?;
                entries.write(&path).map_err(SaveError::Failed)
            });
        match saved {
            // Gelten lässt sie `show_settings` aus der gespeicherten Datei.
            Ok(()) => {}
            Err(error) => send(&self.updates, UiUpdate::Note(error.note())),
        }
        self.show_settings();
    }

    /// Speichert das geänderte Wörterbuch; es gilt ab dem nächsten Diktat.
    /// Eine Datei, die sich nicht lesen lässt, bleibt (siehe
    /// [`Preferences::save_settings`]).
    fn save_dictionary(&self, entries: &DictionaryEntries) {
        let saved = paths::dictionary_path()
            .map_err(|error| SaveError::Failed(DictionaryError::from(error)))
            .and_then(|path| {
                DictionaryEntries::read(&path).map_err(SaveError::Unreadable)?;
                entries.write(&path).map_err(SaveError::Failed)
            });
        let note = match saved {
            Ok(()) => Note::new(
                "Wörterbuch gespeichert – gilt ab dem nächsten Diktat.",
                "Dictionary saved – applies from the next dictation.",
            ),
            Err(error) => error.note(),
        };
        send(&self.updates, UiUpdate::Note(note));
        self.show_dictionary();
    }
}

/// Warum eine Änderung aus dem Panel nicht gespeichert wurde.
#[derive(Debug, thiserror::Error)]
enum SaveError<E> {
    /// Die Datei auf der Platte ist fehlerhaft; sie bleibt, wie sie ist.
    #[error("Nicht gespeichert – erst die Datei korrigieren. {0}")]
    Unreadable(E),
    /// Lesen oder Schreiben scheiterte.
    #[error(transparent)]
    Failed(E),
}

impl<E: Explain> Explain for SaveError<E> {
    fn english(&self) -> String {
        match self {
            Self::Unreadable(error) => {
                format!("Not saved – correct the file first. {}", error.english())
            }
            Self::Failed(error) => error.english(),
        }
    }
}

/// Schaltet den Autostart und meldet den tatsächlichen Stand zurück.
fn set_autostart(wanted: Autostart, updates: &Sender<UiUpdate>) {
    let note = match launch::set_autostart(wanted) {
        Ok(()) => match wanted {
            Autostart::On => Note::new(
                "Diktator startet beim Anmelden.",
                "Diktator starts at login.",
            ),
            Autostart::Off => Note::new(
                "Diktator startet nicht mehr beim Anmelden.",
                "Diktator no longer starts at login.",
            ),
        },
        Err(error) => error.note(),
    };
    send(updates, UiUpdate::Autostart(launch::autostart()));
    send(updates, UiUpdate::Note(note));
}

/// Feste Angaben für den Reiter „Info“.
fn setup(settings: &Settings) -> Setup {
    Setup {
        protocol_version: PROTOCOL_VERSION,
        key: settings.key,
        model: settings.model_file.clone(),
        version: env!("CARGO_PKG_VERSION"),
        autostart: launch::autostart(),
        // Ohne Benutzerordner gibt es kein Protokoll; das Panel bietet dann
        // kein Öffnen an.
        log: paths::log_file()
            .ok()
            .map(|path| path.display().to_string()),
    }
}

/// Schickt eine Änderung an die Oberfläche. Läuft keine, geht sie verloren.
fn send(updates: &Sender<UiUpdate>, update: UiUpdate) {
    match updates.send(update) {
        Ok(()) | Err(mpsc::SendError(_)) => {}
    }
}

/// Die Hintergrund-Threads und die Prozesse, die sie besitzen. Jeder hat
/// hier seinen Besitzer; [`Background::shut_down`] baut sie in fester
/// Reihenfolge ab und wartet auf jeden, auch wenn `Background` auf einem
/// Fehlerweg wegfällt.
struct Background {
    /// Eingang der Steuerung für Befehle aus dem Panel.
    commands: Sender<ControlEvent>,
    /// Klänge, Mitteilungen, Protokoll.
    feedback: Feedback,
    /// Die Steuerung; `None`, sobald beendet.
    controller: Option<JoinHandle<()>>,
    /// Der Tasten-Mitleser.
    hotkey: Stoppable,
    /// Der Taktgeber.
    ticker: Stoppable,
    /// Die Wache über den Wächter (gesetzt, sobald sie läuft).
    watch: Option<watchdog::ParentWatch>,
    /// Der Überarbeiten-Thread samt seinem llama-server.
    polish: PolishThread,
    /// Der Modell-Thread; endet er, gibt er das Modell frei.
    model: ModelThread,
    /// Der Rückmelde-Thread.
    feedback_thread: FeedbackThread,
}

impl Background {
    /// Baut in fester Reihenfolge ab, mehrfach aufrufbar:
    /// 1. die Oberfläche (keine Befehle mehr aus dem Panel),
    /// 2. die Steuerung (danach beginnt kein Diktat, keine Überarbeitung
    ///    und kein Transkript mehr; ein laufendes Diktat legt sie noch in
    ///    Verlauf und Zwischenablage),
    /// 3. was ihr Ereignisse schickt: Wache, Tasten-Mitleser, Taktgeber,
    /// 4. den Überarbeiten-Thread, dann erst seinen llama-server (so startet
    ///    kein laufender Auftrag ihn neu),
    /// 5. den Modell-Thread (gibt das Modell frei),
    /// 6. zuletzt die Rückmeldung, über die alle anderen bis dahin melden.
    fn shut_down(&mut self, panel: Option<Ui>, stop: Stop) {
        drop(panel);
        if let Some(controller) = self.controller.take() {
            match stop {
                Stop::Now => stop_controller(&self.commands),
                Stop::WhenControllerEnds => {}
            }
            if controller.join().is_err() {
                self.feedback.log("Steuerung ist abgestürzt");
            }
        }
        if let Some(mut watch) = self.watch.take() {
            watch.stop();
        }
        self.hotkey.stop(&self.feedback);
        self.ticker.stop(&self.feedback);
        if let Err(error) = self.polish.stop() {
            self.feedback.log(error.to_string());
        }
        self.model.stop();
        self.feedback_thread.stop();
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        // Nach `shut_down` ist nichts mehr zu tun; auf einem Fehlerweg baut
        // dies ab.
        self.shut_down(None, Stop::Now);
    }
}

/// Ein Thread, der endet, sobald sein Flag gesetzt ist.
struct Stoppable {
    /// Name fürs Protokoll.
    name: &'static str,
    stop: Arc<AtomicBool>,
    /// `None`, sobald beendet.
    thread: Option<JoinHandle<()>>,
}

impl Stoppable {
    /// Setzt das Flag und wartet auf den Thread; mehrfach aufrufbar.
    fn stop(&mut self, feedback: &Feedback) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            feedback.log(format!("{} ist abgestürzt", self.name));
        }
    }
}

/// Startet Rückmeldung, Modell, Überarbeiten, Tasten-Mitleser und Steuerung. Ein Fehler
/// in den Einstellungen (`settings_problem`) wird als Mitteilung gemeldet.
fn start_background(
    updates: &Sender<UiUpdate>,
    settings: &Settings,
    settings_problem: Option<&Note>,
) -> Result<Background, Box<dyn Error>> {
    let (feedback, feedback_thread) =
        Feedback::spawn(settings.sounds, settings.logging, settings.language)?;
    // Ab hier endet der Rückmelde-Thread auch auf einem Fehlerweg (Drop).
    if let Some(problem) = settings_problem {
        feedback.notify(Note::new(
            format!("{} – es gelten die Standardwerte.", problem.de()),
            format!("{} – the defaults apply.", problem.en()),
        ));
    }
    let (events, receiver) = mpsc::channel::<ControlEvent>();
    let escape_capture = Arc::new(AtomicBool::new(false));

    // Scheitert danach noch etwas, gibt `model` beim Wegfallen das Modell
    // wieder frei.
    let model = worker::spawn(
        settings.model_path()?,
        paths::dictionary_path()?,
        settings.unload,
        events.clone(),
        feedback.clone(),
    )?;
    let jobs = model.jobs();
    let polish = polish::spawn(
        events.clone(),
        feedback.clone(),
        llama::ServerPid::new(llama::pid_file()),
    )?;
    let hotkey = spawn_hotkey(events.clone(), Arc::clone(&escape_capture), settings.key)?;
    let ticker = spawn_ticker(events.clone())?;

    let controller_events = events.clone();
    let controller_feedback = feedback.clone();
    let controller_polish = polish.requests();
    let controller_updates = updates.clone();
    let live = live_settings(settings);
    let controller_thread =
        thread::Builder::new()
            .name("controller".to_owned())
            .spawn(move || {
                let update_ui = move |update: UiUpdate| send(&controller_updates, update);
                Controller::new(
                    Connections {
                        events: controller_events,
                        jobs,
                        polish: controller_polish,
                        escape_capture,
                    },
                    controller_feedback,
                    live,
                    Mac::new(update_ui),
                )
                .run(&receiver);
            })?;

    Ok(Background {
        commands: events,
        feedback,
        controller: Some(controller_thread),
        hotkey,
        ticker,
        watch: None,
        polish,
        model,
        feedback_thread,
    })
}

/// Startet den Taktgeber: alle 100 ms ein Takt, alle halbe Sekunde ein
/// [`ControlEvent::Tick`] (damit die Steuerung während einer Aufnahme
/// fertige Teile abholt), dazwischen [`ControlEvent::Meter`] für die
/// Pegelanzeige (achtmal pro Sekunde).
fn spawn_ticker(events: Sender<ControlEvent>) -> std::io::Result<Stoppable> {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let thread = thread::Builder::new()
        .name("ticker".to_owned())
        .spawn(move || {
            for beat in [
                Beat::Parts,
                Beat::Meter,
                Beat::Meter,
                Beat::Meter,
                Beat::Meter,
            ]
            .iter()
            .cycle()
            {
                let event = match beat {
                    Beat::Parts => ControlEvent::Tick,
                    Beat::Meter => ControlEvent::Meter,
                };
                if stopped.load(Ordering::Relaxed) || events.send(event).is_err() {
                    return;
                }
                SystemClock.sleep(METER_TICK);
            }
        })?;
    Ok(Stoppable {
        name: "Taktgeber",
        stop,
        thread: Some(thread),
    })
}

/// Ein Schlag des Taktgebers.
#[derive(Debug, Clone, Copy)]
enum Beat {
    /// Nach fertigen Teilen sehen.
    Parts,
    /// Pegel anzeigen.
    Meter,
}

/// Ob das Panel „Neustart nötig“ zeigt: wenn die gelesene Datei von
/// `running` abweicht. Ist sie unlesbar (`None`), zeigt das Panel
/// Standardwerte; ein Grund für einen Neustart sind die nicht.
fn restart_for(read: Option<&SettingsEntries>, running: &Settings) -> Restart {
    match read {
        Some(entries) => entries.settings().needs_restart_from(running),
        None => Restart::NotNeeded,
    }
}

/// Was die Steuerung von `settings` zur Laufzeit liest; dieselben Werte
/// schickt `apply_live` bei einer Änderung.
const fn live_settings(settings: &Settings) -> Live {
    Live {
        secondary: settings.secondary,
        timestamps: settings.timestamps,
        chapters: settings.chapters,
        parts: settings.parts,
        language: settings.language,
    }
}

/// Startet den Tasten-Mitleser; fällt er aus, erfährt es die Steuerung.
fn spawn_hotkey(
    events: Sender<ControlEvent>,
    escape_capture: Arc<AtomicBool>,
    key: TriggerKey,
) -> std::io::Result<Stoppable> {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let thread = thread::Builder::new()
        .name("hotkey".to_owned())
        .spawn(move || {
            if let Err(error) = hotkey::listen(&events, &escape_capture, key, &stopped) {
                // Scheitert nur, wenn die Steuerung schon beendet ist.
                match events.send(ControlEvent::HotkeyFailed(error)) {
                    Ok(()) | Err(mpsc::SendError(_)) => {}
                }
            }
        })?;
    Ok(Stoppable {
        name: "Tasten-Mitleser",
        stop,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_settings_file_asks_for_no_restart() {
        let running = Settings {
            model_file: "anderes.gguf".to_owned(),
            ..Settings::default()
        };
        assert_eq!(restart_for(None, &running), Restart::NotNeeded);
        assert_eq!(
            restart_for(Some(&SettingsEntries::default()), &running),
            Restart::Needed,
            "eine lesbare Datei mit anderem Modell braucht einen Neustart"
        );
    }

    #[test]
    fn a_panic_line_names_place_and_thread_only() {
        let location = std::panic::Location::caller();
        let line = panic_line(Some(location), Some("controller"));
        assert_eq!(
            line,
            format!(
                "Panik im Thread „controller“ an {}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        );
        assert_eq!(
            panic_line(None, None),
            "Panik im Thread „ohne Namen“ an unbekannter Stelle"
        );
    }
}
