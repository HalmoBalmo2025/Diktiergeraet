//! Rückmeldung an den Nutzer: Systemklänge, Mitteilungen und Protokoll.
//!
//! Klänge und Mitteilungen laufen über Kommandozeilenwerkzeuge von macOS
//! (`afplay`, `osascript`) auf einem eigenen Thread, damit weder die Tasten
//! noch das Einfügen auf einen Klang warten müssen. Das Protokoll steht in
//! `~/Library/Logs/Diktator.log` (auch in der App „Konsole“ lesbar) und
//! zusätzlich auf stderr, wenn Diktator aus dem Terminal läuft.

use crate::note::{Explain, Note, UiLanguage};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use jiff::Zoned;
use serde::{Deserialize, Serialize};

use crate::paths::{self, PathsError};
use crate::process::{self, CheckedError, Finished, Limit};
use crate::textfile;

/// Länger darf ein Klang nicht dauern (alle sind kürzer als eine Sekunde).
const SOUND_TIMEOUT: Duration = Duration::from_secs(10);

/// Länger darf das Zeigen einer Mitteilung nicht dauern.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(10);

/// Lautstärke der Klänge in Prozent, 0 bis 100. In der Datei und vom
/// Panel kommt sie als Zahl und wird beim Lesen geprüft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub(crate) struct Volume(u8);

impl Volume {
    /// Leise, damit das Mikrofon den Startklang kaum aufnimmt (Standard).
    pub(crate) const QUIET: Self = Self(25);

    /// Die Prozent.
    #[must_use]
    pub(crate) const fn percent(self) -> u8 {
        self.0
    }

    /// Wert für `afplay -v` (0 bis 1).
    fn afplay_value(self) -> String {
        match self.0 {
            100 => "1".to_owned(),
            percent => format!("0.{percent:02}"),
        }
    }
}

impl TryFrom<u8> for Volume {
    type Error = VolumeError;

    /// Nur 0 bis 100 Prozent.
    fn try_from(percent: u8) -> Result<Self, Self::Error> {
        match percent {
            0..=100 => Ok(Self(percent)),
            other => Err(VolumeError(u16::from(other))),
        }
    }
}

impl From<Volume> for u8 {
    fn from(volume: Volume) -> Self {
        volume.0
    }
}

/// Lautstärke über 100 %.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("lautstaerke = {0}, erlaubt sind 0 bis 100")]
pub(crate) struct VolumeError(pub(crate) u16);

impl Explain for VolumeError {
    fn english(&self) -> String {
        format!("lautstaerke = {}, allowed are 0 to 100", self.0)
    }
}

/// Ob Klänge gespielt werden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sounds {
    /// Keine Klänge.
    Off,
    /// Klänge in dieser Lautstärke.
    On(Volume),
}

impl Default for Sounds {
    fn default() -> Self {
        Self::On(Volume::QUIET)
    }
}

/// Ob das Protokoll geschrieben wird. In Einstellungsdatei und Panel
/// `true` (an) oder `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub(crate) enum Logging {
    /// Zeilen kommen in `~/Library/Logs/Diktator.log`.
    On,
    /// Keine Datei; Zeilen gehen nur nach stderr.
    Off,
}

impl From<bool> for Logging {
    fn from(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }
}

impl From<Logging> for bool {
    fn from(logging: Logging) -> Self {
        match logging {
            Logging::On => true,
            Logging::Off => false,
        }
    }
}

/// Ein Systemklang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sound {
    /// Aufnahme läuft.
    Start,
    /// Aufnahme beendet, Verarbeitung beginnt.
    Stop,
    /// Abgebrochen oder nichts erkannt.
    Nothing,
    /// Ein längerer Auftrag (`YouTube`) ist fertig.
    Done,
    /// Etwas ging schief oder ist gerade nicht möglich.
    Error,
}

impl Sound {
    /// Datei unter `/System/Library/Sounds`.
    const fn file(self) -> &'static str {
        match self {
            Self::Start => "/System/Library/Sounds/Tink.aiff",
            Self::Stop => "/System/Library/Sounds/Pop.aiff",
            Self::Nothing => "/System/Library/Sounds/Funk.aiff",
            Self::Done => "/System/Library/Sounds/Glass.aiff",
            Self::Error => "/System/Library/Sounds/Basso.aiff",
        }
    }
}

