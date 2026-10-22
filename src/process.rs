//! Kindprozesse: Jedes andere Programm startet über dieses Modul. Den
//! Aufruf baut [`command`] (feste Umgebung, eigene Prozessgruppe); kurze
//! Aufrufe laufen über [`run`], lange laufende über [`spawn`], dessen
//! [`Running`] sie auf jedem Weg beendet und einsammelt.
//!
//! Das Programm bekommt seine Eingabe auf einem eigenen Thread, beide
//! Ausgaben werden schon während des Laufs gelesen (ein voller Puffer würde
//! es sonst blockieren), und es wird beendet, wenn die Zeitgrenze abläuft
//! oder das Abbruch-Flag gesetzt wird. Jedes Kind startet als Anführer
//! einer eigenen Prozessgruppe (`process_group(0)`); am Ende wird immer die
//! ganze Gruppe beendet, auch nach einem Ende von selbst: Ein Enkel, der
//! die Pipes geerbt hat, hielte sonst die Lese-Threads fest, und keiner
//! läuft nach dem Aufruf weiter. Danach ist der Prozess immer eingesammelt.
//! Lange laufende Prozesse (Oberfläche, `llama-server`, die Tonspur-Kette
//! von `yt-dlp` und `ffmpeg`, das Kind des Wächters) gehören einem
//! [`Running`]; fällt es weg, endet ihre Gruppe ebenso.
//!
//! Die Umgebung ist fest: Eine aus dem Finder oder von launchd gestartete
//! App hat nur den `PATH` von macOS, ohne Homebrew; jedes Kind bekommt
//! deshalb [`paths::child_path`] als `PATH`, gleich wie Diktator gestartet
//! wurde. Den Rest der Umgebung (`HOME`, `TMPDIR`, Sprache) erbt es.

use crate::note::Explain;
use std::ffi::OsStr;
use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, Scope, ScopedJoinHandle};
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process_group};

use crate::clock::{Clock as _, SystemClock};
use crate::paths;

/// Ein Wächter um ein Programm, das nicht selbst auf seinen Elternprozess
/// achtet: `$0` mit den Argumenten `"$@"` läuft im Hintergrund, ein `cat`
/// liest die Standardeingabe (über fd 3, weil ein Hintergrundbefehl sonst
/// `/dev/null` als Eingabe bekäme) und beendet das Programm, sobald sie
/// sich schließt. Der Status der Shell ist der des Programms.
const WATCH_PARENT: &str = "exec 3<&0; \"$0\" \"$@\" 3<&- & child=$!; { cat <&3 >/dev/null; kill $child 2>/dev/null; } & wait $child";

/// Abstand zwischen zwei Blicken auf einen laufenden Prozess.
const POLL: Duration = Duration::from_millis(10);

/// Wann ein laufender Prozess beendet wird.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Limit<'a> {
    /// Nach dieser Zeit.
    After(Duration),
    /// Nach dieser Zeit oder sobald das Flag gesetzt ist.
    AfterOrCancel(Duration, &'a AtomicBool),
}

/// Ein Prozess, der von selbst geendet hat.
#[derive(Debug)]
pub(crate) struct Finished {
    /// Sein Exit-Status.
    pub status: ExitStatus,
    /// Seine Standardausgabe.
    pub stdout: Vec<u8>,
    /// Seine Standardfehlerausgabe.
    pub stderr: Vec<u8>,
}

impl Finished {
    /// Der Prozess, wenn er Erfolg gemeldet hat; sonst [`Failed`] mit
    /// seiner Begründung.
    ///
    /// # Errors
    /// [`Failed`], wenn der Exit-Status kein Erfolg ist.
    pub(crate) fn checked(self) -> Result<Self, Failed> {
        if self.status.success() {
            return Ok(self);
        }
        Err(Failed {
            reason: self.reason(),
        })
    }

    /// Letzte nicht leere Zeile der Fehlerausgabe, sonst wie das Programm
    /// endete.
    #[must_use]
    pub(crate) fn reason(&self) -> Reason {
        last_line(&self.stderr).map_or_else(|| Reason::Exit(Exit::of(self.status)), Reason::Said)
    }
}

