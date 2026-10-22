//! Ablageorte und externe Werkzeuge.

use crate::note::Explain;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

/// Dateiname des Modells (Cohere Transcribe, GGUF, `Q8_0`).
pub(crate) const MODEL_FILE_NAME: &str = "cohere-transcribe-03-2026-Q8_0.gguf";

/// Name des App-Ordners unter `~/Library/Application Support`.
const APP_DIR_NAME: &str = "Diktator";

/// Orte, an denen Homebrew Programme ablegt. Eine aus dem Finder gestartete
/// App sieht den `PATH` der Shell nicht, deshalb werden sie zuerst geprüft.
const TOOL_DIRS: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];

/// Die Ordner der Werkzeuge von macOS selbst (`/bin/sh`, `ps`, `afplay` …).
const SYSTEM_DIRS: [&str; 4] = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];

/// Fehler beim Bestimmen eines Ablageorts.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PathsError {
    /// macOS liefert keinen „Application Support“-Ordner.
    #[error("der Ordner „Application Support“ wurde nicht gefunden")]
    NoDataDir,
    /// macOS nennt keinen Benutzerordner.
    #[error("der Benutzerordner wurde nicht gefunden")]
    NoHome,
}

impl Explain for PathsError {
    fn english(&self) -> String {
        match self {
            Self::NoDataDir => "the “Application Support” folder was not found",
            Self::NoHome => "the home folder was not found",
        }
        .to_owned()
    }
}

/// `~/Library/Application Support/Diktator`.
///
/// # Errors
/// [`PathsError::NoDataDir`], wenn das Betriebssystem keinen Datenordner nennt.
pub(crate) fn app_dir() -> Result<PathBuf, PathsError> {
    dirs::data_dir()
        .map(|dir| dir.join(APP_DIR_NAME))
        .ok_or(PathsError::NoDataDir)
}

/// Ordner der Modelldateien.
///
/// # Errors
/// Wie [`app_dir`].
pub(crate) fn models_dir() -> Result<PathBuf, PathsError> {
    Ok(app_dir()?.join("models"))
}

/// Ordner der Sprachmodelle fürs Überarbeiten (`models/llm/`, GGUF für
/// llama.cpp).
///
/// # Errors
/// Wie [`app_dir`].
pub(crate) fn llm_dir() -> Result<PathBuf, PathsError> {
    Ok(models_dir()?.join("llm"))
}

/// Pfad der Einstellungen (`settings.toml`).
///
/// # Errors
/// Wie [`app_dir`].
pub(crate) fn settings_path() -> Result<PathBuf, PathsError> {
    Ok(app_dir()?.join("settings.toml"))
}

/// Pfad des Wörterbuchs (`dictionary.toml`).
///
/// # Errors
/// Wie [`app_dir`].
pub(crate) fn dictionary_path() -> Result<PathBuf, PathsError> {
    Ok(app_dir()?.join("dictionary.toml"))
}

/// Das Protokoll von Diktator (`~/Library/Logs/Diktator.log`, auch in der
/// App „Konsole“ lesbar).
///
/// # Errors
/// [`PathsError::NoHome`], wenn das Betriebssystem keinen Benutzerordner nennt.
pub(crate) fn log_file() -> Result<PathBuf, PathsError> {
    logs_dir().map(|dir| dir.join("Diktator.log"))
}

/// Die Ausgabe des lokalen Sprachmodells (`~/Library/Logs/Diktator-llama.log`).
///
/// # Errors
/// Wie [`log_file`].
pub(crate) fn llama_log_file() -> Result<PathBuf, PathsError> {
    logs_dir().map(|dir| dir.join("Diktator-llama.log"))
}

/// `~/Library/Logs`.
fn logs_dir() -> Result<PathBuf, PathsError> {
    dirs::home_dir()
        .map(|home| home.join("Library/Logs"))
        .ok_or(PathsError::NoHome)
}

/// `path` zum Anzeigen und fürs Protokoll, mit `~` statt des
/// Benutzerordners: „~/Library/Application Support/Diktator/settings.toml“.
/// So nennt eine Meldung die Dateien von Diktator, ohne den Namen des
/// Benutzers.
#[must_use]
pub(crate) fn tilde(path: &Path) -> String {
    tilde_below(path, dirs::home_dir().as_deref())
}

/// Wie [`tilde`], mit `home` als Benutzerordner.
fn tilde_below(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Die GGUF-Dateien in `dir`, alphabetisch; leer, wenn der Ordner fehlt.
#[must_use]
pub(crate) fn gguf_files(dir: &Path) -> Vec<String> {
    // Auch ein unlesbarer Ordner heißt „keine Modelle“: Die Auswahl im Panel
    // ist dann leer, und wer ein Modell braucht, meldet beim Laden, dass
    // die Datei fehlt.
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut models: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
        })
        .collect();
    models.sort();
    models
}

/// Der `PATH` jedes Kindprozesses: erst die Homebrew-Ordner, dann die von
/// macOS. Fest, damit ein Kind dieselben Werkzeuge findet, ob Diktator aus
/// dem Terminal, dem Finder oder von launchd gestartet wurde
/// (processes.md).
#[must_use]
pub(crate) fn child_path() -> OsString {
    let mut path = OsString::new();
    for dir in TOOL_DIRS.iter().chain(&SYSTEM_DIRS) {
        if !path.is_empty() {
            path.push(":");
        }
        path.push(dir);
    }
    path
}

/// Pfad eines Kommandozeilenwerkzeugs wie `ffmpeg` oder `yt-dlp`: zuerst
/// in den Homebrew-Ordnern, dann im `PATH`. `None`, wenn es nicht
/// installiert ist.
#[must_use]
pub(crate) fn tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    TOOL_DIRS
        .iter()
        .map(PathBuf::from)
        .chain(std::env::split_paths(&path))
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_hides_the_home_folder() {
        let home = Path::new("/Users/anna");
        assert_eq!(
            tilde_below(
                Path::new("/Users/anna/Library/Logs/Diktator.log"),
                Some(home)
            ),
            "~/Library/Logs/Diktator.log"
        );
        assert_eq!(
            tilde_below(Path::new("/opt/models/x.gguf"), Some(home)),
            "/opt/models/x.gguf"
        );
        assert_eq!(tilde_below(Path::new("/x.gguf"), None), "/x.gguf");
    }

    #[test]
    fn gguf_files_are_sorted_and_only_gguf() {
        let dir = crate::testing::TempDir::new("gguf-files");
        for name in ["z.gguf", "b.gguf", "GROSS.GGUF", "notiz.txt", "a.gguf"] {
            dir.file(name, "");
        }
        dir.file("ordner.gguf/drin.gguf", "");
        assert_eq!(
            gguf_files(dir.path()),
            ["GROSS.GGUF", "a.gguf", "b.gguf", "z.gguf"]
        );
        assert_eq!(gguf_files(&dir.path().join("fehlt")), Vec::<String>::new());
    }
}