/// Ein Auftrag an den Rückmelde-Thread.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    /// Klang abspielen, wenn Klänge an sind.
    Play(Sound),
    /// Ab jetzt so (oder gar nicht) abspielen.
    Sounds(Sounds),
    /// Ab jetzt Mitteilungen in dieser Sprache.
    Language(UiLanguage),
    /// Mitteilung in der Mitteilungszentrale zeigen.
    Notify(Note),
    /// Enden, nachdem alles davor erledigt ist.
    Stop,
}

/// Warum eine Zeile nicht ins Protokoll kam oder es sich nicht leeren ließ.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LogError {
    /// Es gibt keinen Ort fürs Protokoll (kein Benutzerordner).
    #[error("Protokoll: {0}")]
    Paths(#[from] PathsError),
    /// Die Datei ließ sich nicht schreiben.
    #[error("Protokoll nicht schreibbar: {0}")]
    Io(#[from] io::Error),
}

impl Explain for LogError {
    fn english(&self) -> String {
        match self {
            Self::Paths(source) => format!("log: {}", source.english()),
            Self::Io(source) => format!("log not writable: {source}"),
        }
    }
}

/// Fehler beim Starten des Rückmelde-Threads.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FeedbackError {
    /// Der Thread ließ sich nicht starten.
    #[error("Rückmelde-Thread: {0}")]
    Spawn(#[from] io::Error),
}

impl Explain for FeedbackError {
    fn english(&self) -> String {
        match self {
            Self::Spawn(source) => format!("feedback thread: {source}"),
        }
    }
}

/// Griff zum Rückmelde-Thread. Wird er fallen gelassen, endet der Thread,
/// sobald alle Aufträge erledigt sind.
#[derive(Debug, Clone)]
pub(crate) struct Feedback {
    sender: Sender<Request>,
    /// Protokoll an? Geteilt, damit der Schalter im Panel sofort wirkt.
    logging: Arc<AtomicBool>,
}

impl Feedback {
    /// Für Tests: ohne Klänge, Mitteilungen und Protokolldatei.
    #[cfg(test)]
    pub(crate) fn silent() -> Self {
        let (sender, _) = mpsc::channel();
        Self {
            sender,
            logging: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Startet den Rückmelde-Thread; `sounds` sagt, ob und wie laut Klänge
    /// gespielt werden, `logging`, ob das Protokoll geschrieben wird,
    /// `language`, in welcher Sprache Mitteilungen erscheinen.
    ///
    /// # Errors
    /// [`FeedbackError::Spawn`], wenn kein Thread gestartet werden kann.
    pub(crate) fn spawn(
        sounds: Sounds,
        logging: Logging,
        language: UiLanguage,
    ) -> Result<(Self, FeedbackThread), FeedbackError> {
        let (sender, receiver) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("feedback".to_owned())
            .spawn(move || serve(&receiver, sounds, language))?;
        let logging = Arc::new(AtomicBool::new(logging == Logging::On));
        let thread = FeedbackThread {
            sender: sender.clone(),
            thread: Some(handle),
        };
        Ok((Self { sender, logging }, thread))
    }

    /// Spielt einen Klang ab, wenn Klänge eingeschaltet sind.
    pub(crate) fn play(&self, sound: Sound) {
        self.send(Request::Play(sound));
    }

    /// Schaltet Klänge ein oder aus bzw. ändert die Lautstärke (gilt sofort).
    pub(crate) fn set_sounds(&self, sounds: Sounds) {
        self.send(Request::Sounds(sounds));
    }

    /// Zeigt eine Mitteilung in der Sprache der Oberfläche (und schreibt
    /// sie auf Deutsch ins Protokoll).
    pub(crate) fn notify(&self, message: Note) {
        self.log(format!("Mitteilung: {message}"));
        self.send(Request::Notify(message));
    }

    /// Mitteilungen ab jetzt in `language` (gilt sofort).
    pub(crate) fn set_language(&self, language: UiLanguage) {
        self.send(Request::Language(language));
    }

    /// Schreibt eine Zeile ins Protokoll – sofort, auf dem aufrufenden
    /// Thread, damit die Uhrzeit stimmt und nicht hinter Klängen wartet.
    pub(crate) fn log(&self, line: impl Into<String>) {
        let line = line.into();
        if !self.logging.load(Ordering::Relaxed) {
            eprintln!("{line}");
            return;
        }
        if let Err(error) = append_log(&line) {
            eprintln!("{error}");
        }
    }

    /// Ob das Protokoll gerade geschrieben wird.
    #[must_use]
    pub(crate) fn logging(&self) -> Logging {
        if self.logging.load(Ordering::Relaxed) {
            Logging::On
        } else {
            Logging::Off
        }
    }

    /// Schaltet das Protokoll ein oder aus (gilt sofort für alle Threads).
    pub(crate) fn set_logging(&self, logging: Logging) {
        self.logging
            .store(logging == Logging::On, Ordering::Relaxed);
    }

    /// Reicht einen Auftrag weiter.
    fn send(&self, request: Request) {
        // Scheitert nur, wenn der Thread schon beendet ist (Programmende);
        // dann gibt es niemanden mehr, dem etwas zu melden wäre.
        match self.sender.send(request) {
            Ok(()) | Err(mpsc::SendError(_)) => {}
        }
    }
}

/// Der Rückmelde-Thread. [`FeedbackThread::stop`] (oder das Wegfallen)
/// lässt ihn erst alles Wartende erledigen (eine letzte Mitteilung geht
/// nicht verloren), dann enden, und wartet auf ihn. Er endet als Letzter:
/// Alle anderen Threads melden über ihn.
#[derive(Debug)]
pub(crate) struct FeedbackThread {
    sender: Sender<Request>,
    /// `None`, sobald beendet.
    thread: Option<JoinHandle<()>>,
}

impl FeedbackThread {
    /// Beendet den Thread nach den wartenden Aufträgen; mehrfach aufrufbar.
    pub(crate) fn stop(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        // Scheitert nur, wenn der Thread schon beendet ist.
        match self.sender.send(Request::Stop) {
            Ok(()) | Err(mpsc::SendError(_)) => {}
        }
        if thread.join().is_err() {
            eprintln!("Rückmelde-Thread ist abgestürzt");
        }
    }
}

impl Drop for FeedbackThread {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Arbeitet Aufträge ab, bis [`Request::Stop`] kommt oder alle Griffe
/// fallen gelassen sind; spielt
/// Klänge nach `sounds` und zeigt Mitteilungen in `language`, bis eine neue
/// Einstellung kommt.
fn serve(receiver: &Receiver<Request>, mut sounds: Sounds, mut language: UiLanguage) {
    for request in receiver {
        let outcome = match &request {
            Request::Sounds(new) => {
                sounds = *new;
                continue;
            }
            Request::Language(new) => {
                language = *new;
                continue;
            }
            Request::Play(sound) => match sounds {
                Sounds::On(volume) => play(*sound, volume),
                Sounds::Off => continue,
            },
            Request::Notify(message) => notify(message.text(language)),
            Request::Stop => return,
        };
        if let Err(error) = outcome {
            eprintln!("Rückmeldung {request:?} fehlgeschlagen: {error}");
        }
    }
}

/// Spielt einen Klang und wartet, bis er vorbei ist.
fn play(sound: Sound, volume: Volume) -> Result<Finished, CheckedError> {
    process::run_checked(
        process::command("/usr/bin/afplay").args(["-v", &volume.afplay_value(), sound.file()]),
        b"",
        Limit::After(SOUND_TIMEOUT),
    )
}

/// Zeigt eine Mitteilung sofort, ohne Rückmelde-Thread (für den Wächter).
///
/// # Errors
/// [`CheckedError`], wenn `osascript` nicht startet, scheitert oder hängt.
pub(crate) fn show_notification(message: &str) -> Result<Finished, CheckedError> {
    notify(message)
}

/// Schreibt eine Zeile ins Protokoll, ohne Rückmelde-Thread (für den
/// Wächter).
///
/// # Errors
/// [`LogError`], wenn es keinen Ort fürs Protokoll gibt oder die Datei
/// nicht schreibbar ist.
pub(crate) fn write_log_line(line: &str) -> Result<(), LogError> {
    append_log(line)
}

/// Zeigt eine Mitteilung. Der Text geht als Argument an das Skript und wird
/// deshalb nie als `AppleScript` ausgewertet.
fn notify(message: &str) -> Result<Finished, CheckedError> {
    process::run_checked(
        process::command("/usr/bin/osascript").args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 1 of argv) with title \"Diktator\"",
            "-e",
            "end run",
            message,
        ]),
        b"",
        Limit::After(NOTIFY_TIMEOUT),
    )
}

/// Leert das Protokoll.
///
/// # Errors
/// [`LogError`], wenn es keinen Ort fürs Protokoll gibt oder die Datei
/// nicht geschrieben werden kann.
pub(crate) fn clear_log() -> Result<(), LogError> {
    Ok(textfile::replace(&paths::log_file()?, "")?)
}

/// Hängt eine Zeile mit Datum und Ortszeit an das Protokoll an (und
/// schreibt sie auf stderr).
fn append_log(line: &str) -> Result<(), LogError> {
    let stamped = format!("{} {line}\n", clock(&crate::clock::local_time()));
    eprint!("{stamped}");
    append_line(&paths::log_file()?, &stamped)?;
    Ok(())
}

/// Hängt `line` (samt Zeilenende) an die Datei `path` an. In einem einzigen
/// Schreibaufruf: Mit `O_APPEND` bleibt er ganz, auch wenn andere Threads
/// oder der Wächter gleichzeitig schreiben (`writeln!` schriebe Text und
/// Zeilenende getrennt, und Zeilen liefen ineinander).
#[expect(
    clippy::disallowed_methods,
    reason = "hängt nur an das eigene Protokoll an (`append`), kürzt und überschreibt nie"
)]
fn append_line(path: &Path, line: &str) -> io::Result<()> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

