//! Lokales Sprachmodell über llama.cpp.
//!
//! Für „Überarbeiten“ mit einem lokalen Modell startet Diktator das Programm
//! `llama-server` (aus `brew install llama.cpp`) selbst: mit einer GGUF-Datei
//! aus `models/llm/`, nur auf dem eigenen Mac erreichbar (`127.0.0.1`, ein
//! freier Anschluss) und ohne Netz (`--offline`).
//!
//! Den Server besitzt der Überarbeiten-Thread; er beendet ihn
//! nach einer Pause ohne Auftrag und beim Wechsel zu Apple Intelligence.
//! Beim Beenden von Diktator endet erst der Überarbeiten-Thread (mit ihm
//! der Server), dann beendet der Hauptthread über [`ServerPid`], was noch
//! läuft; danach startet kein Server mehr. Der Server läuft hinter einem
//! Wächter ([`process::command_watching_parent`]) in einer eigenen
//! Prozessgruppe: Stürzt Diktator ab, endet er mit ihm. Bleibt trotzdem
//! einer übrig, beendet ihn der nächste Start (Nummer der Gruppe in
//! `llama-server.pid`, siehe [`clean_up_stale`]).

use crate::note::{Explain, Note};
use std::fs::{self, File};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::clock::{Clock as _, SystemClock};
use crate::paths;
use crate::process::{self, Exit, Limit, Reason, Running};
use crate::textfile;
use serde::Serialize;

/// Name des Programms (Homebrew-Paket `llama.cpp`).
const PROGRAM: &str = "llama-server";

/// Größe des Zusammenhangs in Tokens: reicht für Anweisung, ein langes
/// Diktat und dessen Überarbeitung.
const CONTEXT_TOKENS: &str = "8192";

/// So lange darf das Laden des Modells dauern.
const LOAD_TIMEOUT: Duration = Duration::from_secs(180);

/// Abstand zwischen zwei Fragen, ob das Modell geladen ist.
const HEALTH_POLL: Duration = Duration::from_millis(250);

/// So lange darf eine solche Frage dauern.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(1);

/// So lange darf `ps` dauern.
const TOOL_TIMEOUT: Duration = Duration::from_secs(5);

/// Warum das lokale Modell nicht bereitsteht.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LlamaError {
    /// `llama-server` ist nicht installiert.
    #[error("llama.cpp fehlt (im Terminal: brew install llama.cpp)")]
    MissingProgram,
    /// Die Modelldatei gibt es nicht.
    #[error("Modelldatei {} fehlt", paths::tilde(.0))]
    MissingModel(PathBuf),
    /// Das Programm ließ sich nicht starten.
    #[error("llama-server nicht startbar: {0}")]
    Start(#[from] std::io::Error),
    /// Das Programm hat sich gleich wieder beendet; letzte Zeile seines
    /// Protokolls, ohne die, wie es endete.
    #[error("llama-server hat sich beendet: {0}")]
    Exited(Reason),
    /// Das Modell war nicht rechtzeitig geladen.
    #[error("llama-server hat das Modell nicht in {} s geladen", LOAD_TIMEOUT.as_secs())]
    TimedOut,
    /// Der Start mit diesem Modell ist vorhin gescheitert, aus diesem
    /// Grund; bis zum nächsten Anlass (neue Aufnahme, Übersetzung,
    /// Modellwechsel) wird es nicht erneut versucht.
    #[error("llama-server ließ sich vorhin nicht starten: {0}")]
    FailedBefore(Note),
    /// Diktator wird beendet; es startet kein Server mehr.
    #[error("Diktator wird beendet")]
    Closed,
}

