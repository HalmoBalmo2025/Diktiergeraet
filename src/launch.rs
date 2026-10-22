//! Autostart beim Anmelden.
//!
//! Autostart ist ein `LaunchAgent` in `~/Library/LaunchAgents`, der beim
//! Anmelden `open -a Diktator.app` ausführt – derselbe Weg wie ein Doppelklick,
//! damit die Rechte der App gelten. Das geht nur, wenn Diktator als
//! `Diktator.app` läuft (nicht bei `cargo run`).

use crate::note::Explain;
use crate::textfile;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Kennung des `LaunchAgent`s.
const AGENT_LABEL: &str = "local.diktator";

/// Fehler beim Autostart.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LaunchError {
    /// Diktator läuft nicht aus einer `.app`.
    #[error("geht nur, wenn Diktator als Diktator.app läuft")]
    NotBundled,
    /// Der Pfad des laufenden Programms ist nicht ermittelbar.
    #[error("Programmpfad nicht ermittelbar: {0}")]
    Executable(#[source] io::Error),
    /// Kein Benutzerordner.
    #[error("Benutzerordner nicht gefunden")]
    NoHome,
    /// Die Autostart-Datei ließ sich nicht schreiben.
    #[error("Autostart nicht einrichtbar ({path}): {source}")]
    Write {
        /// Pfad der Datei.
        path: PathBuf,
        /// Ursache.
        #[source]
        source: io::Error,
    },
    /// Die Autostart-Datei ließ sich nicht löschen.
    #[error("Autostart nicht abschaltbar ({path}): {source}")]
    Remove {
        /// Pfad der Datei.
        path: PathBuf,
        /// Ursache.
        #[source]
        source: io::Error,
    },
}

impl Explain for LaunchError {
    fn english(&self) -> String {
        match self {
            Self::NotBundled => "only works when Diktator runs as Diktator.app".to_owned(),
            Self::Executable(source) => format!("cannot determine the program path: {source}"),
            Self::NoHome => "home folder not found".to_owned(),
            Self::Write { path, source } => {
                format!("cannot set up autostart ({}): {source}", path.display())
            }
            Self::Remove { path, source } => {
                format!("cannot turn off autostart ({}): {source}", path.display())
            }
        }
    }
}

/// Ob Diktator beim Anmelden startet. Zum Panel geht es als `true`
/// (ein) oder `false` (aus).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub(crate) enum Autostart {
    /// Startet beim Anmelden.
    On,
    /// Startet nicht von selbst.
    Off,
}

impl From<bool> for Autostart {
    fn from(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }
}

impl From<Autostart> for bool {
    fn from(autostart: Autostart) -> Self {
        match autostart {
            Autostart::On => true,
            Autostart::Off => false,
        }
    }
}

/// Ist der Autostart eingerichtet?
#[must_use]
pub(crate) fn autostart() -> Autostart {
    match agent_path() {
        Ok(path) if path.is_file() => Autostart::On,
        Ok(_) | Err(_) => Autostart::Off,
    }
}

/// Richtet den Autostart ein oder entfernt ihn.
///
/// # Errors
/// [`LaunchError`], wenn Diktator nicht als App läuft oder die Datei nicht
/// geschrieben bzw. gelöscht werden kann.
pub(crate) fn set_autostart(wanted: Autostart) -> Result<(), LaunchError> {
    let path = agent_path()?;
    match wanted {
        Autostart::On => {
            let plist = agent_plist(&bundle_path()?);
            let write_error = |source| LaunchError::Write {
                path: path.clone(),
                source,
            };
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).map_err(write_error)?;
            }
            textfile::replace(&path, &plist).map_err(write_error)
        }
        Autostart::Off => remove_agent(path),
    }
}

/// Entfernt die Datei für den Autostart; fehlt sie, ist nichts zu tun.
#[expect(
    clippy::disallowed_methods,
    reason = "löscht nur die eigene Datei für den Autostart, die `set_autostart` anlegt"
)]
fn remove_agent(path: PathBuf) -> Result<(), LaunchError> {
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(LaunchError::Remove { path, source }),
    }
}

/// `…/Diktator.app`, wenn das Programm darin läuft.
fn bundle_path() -> Result<PathBuf, LaunchError> {
    let executable = std::env::current_exe().map_err(LaunchError::Executable)?;
    // …/Diktator.app/Contents/MacOS/diktator
    executable
        .ancestors()
        .nth(3)
        .filter(|bundle| {
            bundle
                .extension()
                .is_some_and(|extension| extension == "app")
        })
        .map(Path::to_path_buf)
        .ok_or(LaunchError::NotBundled)
}

/// `~/Library/LaunchAgents/local.diktator.plist`.
fn agent_path() -> Result<PathBuf, LaunchError> {
    dirs::home_dir()
        .map(|home| {
            home.join("Library/LaunchAgents")
                .join(format!("{AGENT_LABEL}.plist"))
        })
        .ok_or(LaunchError::NoHome)
}

/// Inhalt des `LaunchAgent`s.
fn agent_plist(bundle: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/open</string>
        <string>-a</string>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
        xml_escape(&bundle.to_string_lossy())
    )
}

/// Maskiert die XML-Sonderzeichen.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_points_to_bundle() {
        let plist = agent_plist(Path::new("/Users/a & b/Applications/Diktator.app"));
        assert!(
            plist.contains("<string>/Users/a &amp; b/Applications/Diktator.app</string>"),
            "der Pfad zum Bundle steht maskiert in der Plist"
        );
        assert!(
            plist.contains("<string>local.diktator</string>"),
            "die Plist trägt die Kennung des Startagenten"
        );
    }
}