/// Datum und Uhrzeit für das Protokoll, in der Zeitzone von `time`
/// („2026-10-14 09:05:03“).
fn clock(time: &Zoned) -> String {
    time.strftime("%Y-%m-%d %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_goes_to_afplay_as_a_fraction() {
        let value = |percent: u8| Volume::try_from(percent).unwrap().afplay_value();
        assert_eq!(value(100), "1");
        assert_eq!(value(50), "0.50");
        assert_eq!(value(5), "0.05");
        assert_eq!(value(0), "0.00");
    }

    #[test]
    fn stop_ends_the_thread() {
        let (feedback, mut thread) =
            Feedback::spawn(Sounds::Off, Logging::Off, UiLanguage::German).unwrap();
        thread.stop();
        assert!(
            feedback.sender.send(Request::Play(Sound::Done)).is_err(),
            "der Rückmelde-Thread läuft nach stop weiter"
        );
        thread.stop();
    }
    use jiff::civil::date;
    use jiff::tz::TimeZone;

    #[test]
    fn log_lines_from_several_threads_stay_whole() {
        let dir = crate::testing::TempDir::new("log-threads");
        let path = dir.path().join("Diktator.log");
        let write_lines = |writer: usize| {
            for line in 0..3000 {
                append_line(&path, &format!("Schreiber {writer} Zeile {line}\n")).unwrap();
            }
        };
        thread::scope(|scope| {
            for writer in 0..4 {
                thread::Builder::new()
                    .spawn_scoped(scope, move || write_lines(writer))
                    .unwrap();
            }
        });
        let log = std::fs::read_to_string(&path).unwrap();
        let broken: Vec<&str> = log
            .lines()
            .filter(|line| {
                !(line.starts_with("Schreiber") && line.matches("Schreiber").count() == 1)
            })
            .collect();
        assert_eq!(log.lines().count(), 12_000);
        assert!(
            broken.is_empty(),
            "{} Zeilen ineinandergelaufen oder leer, z. B. {:?}",
            broken.len(),
            broken.iter().take(3).collect::<Vec<_>>()
        );
    }

    #[test]
    fn clock_shows_date_and_time() {
        let time = date(2026, 10, 14)
            .at(9, 5, 3, 0)
            .to_zoned(TimeZone::fixed(jiff::tz::offset(2)))
            .unwrap();
        assert_eq!(clock(&time), "2026-10-14 09:05:03");
    }
}