impl Explain for LlamaError {
    fn english(&self) -> String {
        match self {
            Self::MissingProgram => {
                "llama.cpp is missing (in Terminal: brew install llama.cpp)".to_owned()
            }
            Self::MissingModel(path) => format!("model file {} is missing", paths::tilde(path)),
            Self::Start(source) => format!("cannot start llama-server: {source}"),
            Self::Exited(reason) => format!("llama-server has exited: {}", reason.english()),
            Self::TimedOut => format!(
                "llama-server did not load the model within {} s",
                LOAD_TIMEOUT.as_secs()
            ),
            Self::FailedBefore(reason) => {
                format!("llama-server could not be started earlier: {}", reason.en())
            }
            Self::Closed => "Diktator is quitting".to_owned(),
        }
    }
}

/// Ob llama.cpp installiert ist (nötig für ein lokales Modell). Zum Panel
/// geht es als `true` (installiert) oder `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(into = "bool")]
pub(crate) enum Installed {
    /// `llama-server` ist im Suchpfad.
    Yes,
    /// Nicht gefunden.
    No,
}

impl From<Installed> for bool {
    fn from(installed: Installed) -> Self {
        match installed {
            Installed::Yes => true,
            Installed::No => false,
        }
    }
}

/// Ist llama.cpp installiert?
#[must_use]
pub(crate) fn installed() -> Installed {
    match paths::tool(PROGRAM) {
        Some(_) => Installed::Yes,
        None => Installed::No,
    }
}

/// Eine Modelldatei in `models/llm/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelFile {
    /// Dateiname.
    pub file: String,
    /// Lesbarer Name ([`display_name`]).
    pub name: String,
    /// Größe in Bytes; etwa so viel Speicher belegt das Modell geladen.
    pub bytes: u64,
}

/// Die Sprachmodelle (`*.gguf`) in `models/llm/`, alphabetisch.
/// Zusatzdateien für Bilder (`mmproj-…`) und Vorhersage (`mtp-…`) zählen
/// nicht.
#[must_use]
pub(crate) fn model_files() -> Vec<ModelFile> {
    let Ok(dir) = paths::llm_dir() else {
        return Vec::new();
    };
    paths::gguf_files(&dir)
        .into_iter()
        .filter(|file| !file.starts_with("mmproj") && !file.starts_with("mtp"))
        .map(|file| ModelFile {
            name: display_name(&file),
            bytes: fs::metadata(dir.join(&file)).map_or(0, |metadata| metadata.len()),
            file,
        })
        .collect()
}