/// Letzte nicht leere Zeile einer Ausgabe, ohne Leerraum am Rand; `None`
/// ohne Ausgabe.
#[must_use]
pub(crate) fn last_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Wie ein Programm endete, wenn es keinen Grund nannte. Aus dem
/// [`ExitStatus`], dessen Text (`exit status: 3`) nur englisch wäre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    /// Mit diesem Code.
    Code(i32),
    /// Durch dieses Signal beendet.
    Signal(i32),
}

impl Exit {
    /// Wie der Prozess mit `status` endete. Unter Unix hat jeder beendete
    /// Prozess einen Code oder ein Signal.
    #[must_use]
    pub(crate) fn of(status: ExitStatus) -> Self {
        match (status.code(), status.signal()) {
            (Some(code), _) => Self::Code(code),
            (None, Some(signal)) => Self::Signal(signal),
            (None, None) => {
                debug_assert!(
                    false,
                    "invariant: ein beendeter Prozess hat Code oder Signal"
                );
                // So zeigt es auch `ExitStatus` an: der rohe Wartestatus.
                Self::Code(status.into_raw())
            }
        }
    }
}

impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(code) => write!(f, "endete mit Code {code}"),
            Self::Signal(signal) => write!(f, "durch Signal {signal} beendet"),
        }
    }
}

impl Explain for Exit {
    fn english(&self) -> String {
        match self {
            Self::Code(code) => format!("exited with code {code}"),
            Self::Signal(signal) => format!("killed by signal {signal}"),
        }
    }
}

/// Warum ein Programm ohne Erfolg endete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reason {
    /// Was es selbst meldete (letzte Zeile seiner Fehlerausgabe, wie sie
    /// kommt, meist englisch).
    Said(String),
    /// Es meldete nichts; nur wie es endete.
    Exit(Exit),
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Said(text) => f.write_str(text),
            Self::Exit(exit) => exit.fmt(f),
        }
    }
}

impl Explain for Reason {
    fn english(&self) -> String {
        match self {
            Self::Said(text) => text.clone(),
            Self::Exit(exit) => exit.english(),
        }
    }
}

/// Das Programm lief, meldete aber keinen Erfolg ([`Finished::checked`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}")]
pub(crate) struct Failed {
    /// Seine Begründung.
    pub reason: Reason,
}

impl Explain for Failed {
    fn english(&self) -> String {
        self.reason.english()
    }
}

/// Ein Lauf, der Erfolg melden muss ([`run_checked`]).
#[derive(Debug, thiserror::Error)]
pub(crate) enum CheckedError {
    /// Der Lauf selbst scheiterte.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// Das Programm meldete einen Fehler.
    #[error(transparent)]
    Failed(#[from] Failed),
}

impl Explain for CheckedError {
    fn english(&self) -> String {
        match self {
            Self::Process(source) => source.english(),
            Self::Failed(source) => source.english(),
        }
    }
}

/// [`run`], und das Programm muss Erfolg melden.
///
/// # Errors
/// [`CheckedError`], wenn der Lauf scheitert oder das Programm einen
/// Fehler meldet.
pub(crate) fn run_checked(
    command: &mut Command,
    input: &[u8],
    limit: Limit<'_>,
) -> Result<Finished, CheckedError> {
    Ok(run(command, input, limit)?.checked()?)
}

/// Warum ein Prozess kein Ergebnis lieferte.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessError {
    /// Das Programm ließ sich nicht starten.
    #[error("nicht startbar: {0}")]
    Spawn(#[source] io::Error),
    /// Ein- oder Ausgabe schlug fehl.
    #[error("Ein- oder Ausgabe gescheitert: {0}")]
    Io(#[source] io::Error),
    /// Keine Antwort in der Zeitgrenze; der Prozess wurde beendet.
    #[error("antwortet nicht")]
    TimedOut,
    /// Das Abbruch-Flag wurde gesetzt; der Prozess wurde beendet.
    #[error("abgebrochen")]
    Cancelled,
    /// Ein Ein- oder Ausgabe-Thread ist abgestürzt.
    #[error("Ein- oder Ausgabe-Thread abgestürzt")]
    Crashed,
}

impl Explain for ProcessError {
    fn english(&self) -> String {
        match self {
            Self::Spawn(source) => format!("cannot start: {source}"),
            Self::Io(source) => format!("input or output failed: {source}"),
            Self::TimedOut => "not responding".to_owned(),
            Self::Cancelled => "cancelled".to_owned(),
            Self::Crashed => "an input or output thread crashed".to_owned(),
        }
    }
}

/// Wie das Warten endete.
enum Ended {
    /// Der Prozess endete von selbst.
    Exited(ExitStatus),
    /// Die Zeitgrenze lief ab.
    TimedOut,
    /// Das Abbruch-Flag wurde gesetzt.
    Cancelled,
}

/// Der Aufruf von `program`, die einzige Stelle, an der ein `Command`
/// entsteht: in einer eigenen Prozessgruppe (`process_group(0)`) und mit
/// [`paths::child_path`] als `PATH`. Gestartet wird er mit [`run`] oder
/// [`spawn`].
#[must_use]
#[expect(
    clippy::disallowed_methods,
    reason = "die eine Stelle, an der ein Kindprozess gebaut wird (processes.md)"
)]
pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.env("PATH", paths::child_path()).process_group(0);
    command
}

