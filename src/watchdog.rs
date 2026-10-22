//! Der Wächter: Er startet Diktator als Kindprozess und startet es neu,
//! wenn es abstürzt.
//!
//! Im Release-Build beendet jede Panik das Programm sofort (`panic =
//! "abort"`). Damit Diktator dann nicht einfach weg ist, läuft die
//! eigentliche App in einem Kindprozess ([`CHILD_ARGUMENT`]); der Wächter
//! selbst tut nichts, als zu warten. Endet das Kind normal („Beenden“),
//! endet auch der Wächter; mit [`RESTART_CODE`] („Jetzt neu starten“)
//! startet er es sofort neu; nach einem Absturz nach einer Sekunde. Stürzt
//! es zu oft kurz hintereinander ab ([`Crashes`]), gibt er auf und sagt
//! Bescheid. Konnte das Kind gar nicht starten ([`START_FAILED_CODE`]),
//! hilft ein Neustart nicht: Der Wächter meldet es und endet.
//!
//! Das Kind hängt an einer Pipe des Wächters (Standardeingabe). Wird der
//! Wächter beendet (Abmelden, „Sofort beenden“), schließt sie sich, und das
//! Kind beendet sich auch ([`watch_parent`]).
//!
//! rostfrei sieht hier launchd vor (`KeepAlive`); Diktator behält vorerst
//! diesen Wächter (CLAUDE.md, Projektnotizen).

use crate::clock::Clock;
use crate::note::Note;
use crate::process;
use std::collections::VecDeque;
use std::io::{self, Read};
use std::os::fd::AsFd;
use std::os::unix::process::ExitStatusExt as _;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;

/// Argument, mit dem der Wächter das Kind startet.
pub(crate) const CHILD_ARGUMENT: &str = "--kind";

/// Exit-Code des Kindes für „bitte neu starten“ (`EX_TEMPFAIL`).
pub(crate) const RESTART_CODE: u8 = 75;

/// Exit-Code des Kindes, wenn es gar nicht erst starten konnte
/// (`EX_CONFIG`); der Grund steht im Protokoll.
pub(crate) const START_FAILED_CODE: u8 = 78;

/// So viele Abstürze in [`CRASH_WINDOW`], dann gibt der Wächter auf.
const MAX_CRASHES: usize = 3;

/// Zeitraum, in dem Abstürze gezählt werden.
const CRASH_WINDOW: Duration = Duration::from_secs(120);

/// Pause vor dem Neustart nach einem Absturz.
const CRASH_PAUSE: Duration = Duration::from_secs(1);

/// Wie der Wächter endet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ending {
    /// Diktator wurde beendet.
    Quit,
    /// Diktator ist zu oft abgestürzt.
    GaveUp,
    /// Diktator konnte nicht starten.
    StartFailed,
}

/// Wie ein Lauf des Kindes endete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exit {
    /// „Beenden“, oder von außen mit SIGTERM beendet.
    Quit,
    /// „Jetzt neu starten“.
    Restart,
    /// Start gescheitert; ein Neustart scheiterte genauso.
    StartFailed,
    /// Abgestürzt (Signal, Panik, unerwarteter Code).
    Crashed,
}

/// SIGTERM: Jemand hat das Kind beenden lassen (Aktivitätsanzeige
/// „Beenden“, `kill`); das ist kein Absturz.
const SIGTERM: i32 = 15;

impl Exit {
    /// Was der Exit-Status des Kindes bedeutet.
    fn of(status: ExitStatus) -> Self {
        if status.signal() == Some(SIGTERM) {
            return Self::Quit;
        }
        match status.code() {
            Some(0) => Self::Quit,
            Some(code) if code == i32::from(RESTART_CODE) => Self::Restart,
            Some(code) if code == i32::from(START_FAILED_CODE) => Self::StartFailed,
            Some(_) | None => Self::Crashed,
        }
    }
}