/// Lesbarer Name zu einer Modelldatei:
/// „gemma-4-12B-it-qat-UD-Q4_K_XL.gguf“ → „Gemma 4 12B“.
#[must_use]
pub(crate) fn display_name(file: &str) -> String {
    let stem = file.strip_suffix(".gguf").unwrap_or(file);
    let words: Vec<&str> = stem
        .split(['-', '_', ' '])
        .take_while(|word| !is_suffix_word(word))
        .collect();
    let name = if words.is_empty() {
        stem.to_owned()
    } else {
        words.join(" ")
    };
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// Gehört das Wort schon zur Angabe von Variante oder Stufe („it“, „Q4“)?
fn is_suffix_word(word: &str) -> bool {
    let lower = word.to_lowercase();
    matches!(
        lower.as_str(),
        "it" | "instruct" | "chat" | "qat" | "ud" | "gguf" | "bf16" | "f16" | "f32"
    ) || (lower.starts_with('q') && lower.chars().nth(1).is_some_and(|c| c.is_ascii_digit()))
        || (lower.starts_with("iq") && lower.chars().nth(2).is_some_and(|c| c.is_ascii_digit()))
}

/// Nummer der Prozessgruppe des laufenden Servers, geteilt zwischen dem
/// Überarbeiten-Thread (setzt sie) und dem Hauptthread (beendet beim
/// Programmende, was nach dem Ende des Threads noch läuft).
///
/// Nach [`ServerPid::kill`] startet kein Server mehr: Ein Auftrag, der noch
/// läuft, bekommt [`LlamaError::Closed`].
#[derive(Debug)]
pub(crate) struct ServerPid {
    /// Nummer der Gruppe; 0, solange keiner läuft (so kann sie atomar
    /// zwischen den Threads wandern).
    group: Arc<AtomicU32>,
    /// Gesetzt von [`ServerPid::kill`].
    closed: Arc<AtomicBool>,
    /// Wo die Nummer für den nächsten Start nach einem Absturz steht
    /// ([`pid_file`]); `None` ohne Datei (Tests).
    file: Option<PathBuf>,
}

impl ServerPid {
    /// Noch ohne Server; die Nummer jedes gestarteten steht in `file`.
    #[must_use]
    pub(crate) fn new(file: Option<PathBuf>) -> Self {
        Self {
            group: Arc::default(),
            closed: Arc::default(),
            file,
        }
    }

    /// Ein zweiter Griff auf dieselbe Nummer.
    #[must_use]
    pub(crate) fn share(&self) -> Self {
        Self {
            group: Arc::clone(&self.group),
            closed: Arc::clone(&self.closed),
            file: self.file.clone(),
        }
    }

    /// Beendet den Server samt seinem Wächter, falls einer läuft; danach
    /// startet keiner mehr.
    pub(crate) fn kill(&self) {
        self.closed.store(true, Ordering::Relaxed);
        let group = self.group.swap(0, Ordering::Relaxed);
        if group != 0 {
            // Scheitert nur, wenn er schon beendet ist.
            match process::kill_group(group) {
                Ok(()) | Err(_) => {}
            }
        }
        remove_pid_file(self.file.as_deref());
    }

    /// Merkt sich die Nummer eines gestarteten Servers, auch für den
    /// nächsten Start nach einem Absturz.
    fn set(&self, pid: u32) {
        self.group.store(pid, Ordering::Relaxed);
        if let Some(file) = &self.file {
            write_pid_file(file, pid);
        }
    }

    /// Der Server läuft nicht mehr.
    fn clear(&self) {
        self.group.store(0, Ordering::Relaxed);
        remove_pid_file(self.file.as_deref());
    }
}

/// Höchstens ein laufender `llama-server`; gehört dem Überarbeiten-Thread.
/// Fällt er weg, wird der Server beendet.
#[derive(Debug)]
pub(crate) struct Llama {
    running: Option<Server>,
    pid: ServerPid,
    /// Das Modell, mit dem der letzte Start scheiterte: Jedes Stück einer
    /// Übersetzung würde es sonst erneut versuchen, bis zu
    /// `LOAD_TIMEOUT` lang.
    failed: Option<(PathBuf, Note)>,
}

/// Ein gestarteter Server; fällt er weg, endet der Prozess.
#[derive(Debug)]
struct Server {
    child: Running,
    model: PathBuf,
    url: String,
    /// Ende des letzten Auftrags.
    last_used: Instant,
}

impl Llama {
    /// Noch ohne Server; `pid` erfährt die Nummer jedes gestarteten.
    pub(crate) const fn new(pid: ServerPid) -> Self {
        Self {
            running: None,
            pid,
            failed: None,
        }
    }

    /// Der nächste Aufruf von [`Llama::url_for`] versucht den Start wieder,
    /// auch wenn er vorhin scheiterte.
    pub(crate) fn retry(&mut self) {
        self.failed = None;
    }

    /// Adresse der Schnittstelle (bis `/v1`) für `model`; startet den
    /// Server, falls nötig, und wartet, bis das Modell geladen ist (beim
    /// ersten Mal einige Sekunden). Danach [`Llama::finished`] aufrufen.
    pub(crate) fn url_for(&mut self, model: &Path) -> Result<String, LlamaError> {
        if let Some(running) = self.running.as_mut()
            && running.model == model
            && matches!(running.child.try_wait(), Ok(None))
        {
            return Ok(running.url.clone());
        }
        if let Some((failed, reason)) = &self.failed
            && failed == model
        {
            return Err(LlamaError::FailedBefore(reason.clone()));
        }
        if self.pid.closed.load(Ordering::Relaxed) {
            return Err(LlamaError::Closed);
        }
        self.stop();
        let running = start(model, &self.pid).inspect_err(|error| {
            self.failed = Some((model.to_owned(), error.note()));
        })?;
        let url = running.url.clone();
        self.running = Some(running);
        Ok(url)
    }

    /// Der Auftrag ist erledigt; ab jetzt läuft die Pause bis zum Entladen.
    pub(crate) fn finished(&mut self) {
        if let Some(running) = self.running.as_mut() {
            running.last_used = SystemClock.now();
        }
    }

    /// Ende des letzten Auftrags; `None`, wenn kein Server läuft.
    pub(crate) fn last_used(&self) -> Option<Instant> {
        self.running.as_ref().map(|running| running.last_used)
    }

    /// Beendet den Server, falls einer läuft.
    pub(crate) fn stop(&mut self) {
        if let Some(mut running) = self.running.take() {
            running.child.stop();
            self.pid.clear();
        }
    }
}

impl Drop for Llama {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Beendet einen Server, den ein früherer Lauf von Diktator hinterlassen
/// hat (etwa nach einem Absturz): nur die Gruppe aus der PID-Datei, nur wenn
/// dort ein llama-server läuft und ihr Elternprozess weg ist. Den Server
/// einer laufenden zweiten Instanz lässt es in Ruhe.
pub(crate) fn clean_up_stale() {
    if let Some(path) = pid_file() {
        clean_up_stale_at(&path);
    }
}

/// Wie [`clean_up_stale`], mit der Nummer in `path`.
fn clean_up_stale_at(path: &Path) {
    let Some(pid) = fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
    else {
        return;
    };
    let listed = process::run(
        process::command("/bin/ps").args(["-A", "-o", "pid=,ppid=,pgid=,comm="]),
        b"",
        Limit::After(TOOL_TIMEOUT),
    );
    let Ok(listed) = listed else {
        // Scheitert `ps`, lässt sich nichts prüfen: nichts beenden, die
        // Datei für den nächsten Start behalten.
        return;
    };
    match group_state(&String::from_utf8_lossy(&listed.stdout), pid) {
        Group::Stale => {
            // Scheitert nur, wenn er inzwischen beendet ist.
            match process::kill_group(pid) {
                Ok(()) | Err(_) => {}
            }
            remove_pid_file(Some(path));
        }
        // Dort läuft etwas anderes oder nichts mehr: Die Datei ist veraltet.
        Group::Gone => remove_pid_file(Some(path)),
        // Der Server einer anderen, laufenden Instanz: Er und seine Datei
        // bleiben.
        Group::Owned => {}
    }
}

/// Was in einer Prozessgruppe aus der PID-Datei läuft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    /// Kein llama-server darin.
    Gone,
    /// Ein llama-server, dessen Gruppe einen lebenden Elternprozess hat
    /// (eine andere Instanz von Diktator).
    Owned,
    /// Ein llama-server, dessen Elternprozess weg ist (übernommen von
    /// launchd, Nummer 1): übrig von einem Absturz.
    Stale,
}

/// Der Zustand der Gruppe `group` laut `listed` (Zeilen „pid ppid pgid
/// Programm“ von `ps -o pid=,ppid=,pgid=,comm=`). Elternprozess der Gruppe
/// ist, wer einen ihrer Prozesse gestartet hat, ohne selbst dazuzugehören.
fn group_state(listed: &str, group: u32) -> Group {
    let processes: Vec<Listed> = listed.lines().filter_map(Listed::parse).collect();
    let members: Vec<&Listed> = processes
        .iter()
        .filter(|process| process.group == group)
        .collect();
    if !members
        .iter()
        .any(|process| process.program.ends_with(PROGRAM))
    {
        return Group::Gone;
    }
    let orphaned = members
        .iter()
        .filter(|process| !members.iter().any(|other| other.id == process.parent))
        .all(|process| process.parent == 1);
    if orphaned { Group::Stale } else { Group::Owned }
}

/// Eine Zeile von `ps -o pid=,ppid=,pgid=,comm=`.
#[derive(Debug)]
struct Listed {
    id: u32,
    parent: u32,
    group: u32,
    program: String,
}

impl Listed {
    /// Liest eine Zeile; `None`, wenn sie nicht so aussieht.
    fn parse(line: &str) -> Option<Self> {
        let mut fields = line.split_whitespace();
        Some(Self {
            id: fields.next()?.parse().ok()?,
            parent: fields.next()?.parse().ok()?,
            group: fields.next()?.parse().ok()?,
            program: fields.collect::<Vec<_>>().join(" "),
        })
    }
}

/// Startet `llama-server` mit `model` und wartet, bis es antwortet.
fn start(model: &Path, pid: &ServerPid) -> Result<Server, LlamaError> {
    let program = paths::tool(PROGRAM).ok_or(LlamaError::MissingProgram)?;
    if !model.is_file() {
        return Err(LlamaError::MissingModel(model.to_owned()));
    }
    let port = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
    // Für die Fehlersuche; wird bei jedem Start neu geschrieben.
    let log_path = paths::llama_log_file().ok();
    let (stdout, stderr) = log_output(log_path.as_deref());
    // Hinter dem Wächter: Schließt sich seine Standardeingabe (Diktator
    // ist weg, auch nach einem Absturz), beendet er den Server.
    let mut child = process::spawn(
        process::command_watching_parent(&program)
            .arg("--model")
            .arg(model)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .args(["--ctx-size", CONTEXT_TOKENS, "--parallel", "1"])
            // Kein Nachdenken vor der Antwort: kostet nur Zeit.
            .args(["--reasoning-budget", "0", "--no-webui", "--offline"])
            .stdin(Stdio::piped())
            .stdout(stdout)
            .stderr(stderr),
    )?;
    pid.set(child.id());
    let health = format!("http://127.0.0.1:{port}/health");
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(HEALTH_TIMEOUT))
        .build()
        .new_agent();
    let started = SystemClock.now();
    loop {
        let exited = match child.try_wait() {
            Ok(exited) => exited,
            Err(error) => {
                child.stop();
                pid.clear();
                return Err(error.into());
            }
        };
        if let Some(status) = exited {
            pid.clear();
            let reason = log_path
                .as_deref()
                .and_then(last_log_line)
                .map_or_else(|| Reason::Exit(Exit::of(status)), Reason::Said);
            return Err(LlamaError::Exited(reason));
        }
        if agent.get(&health).call().is_ok() {
            break;
        }
        if SystemClock.now().duration_since(started) > LOAD_TIMEOUT {
            child.stop();
            pid.clear();
            return Err(LlamaError::TimedOut);
        }
        SystemClock.sleep(HEALTH_POLL);
    }
    Ok(Server {
        child,
        model: model.to_owned(),
        url: format!("http://127.0.0.1:{port}/v1"),
        last_used: SystemClock.now(),
    })
}