/// Wie [`command`], aber für ein lange laufendes Programm, das nicht von
/// selbst endet, wenn Diktator abstürzt (`panic = "abort"`, dann läuft kein
/// `Drop`): Eine Shell davor beendet es, sobald sich ihre Standardeingabe
/// schließt, also mit dem Ende von Diktator. Der Aufrufer muss die
/// Standardeingabe als Pipe anlegen (`Stdio::piped()`) und offen lassen;
/// [`Running`] tut das, bis es wegfällt. Weitere Argumente gehen an
/// `program`; das Programm selbst bekommt `/dev/null` als Eingabe nicht,
/// sondern erbt Standardausgabe und -fehlerausgabe.
#[must_use]
pub(crate) fn command_watching_parent(program: &Path) -> Command {
    let mut watched = command("/bin/sh");
    watched.arg("-c").arg(WATCH_PARENT).arg(program);
    watched
}

/// Startet ein lange laufendes Programm. Sein Besitzer hält das
/// [`Running`]; fällt es weg, wird die ganze Prozessgruppe beendet und das
/// Kind eingesammelt.
///
/// # Errors
/// Wenn das Programm sich nicht starten lässt.
pub(crate) fn spawn(command: &mut Command) -> io::Result<Running> {
    // `process_group(0)` noch einmal: Auch ein Aufruf, den jemand nach
    // `command` umgebaut hat, bekommt seine eigene Gruppe.
    let child = command.process_group(0).spawn()?;
    Ok(Running { child })
}

/// Ein lange laufender Kindprozess mit eigener Prozessgruppe. Fällt er weg,
/// endet die ganze Gruppe, und das Kind wird eingesammelt; auf jedem Weg,
/// auch bei einer Fehlerrückgabe. Nur bei einem Abbruch (`panic = "abort"`)
/// läuft kein `Drop`; dafür gibt es [`command_watching_parent`].
#[derive(Debug)]
pub(crate) struct Running {
    child: Child,
}