/// Die Abstürze der letzten [`CRASH_WINDOW`]: Wann der Wächter aufgibt.
#[derive(Debug, Default)]
struct Crashes {
    recent: VecDeque<Instant>,
}

/// Was nach einem Absturz geschieht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Nach [`CRASH_PAUSE`] neu starten.
    Restart,
    /// Aufgeben: [`MAX_CRASHES`] Abstürze in [`CRASH_WINDOW`].
    GiveUp,
}

impl Crashes {
    /// Ein Absturz um `now`.
    fn record(&mut self, now: Instant) -> Verdict {
        self.recent
            .retain(|crash| now.saturating_duration_since(*crash) < CRASH_WINDOW);
        self.recent.push_back(now);
        if self.recent.len() >= MAX_CRASHES {
            Verdict::GiveUp
        } else {
            Verdict::Restart
        }
    }
}

/// Startet `executable` mit [`CHILD_ARGUMENT`] und startet es neu, bis es
/// normal endet, zu oft abstürzt oder nicht starten kann. Abstürze gehen an
/// `log`, das Aufgeben zusätzlich an `notify`; gewartet wird mit `clock`.
///
/// # Errors
/// Wenn das Kind sich nicht starten lässt oder nicht abwarten.
pub(crate) fn supervise(
    executable: &Path,
    clock: &impl Clock,
    log: impl Fn(&str),
    notify: impl Fn(&Note),
) -> io::Result<Ending> {
    let mut crashes = Crashes::default();
    loop {
        let status = run_child(executable)?;
        match Exit::of(status) {
            Exit::Quit => return Ok(Ending::Quit),
            Exit::Restart => continue,
            Exit::StartFailed => {
                let message = Note::new(
                    "Diktator konnte nicht starten. Details im Protokoll.",
                    "Diktator could not start. Details in the log.",
                );
                log(message.de());
                notify(&message);
                return Ok(Ending::StartFailed);
            }
            Exit::Crashed => {}
        }
        match crashes.record(clock.now()) {
            Verdict::Restart => {
                log(&format!("Diktator ist abgestürzt ({status}); Neustart"));
                clock.sleep(CRASH_PAUSE);
            }
            Verdict::GiveUp => {
                log(&format!("Diktator ist abgestürzt ({status})"));
                let message = Note::new(
                    "Diktator ist mehrmals kurz hintereinander abgestürzt und bleibt aus. \
                     Details im Protokoll.",
                    "Diktator crashed several times in a row and stays off. Details in the log.",
                );
                log(message.de());
                notify(&message);
                return Ok(Ending::GaveUp);
            }
        }
    }
}

/// Startet das Kind und wartet, bis es endet. Die Pipe an seiner
/// Standardeingabe bleibt so lange offen. Scheitert das Warten, beendet
/// `child` beim Wegfallen das Kind; sonst ist es schon eingesammelt.
fn run_child(executable: &Path) -> io::Result<ExitStatus> {
    // Eigene Prozessgruppe wie jedes Kind (processes.md); das Kind endet
    // trotzdem mit dem Wächter, weil es dessen Pipe liest.
    let mut child = process::spawn(
        process::command(executable)
            .arg(CHILD_ARGUMENT)
            .stdin(Stdio::piped()),
    )?;
    let pipe = child.take_stdin();
    let status = child.wait();
    drop(pipe);
    status
}

/// So oft sieht die Wache nach, ob sie enden soll.
const STOP_CHECK: Duration = Duration::from_millis(250);

/// Die Wache des Kindes über den Wächter. [`ParentWatch::stop`] (oder das
/// Wegfallen) beendet ihren Thread und wartet auf ihn.
#[derive(Debug)]
pub(crate) struct ParentWatch {
    stop: Arc<AtomicBool>,
    /// `None`, sobald beendet.
    thread: Option<JoinHandle<()>>,
}