/// Standardausgabe und -fehler des Servers: das Server-Protokoll in `path`,
/// ohne Protokolldatei nirgendwohin. Es dient nur der Fehlersuche, also
/// startet der Server auch, wenn es sich nicht anlegen lässt.
#[expect(
    clippy::disallowed_methods,
    reason = "das eigene Server-Protokoll im App-Ordner wird bei jedem Start neu geschrieben"
)]
fn log_output(path: Option<&Path>) -> (Stdio, Stdio) {
    let log = path.and_then(|path| File::create(path).ok());
    log.map_or_else(
        || (Stdio::null(), Stdio::null()),
        |file| {
            let stdout = file.try_clone().map_or_else(|_| Stdio::null(), Stdio::from);
            (stdout, Stdio::from(file))
        },
    )
}

/// Letzte nicht leere Zeile des Server-Protokolls.
fn last_log_line(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Datei mit der Prozessnummer des laufenden Servers.
#[must_use]
pub(crate) fn pid_file() -> Option<PathBuf> {
    paths::app_dir()
        .ok()
        .map(|dir| dir.join("llama-server.pid"))
}

/// Merkt sich die Prozessnummer für den nächsten Start.
fn write_pid_file(path: &Path, pid: u32) {
    // Ohne Datei räumt nur ein Absturz nicht auf.
    match textfile::replace(path, &pid.to_string()) {
        Ok(()) | Err(_) => {}
    }
}

/// Löscht die Datei mit der Prozessnummer.
#[expect(
    clippy::disallowed_methods,
    reason = "löscht nur die eigene Datei mit der Prozessnummer des Servers"
)]
fn remove_pid_file(path: Option<&Path>) {
    if let Some(path) = path {
        // Fehlt sie, ist nichts zu tun.
        match fs::remove_file(path) {
            Ok(()) | Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    /// Wartet bis zu 5 s, bis `running` endet.
    fn ends_soon(running: &mut Running) -> bool {
        let started = SystemClock.now();
        while started.elapsed() < Duration::from_secs(5) {
            if running.try_wait().unwrap().is_some() {
                return true;
            }
            SystemClock.sleep(Duration::from_millis(20));
        }
        false
    }

    /// Ein Stellvertreter, der 30 s schläft.
    fn sleeper(program: &Path) -> Running {
        process::spawn(process::command(program).arg("30")).unwrap()
    }

    #[test]
    fn kill_ends_the_server() {
        let pid = ServerPid::new(None);
        let mut server = sleeper(Path::new("/bin/sleep"));
        pid.set(server.id());
        pid.kill();
        assert!(ends_soon(&mut server), "der Server läuft nach kill weiter");
    }

    #[test]
    fn stop_ends_the_running_server() {
        let pid = ServerPid::new(None);
        let mut llama = Llama::new(pid.share());
        let child = sleeper(Path::new("/bin/sleep"));
        let group = child.id();
        pid.set(group);
        llama.running = Some(Server {
            child,
            model: PathBuf::from("/m.gguf"),
            url: "http://127.0.0.1:1/v1".to_owned(),
            last_used: SystemClock.now(),
        });
        llama.stop();
        assert!(llama.last_used().is_none(), "kein Server mehr");
        process::kill_group(group).unwrap_err();
    }

    #[test]
    fn clean_up_leaves_other_programs_and_living_instances() {
        // Eigener Name: `textfile` räumt `diktator-stale-…` im selben
        // Testprogramm weg.
        let temp = TempDir::new("llama-stale");
        let dir = temp.path();
        // Ein Stellvertreter, der für `ps` „llama-server“ heißt. Ein Link,
        // keine Kopie: Eine Kopie von `/bin/sleep` beendet macOS sofort mit
        // SIGKILL, weil ihre Signatur dort nicht mehr gilt.
        let program = dir.join(PROGRAM);
        std::os::unix::fs::symlink("/bin/sleep", &program).unwrap();
        let mut other = sleeper(Path::new("/bin/sleep"));
        let pid_file = temp.file("llama-server.pid", other.id().to_string());
        clean_up_stale_at(&pid_file);
        // Ein SIGKILL käme sofort an; so lange darf es dauern.
        SystemClock.sleep(Duration::from_millis(300));
        assert!(
            other.try_wait().unwrap().is_none(),
            "ein anderes Programm unter der Nummer bleibt"
        );
        assert!(!pid_file.exists(), "die Datei ist weg");
        other.stop();

        // Ein llama-server, dessen Elternprozess (dieser Test, wie eine
        // zweite laufende Instanz) lebt: bleibt, samt Datei. Ob ein
        // verwaister beendet wird, prüft `only_an_orphaned_server_counts_as_stale`.
        let mut owned = sleeper(&program);
        // `ps` muss den Prozess schon unter seinem Namen kennen.
        SystemClock.sleep(Duration::from_millis(100));
        temp.file("llama-server.pid", owned.id().to_string());
        clean_up_stale_at(&pid_file);
        SystemClock.sleep(Duration::from_millis(300));
        assert!(
            owned.try_wait().unwrap().is_none(),
            "der Server einer laufenden Instanz bleibt"
        );
        assert!(pid_file.exists(), "und seine Datei auch");
        owned.stop();
    }

    #[test]
    fn no_server_starts_after_kill() {
        let pid = ServerPid::new(None);
        let mut llama = Llama::new(pid.share());
        pid.kill();
        let error = llama.url_for(Path::new("/gibt/es/nicht.gguf")).unwrap_err();
        assert!(matches!(error, LlamaError::Closed), "{error:?}");
    }

    #[test]
    fn only_an_orphaned_server_counts_as_stale() {
        // pid ppid pgid comm
        let listed = "    1     0     1 /sbin/launchd\n\
                        500     1   500 /Applications/Diktator.app/Contents/MacOS/diktator\n\
                        812   500   812 /bin/sh\n\
                        813   812   812 /opt/homebrew/bin/llama-server\n\
                        900     1   900 /bin/sh\n\
                        901   900   900 /opt/homebrew/bin/llama-server\n\
                        950     1   950 llama-server\n\
                        960     1   960 /bin/sleep\n";
        assert_eq!(group_state(listed, 812), Group::Owned, "Diktator lebt noch");
        assert_eq!(
            group_state(listed, 900),
            Group::Stale,
            "Wächter-Shell verwaist"
        );
        assert_eq!(
            group_state(listed, 950),
            Group::Stale,
            "Server allein verwaist"
        );
        assert_eq!(group_state(listed, 960), Group::Gone, "kein llama-server");
        assert_eq!(group_state(listed, 81), Group::Gone, "Gruppe gibt es nicht");
        assert_eq!(group_state("", 812), Group::Gone, "ohne Liste keiner");
    }

    #[test]
    fn names_model_files() {
        assert_eq!(
            display_name("gemma-4-12B-it-qat-UD-Q4_K_XL.gguf"),
            "Gemma 4 12B"
        );
        assert_eq!(display_name("gemma-4-12B-it-Q4_0.gguf"), "Gemma 4 12B");
        assert_eq!(
            display_name("Mistral-Small-3.2-24B-Instruct-2506-Q4_K_M.gguf"),
            "Mistral Small 3.2 24B"
        );
        assert_eq!(display_name("Qwen3.8-27B-IQ4_XS.gguf"), "Qwen3.8 27B");
        assert_eq!(display_name("modell.gguf"), "Modell");
    }

    #[test]
    fn missing_model_is_reported() {
        if installed() == Installed::Yes {
            let mut llama = Llama::new(ServerPid::new(None));
            let error = llama.url_for(Path::new("/nicht/da.gguf")).unwrap_err();
            assert!(matches!(error, LlamaError::MissingModel(_)), "{error:?}");
        }
    }

    #[test]
    fn a_failed_start_is_not_repeated_until_retry() {
        let mut llama = Llama::new(ServerPid::new(None));
        let model = Path::new("/gibt/es/nicht.gguf");
        let first = llama.url_for(model).unwrap_err();
        assert!(
            !matches!(first, LlamaError::FailedBefore(_)),
            "der erste Versuch scheitert mit seinem eigenen Grund: {first}"
        );
        let again = llama.url_for(model).unwrap_err();
        assert!(
            matches!(&again, LlamaError::FailedBefore(reason) if reason.de() == first.to_string()),
            "ohne neuen Anlass kein zweiter Startversuch, mit dem ersten Grund: {again}"
        );
        llama.retry();
        let retried = llama.url_for(model).unwrap_err();
        assert!(
            !matches!(retried, LlamaError::FailedBefore(_)),
            "nach retry wird wieder gestartet: {retried}"
        );
    }
}