impl Running {
    /// Prozessnummer, zugleich die Nummer seiner Gruppe.
    #[must_use]
    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    /// Die Standardeingabe, falls als Pipe angelegt und noch nicht genommen.
    pub(crate) const fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    /// Die Standardausgabe, falls als Pipe angelegt und noch nicht genommen.
    pub(crate) const fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    /// Die Fehlerausgabe, falls als Pipe angelegt und noch nicht genommen.
    pub(crate) const fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    /// Der Exit-Status, falls das Kind schon geendet hat.
    ///
    /// # Errors
    /// Wenn das Betriebssystem den Zustand nicht liefert.
    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Wartet, bis das Kind endet. Schließt vorher seine Standardeingabe,
    /// falls sie noch hier liegt.
    ///
    /// # Errors
    /// Wenn das Warten scheitert.
    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }

    /// Beendet die ganze Gruppe und sammelt das Kind ein; mehrfach
    /// aufrufbar.
    pub(crate) fn stop(&mut self) {
        stop(&mut self.child);
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Beendet die Prozessgruppe `group` (die Nummer ihres Anführers, siehe
/// [`Running::id`]) mit SIGKILL, ohne ihn einzusammeln: für einen Prozess,
/// den ein anderer Thread oder ein früherer Lauf besitzt.
///
/// # Errors
/// Wenn niemand aus der Gruppe mehr läuft oder `group` keine Gruppe sein
/// kann (0 oder 1).
pub(crate) fn kill_group(group: u32) -> io::Result<()> {
    let group = group_id(group).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    Ok(kill_process_group(group, Signal::KILL)?)
}

/// Startet `command`, schreibt `input` in seine Standardeingabe und wartet,
/// bis es endet – höchstens bis `limit`. Ein Exit-Status ohne Erfolg ist
/// kein Fehler; [`Finished::checked`] macht einen daraus. Das Programm
/// läuft in einer eigenen Prozessgruppe; wenn `run` zurückkehrt, ist die
/// ganze Gruppe beendet, auch Prozesse, die es im Hintergrund gestartet hat.
///
/// # Errors
/// [`ProcessError`], wenn das Programm nicht startet, die Ein- oder Ausgabe
/// scheitert, die Zeit abläuft oder abgebrochen wird.
pub(crate) fn run(
    command: &mut Command,
    input: &[u8],
    limit: Limit<'_>,
) -> Result<Finished, ProcessError> {
    let mut child = command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(ProcessError::Spawn)?;
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    thread::scope(|scope| {
        let threads = writer(scope, stdin, input).and_then(|writer| {
            Ok((
                writer,
                reader(scope, "stdout", stdout)?,
                reader(scope, "stderr", stderr)?,
            ))
        });
        let (written, output, errors) = match threads {
            Ok(threads) => threads,
            Err(error) => {
                // Schon gestartete Threads enden mit dem Prozess.
                stop(&mut child);
                return Err(ProcessError::Io(error));
            }
        };
        let ended = wait(&mut child, limit);
        // Auf jedem Weg, auch nach einem Ende von selbst: Sonst warten die
        // Threads auf Enkel, die die Pipes geerbt haben.
        stop(&mut child);
        let (written, output, errors) = (joined(written)?, joined(output)?, joined(errors)?);
        let status = match ended.map_err(ProcessError::Io)? {
            Ended::Exited(status) => status,
            Ended::TimedOut => return Err(ProcessError::TimedOut),
            Ended::Cancelled => return Err(ProcessError::Cancelled),
        };
        match written {
            // Endet ein Programm, ohne alles zu lesen, sagen Status und
            // Fehlerausgabe, warum.
            Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
                return Err(ProcessError::Io(error));
            }
            Ok(()) | Err(_) => {}
        }
        Ok(Finished {
            status,
            stdout: output.map_err(ProcessError::Io)?,
            stderr: errors.map_err(ProcessError::Io)?,
        })
    })
}

/// Beendet einen Kindprozess samt allen Prozessen seiner Gruppe (SIGKILL)
/// und wartet auf ihn. Das Kind ist mit `process_group(0)` gestartet
/// ([`command`]). Fehler sind hier egal: Der Prozess und seine Gruppe
/// können bereits von selbst beendet sein.
///
/// Ist das Kind schon eingesammelt, gilt die Nummer der Gruppe weiter,
/// solange einer ihrer Prozesse lebt (POSIX vergibt sie bis dahin nicht neu);
/// lebt keiner mehr, trifft der Aufruf niemanden.
fn stop(child: &mut Child) {
    let group = group_id(child.id());
    debug_assert!(
        group.is_some(),
        "invariant: ein Kind hat eine Prozessnummer größer 1"
    );
    match group {
        // Scheitert nur, wenn aus der Gruppe niemand mehr läuft.
        Some(group) => match kill_process_group(group, Signal::KILL) {
            Ok(()) | Err(_) => {}
        },
        None => match child.kill() {
            Ok(()) | Err(_) => {}
        },
    }
    match child.wait() {
        Ok(_) | Err(_) => {}
    }
}

/// Die Prozessgruppe, die der Prozess `id` anführt (`process_group(0)`):
/// Ihre Nummer ist seine Prozessnummer. `None` nur für eine Nummer, die
/// kein Kind haben kann; Nummer 1 hieße bei `kill` „alle Prozesse“.
fn group_id(id: u32) -> Option<Pid> {
    // Eine Prozessnummer ist ein positives `pid_t`; was nicht passt, ist
    // keine Gruppe.
    i32::try_from(id)
        .ok()
        .and_then(Pid::from_raw)
        .filter(|group| *group != Pid::INIT)
}

/// Wartet auf das Ende, höchstens bis `limit`.
fn wait(child: &mut Child, limit: Limit<'_>) -> io::Result<Ended> {
    let (timeout, cancel) = match limit {
        Limit::After(timeout) => (timeout, None),
        Limit::AfterOrCancel(timeout, cancel) => (timeout, Some(cancel)),
    };
    let started = SystemClock.now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Ended::Exited(status));
        }
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            return Ok(Ended::Cancelled);
        }
        if SystemClock.now().duration_since(started) > timeout {
            return Ok(Ended::TimedOut);
        }
        SystemClock.sleep(POLL);
    }
}