impl ParentWatch {
    /// Beendet die Wache (spätestens nach [`STOP_CHECK`]); mehrfach
    /// aufrufbar.
    pub(crate) fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("Wache über den Wächter ist abgestürzt");
        }
    }
}

impl Drop for ParentWatch {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Für das Kind: ruft `on_parent_gone` auf, wenn der Wächter endet (die
/// Standardeingabe schließt sich), außer die Wache wurde vorher beendet.
///
/// # Errors
/// Wenn kein Thread gestartet werden kann.
pub(crate) fn watch_parent(
    on_parent_gone: impl FnOnce() + Send + 'static,
) -> io::Result<ParentWatch> {
    watch(io::stdin(), on_parent_gone)
}

/// Wie [`watch_parent`], für die Pipe `input`.
fn watch(
    input: impl Read + AsFd + Send + 'static,
    on_gone: impl FnOnce() + Send + 'static,
) -> io::Result<ParentWatch> {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let thread = thread::Builder::new()
        .name("watchdog".to_owned())
        .spawn(move || {
            if read_until_closed(input, &stopped) == Watched::Closed {
                on_gone();
            }
        })?;
    Ok(ParentWatch {
        stop,
        thread: Some(thread),
    })
}

/// Wie das Lesen der Pipe endete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Watched {
    /// Die Pipe ist zu (oder nicht mehr lesbar, was dasselbe heißt).
    Closed,
    /// Die Wache wurde beendet.
    Stopped,
}

/// Liest `input`, bis es sich schließt oder `stop` gesetzt wird. Wartet
/// dazwischen mit `poll` höchstens [`STOP_CHECK`], damit `stop` gesehen
/// wird.
fn read_until_closed(mut input: impl Read + AsFd, stop: &AtomicBool) -> Watched {
    // Eine Viertelsekunde passt immer in ein `timespec`; sonst warten,
    // bis die Pipe sich rührt, und `stop` erst danach sehen.
    let timeout = Timespec::try_from(STOP_CHECK).ok();
    let mut buffer = [0_u8; 64];
    while !stop.load(Ordering::Relaxed) {
        let mut fds = [PollFd::new(&input, PollFlags::IN)];
        match poll(&mut fds, timeout.as_ref()) {
            // Nichts in der Frist, oder unterbrochen: noch einmal.
            Ok(0) | Err(Errno::INTR) => continue,
            Ok(_) => {}
            Err(_) => return Watched::Closed,
        }
        match input.read(&mut buffer) {
            Ok(0) | Err(_) => return Watched::Closed,
            Ok(_) => {}
        }
    }
    Watched::Stopped
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::clock::FakeClock;
    use crate::testing::TempDir;
    use std::cell::RefCell;

    /// Was der Wächter mit einem „Kind“ tat.
    struct Supervised {
        ending: Ending,
        /// So oft lief das Kind.
        runs: usize,
        /// Die Zeilen fürs Protokoll.
        lines: Vec<String>,
        /// Die Mitteilungen.
        notes: Vec<String>,
    }

    /// Lässt den Wächter ein Skript als „Kind“ betreuen; `body` sieht in
    /// `$runs`, der wievielte Lauf es ist.
    fn supervise_script(name: &str, body: &str) -> Supervised {
        let dir = TempDir::new(&format!("watchdog-{name}"));
        let counter = dir.path().join("runs");
        let script = dir.script(
            "kind.sh",
            &format!(
                "echo x >> '{counter}'\nruns=$(wc -l < '{counter}')\n{body}\n",
                counter = counter.display()
            ),
        );
        let lines = RefCell::new(Vec::new());
        let notes = RefCell::new(Vec::new());
        let ending = supervise(
            &script,
            &FakeClock::new(),
            |line: &str| lines.borrow_mut().push(line.to_owned()),
            |note: &Note| notes.borrow_mut().push(note.de().to_owned()),
        )
        .unwrap();
        let runs = std::fs::read_to_string(&counter).unwrap().lines().count();
        Supervised {
            ending,
            runs,
            lines: lines.into_inner(),
            notes: notes.into_inner(),
        }
    }

    #[test]
    fn restarts_after_a_crash_and_stops_after_quit() {
        // Stürzt beim ersten Lauf ab (Signal), fordert beim zweiten einen
        // Neustart an und endet beim dritten.
        let supervised = supervise_script(
            "restart",
            &format!(
                "[ $runs -eq 1 ] && kill -ABRT $$\n[ $runs -eq 2 ] && exit {RESTART_CODE}\nexit 0"
            ),
        );
        assert_eq!(supervised.ending, Ending::Quit);
        assert_eq!(supervised.runs, 3);
        assert!(supervised.notes.is_empty(), "{:?}", supervised.notes);
    }

    #[test]
    fn gives_up_after_three_crashes_in_a_row() {
        let supervised = supervise_script("give-up", "kill -ABRT $$");
        assert_eq!(supervised.ending, Ending::GaveUp);
        assert_eq!(supervised.runs, MAX_CRASHES);
        assert_eq!(supervised.notes.len(), 1, "{:?}", supervised.notes);
    }

    #[test]
    fn the_log_announces_only_restarts_that_happen() {
        let supervised = supervise_script("log", "kill -ABRT $$");
        let announced = supervised
            .lines
            .iter()
            .filter(|line| line.ends_with("Neustart"))
            .count();
        assert_eq!(
            announced,
            supervised.runs - 1,
            "Protokoll: {:?}",
            supervised.lines
        );
    }

    #[test]
    fn a_failed_start_is_reported_and_not_repeated() {
        let supervised = supervise_script("start-failed", &format!("exit {START_FAILED_CODE}"));
        assert_eq!(supervised.ending, Ending::StartFailed);
        assert_eq!(supervised.runs, 1, "ein Neustart hilft nicht");
        assert_eq!(supervised.notes.len(), 1, "{:?}", supervised.notes);
    }

    #[test]
    fn a_child_terminated_from_outside_is_not_restarted() {
        let supervised = supervise_script("sigterm", "kill -TERM $$");
        assert_eq!(supervised.ending, Ending::Quit);
        assert_eq!(supervised.runs, 1, "SIGTERM ist kein Absturz");
    }

    #[test]
    fn crashes_count_only_within_the_window() {
        let clock = FakeClock::new();
        let mut crashes = Crashes::default();
        assert_eq!(crashes.record(clock.now()), Verdict::Restart);
        clock.advance(CRASH_WINDOW);
        // Der erste liegt jetzt außerhalb des Fensters.
        assert_eq!(crashes.record(clock.now()), Verdict::Restart);
        clock.advance(Duration::from_secs(1));
        assert_eq!(crashes.record(clock.now()), Verdict::Restart);
        clock.advance(Duration::from_secs(1));
        assert_eq!(crashes.record(clock.now()), Verdict::GiveUp);
    }

    #[test]
    fn a_closed_pipe_means_the_parent_is_gone() {
        let (reader, writer) = io::pipe().unwrap();
        let (gone, noticed) = std::sync::mpsc::channel();
        let mut watch = watch(reader, move || gone.send(()).unwrap()).unwrap();
        drop(writer);
        noticed.recv_timeout(Duration::from_secs(5)).unwrap();
        watch.stop();
    }

    #[test]
    fn a_stopped_watch_ends_without_calling_back() {
        let (reader, writer) = io::pipe().unwrap();
        let (gone, noticed) = std::sync::mpsc::channel();
        let mut watch = watch(reader, move || gone.send(()).unwrap()).unwrap();
        let started = crate::clock::SystemClock.now();
        watch.stop();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "die Wache endet bald nach stop"
        );
        noticed.try_recv().unwrap_err();
        drop(writer);
    }
}