/// Startet einen Thread, der `input` schreibt; danach fällt `stdin` weg,
/// und das Programm sieht das Ende der Eingabe.
fn writer<'scope>(
    scope: &'scope Scope<'scope, '_>,
    stdin: Option<ChildStdin>,
    input: &'scope [u8],
) -> io::Result<ScopedJoinHandle<'scope, io::Result<()>>> {
    thread::Builder::new()
        .name("process-stdin".to_owned())
        .spawn_scoped(scope, move || {
            stdin.map_or(Ok(()), |mut stdin| stdin.write_all(input))
        })
}

/// Startet einen Thread, der `stream` bis zum Ende liest (leer, wenn es
/// ihn nicht gibt); `name` steht im Namen des Threads.
///
/// # Errors
/// Wenn kein Thread gestartet werden kann.
pub(crate) fn reader<'scope, R: Read + Send + 'scope>(
    scope: &'scope Scope<'scope, '_>,
    name: &str,
    stream: Option<R>,
) -> io::Result<ScopedJoinHandle<'scope, io::Result<Vec<u8>>>> {
    thread::Builder::new()
        .name(format!("process-{name}"))
        .spawn_scoped(scope, move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                stream.read_to_end(&mut bytes)?;
            }
            Ok(bytes)
        })
}

/// Ergebnis eines Ein- oder Ausgabe-Threads.
///
/// # Errors
/// [`ProcessError::Crashed`], wenn der Thread abgestürzt ist.
pub(crate) fn joined<T>(thread: ScopedJoinHandle<'_, T>) -> Result<T, ProcessError> {
    // Der Inhalt einer Panik ist hier nichts wert; der Absturz zählt.
    thread.join().map_err(|_panic| ProcessError::Crashed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str, input: &[u8], limit: Limit<'_>) -> Result<Finished, ProcessError> {
        run(command("/bin/sh").args(["-c", script]), input, limit)
    }

    #[test]
    fn passes_long_input_and_output_through() {
        // Mehr als ein Pipe-Puffer (64 KB) in beide Richtungen.
        let input = "Wort\n".repeat(50_000);
        let finished = shell(
            "cat",
            input.as_bytes(),
            Limit::After(Duration::from_secs(10)),
        )
        .unwrap();
        assert_eq!(finished.stdout, input.as_bytes());
    }

    #[test]
    fn a_program_that_reads_nothing_keeps_its_reason() {
        // Endet, ohne die (große) Eingabe zu lesen.
        let input = vec![b'x'; 1_000_000];
        let error = shell(
            "echo 'Sprache unbekannt' >&2; exit 3",
            &input,
            Limit::After(Duration::from_secs(10)),
        )
        .unwrap()
        .checked()
        .unwrap_err();
        assert_eq!(error.to_string(), "Sprache unbekannt");
    }

    #[test]
    fn a_program_that_never_reads_runs_into_the_timeout() {
        // Schläft, statt die Eingabe zu lesen: Das Schreiben darf das Warten
        // nicht aufhalten.
        let started = SystemClock.now();
        let input = vec![b'x'; 1_000_000];
        let error =
            shell("sleep 10", &input, Limit::After(Duration::from_millis(100))).unwrap_err();
        assert!(matches!(error, ProcessError::TimedOut), "{error:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "der hängende Prozess wird nach der Frist beendet, nicht erst nach 10 s"
        );
    }

    #[test]
    fn stops_when_cancelled() {
        let cancel = AtomicBool::new(true);
        let started = SystemClock.now();
        let error = shell(
            "sleep 10",
            b"",
            Limit::AfterOrCancel(Duration::from_secs(60), &cancel),
        )
        .unwrap_err();
        assert!(matches!(error, ProcessError::Cancelled), "{error:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "abgebrochen wird sofort, nicht erst nach 10 s"
        );
    }

    #[test]
    fn a_silent_failure_names_the_status_in_both_languages() {
        let error = shell("exit 3", b"", Limit::After(Duration::from_secs(10)))
            .unwrap()
            .checked()
            .unwrap_err();
        assert_eq!(error.to_string(), "endete mit Code 3");
        assert_eq!(error.english(), "exited with code 3");
        let killed = shell("kill -KILL $$", b"", Limit::After(Duration::from_secs(10)))
            .unwrap()
            .checked()
            .unwrap_err();
        assert_eq!(killed.reason, Reason::Exit(Exit::Signal(9)));
        assert_eq!(killed.to_string(), "durch Signal 9 beendet");
    }

    #[test]
    fn last_line_skips_blank_lines() {
        assert_eq!(
            last_line(b"WARNING: x\n ERROR: weg \n\n").as_deref(),
            Some("ERROR: weg")
        );
        assert_eq!(last_line(b" \n"), None);
    }

    // `sleep 5; true`: Es folgt noch ein Befehl, also kann keine Shell sich
    // durch `sleep` ersetzen; es läuft als Enkel, der stdout und stderr
    // geerbt hat (processes.md).
    const GRANDCHILD: &str = "sleep 5; true";

    #[test]
    fn a_grandchild_holding_the_pipes_does_not_outlast_the_timeout() {
        let started = SystemClock.now();
        let error = shell(GRANDCHILD, b"", Limit::After(Duration::from_millis(100))).unwrap_err();
        assert!(matches!(error, ProcessError::TimedOut), "{error:?}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "nach {:?} zurück, nicht an der Frist von 100 ms",
            started.elapsed()
        );
    }

    #[test]
    fn cancel_also_ends_a_grandchild_holding_the_pipes() {
        let cancel = AtomicBool::new(false);
        let started = SystemClock.now();
        let error = thread::scope(|scope| {
            thread::Builder::new()
                .spawn_scoped(scope, || {
                    SystemClock.sleep(Duration::from_millis(200));
                    cancel.store(true, Ordering::Relaxed);
                })
                .unwrap();
            shell(
                GRANDCHILD,
                b"",
                Limit::AfterOrCancel(Duration::from_secs(60), &cancel),
            )
            .unwrap_err()
        });
        assert!(matches!(error, ProcessError::Cancelled), "{error:?}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "nach {:?} zurück, nicht gleich nach dem Abbruch",
            started.elapsed()
        );
    }

    #[test]
    fn a_background_grandchild_does_not_hold_up_a_finished_run() {
        // Die Shell endet sofort; `sleep` behält ihre Standardausgabe.
        let started = SystemClock.now();
        let finished = shell(
            "sleep 5 & exit 0",
            b"",
            Limit::After(Duration::from_secs(10)),
        )
        .unwrap();
        assert!(finished.status.success(), "{finished:?}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "nach {:?} zurück, obwohl die Shell sofort endete",
            started.elapsed()
        );
    }

    /// Läuft der Prozess `pid` noch (ein Zombie zählt nicht)?
    fn still_runs(pid: &str) -> bool {
        let listed = shell(
            &format!("/bin/ps -o stat= -p {pid}"),
            b"",
            Limit::After(Duration::from_secs(10)),
        )
        .unwrap();
        let state = String::from_utf8_lossy(&listed.stdout).trim().to_owned();
        !state.is_empty() && !state.starts_with('Z')
    }

    #[test]
    fn a_timeout_ends_the_whole_process_group() {
        let dir = crate::testing::TempDir::new("grandchild");
        let pid_file = dir.path().join("pid");
        // Der Enkel hält die Pipes nicht, `run` kehrt also rechtzeitig
        // zurück; die Frage ist nur, ob er danach noch läuft.
        let script = format!(
            "sleep 30 >/dev/null 2>&1 & echo $! > '{}'; wait",
            pid_file.display()
        );
        let error = shell(&script, b"", Limit::After(Duration::from_millis(300))).unwrap_err();
        assert!(matches!(error, ProcessError::TimedOut), "{error:?}");
        let pid = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .to_owned();
        SystemClock.sleep(Duration::from_millis(100));
        let running = still_runs(&pid);
        if running {
            shell(
                &format!("kill {pid}"),
                b"",
                Limit::After(Duration::from_secs(10)),
            )
            .unwrap();
        }
        assert!(!running, "Enkel {pid} läuft nach der Frist weiter");
    }

    /// Startet `script` lange laufend und liest die erste Zeile seiner
    /// Ausgabe (die Nummer eines Enkels).
    fn spawned(mut command: Command) -> (Running, String) {
        let mut running = spawn(command.stdout(Stdio::piped())).unwrap();
        let mut line = String::new();
        io::BufRead::read_line(
            &mut io::BufReader::new(running.take_stdout().unwrap()),
            &mut line,
        )
        .unwrap();
        (running, line.trim().to_owned())
    }

    /// Wartet bis zu 5 s, bis `pid` nicht mehr läuft; beendet ihn sonst.
    fn gone_soon(pid: &str) -> bool {
        let started = SystemClock.now();
        while started.elapsed() < Duration::from_secs(5) {
            if !still_runs(pid) {
                return true;
            }
            SystemClock.sleep(Duration::from_millis(50));
        }
        shell(
            &format!("kill {pid}"),
            b"",
            Limit::After(Duration::from_secs(10)),
        )
        .unwrap();
        false
    }

    #[test]
    fn stop_ends_the_whole_process_group() {
        let mut script = command("/bin/sh");
        script.args(["-c", "sleep 30 >/dev/null 2>&1 & echo $!; wait"]);
        let (mut running, pid) = spawned(script);
        running.stop();
        assert!(gone_soon(&pid), "Enkel {pid} läuft nach stop weiter");
        assert!(
            running.try_wait().unwrap().is_some(),
            "das Kind ist eingesammelt"
        );
        running.stop();
    }

    #[test]
    fn dropping_a_running_child_ends_its_group() {
        let mut script = command("/bin/sh");
        script.args(["-c", "sleep 30 >/dev/null 2>&1 & echo $!; wait"]);
        let (running, pid) = spawned(script);
        drop(running);
        assert!(
            gone_soon(&pid),
            "Enkel {pid} läuft nach dem Wegfallen weiter"
        );
    }

    #[test]
    fn a_child_gets_the_fixed_path() {
        let finished = shell("echo \"$PATH\"", b"", Limit::After(Duration::from_secs(10))).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&finished.stdout).trim_end(),
            paths::child_path().to_string_lossy()
        );
    }

    #[test]
    fn a_watched_program_ends_with_its_parent() {
        // Das Programm meldet seine Nummer und schläft; `sh -c` als
        // Programm, `exec`, damit die Nummer die von `sleep` ist.
        let mut watched = command_watching_parent(Path::new("/bin/sh"));
        watched
            .args(["-c", "echo $$; exec sleep 30"])
            .stdin(Stdio::piped());
        let (mut running, pid) = spawned(watched);
        assert!(still_runs(&pid), "das Programm läuft");
        // Wie bei einem Absturz von Diktator: Die Pipe schließt sich, ohne
        // dass jemand die Gruppe beendet.
        drop(running.take_stdin());
        assert!(
            gone_soon(&pid),
            "Programm {pid} läuft ohne Elternprozess weiter"
        );
    }

    #[test]
    fn a_watched_program_reports_its_own_status() {
        let mut watched = command_watching_parent(Path::new("/bin/sh"));
        watched.args(["-c", "exit 7"]).stdin(Stdio::piped());
        let mut running = spawn(&mut watched).unwrap();
        let started = SystemClock.now();
        let status = loop {
            if let Some(status) = running.try_wait().unwrap() {
                break status;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "der Wächter endet nicht mit dem Programm"
            );
            SystemClock.sleep(Duration::from_millis(20));
        };
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn kill_group_ends_a_group_owned_elsewhere() {
        let mut script = command("/bin/sh");
        script.args(["-c", "sleep 30 >/dev/null 2>&1 & echo $!; wait"]);
        let (mut running, pid) = spawned(script);
        kill_group(running.id()).unwrap();
        assert!(gone_soon(&pid), "Enkel {pid} läuft nach kill_group weiter");
        running.stop();
        // Nicht mit 1 prüfen: Ein Fehler hier schickte SIGKILL an alle
        // Prozesse des Nutzers.
        kill_group(0).unwrap_err();
    }
}
