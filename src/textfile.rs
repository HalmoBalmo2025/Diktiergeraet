//! Textdateien, die der Nutzer selbst bearbeitet (Wörterbuch,
//! Einstellungen): beim ersten Bedarf mit Erklärungen anlegen, ersetzen
//! und im Standard-Texteditor öffnen.

use crate::note::Explain;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::paths;
use crate::process::{self, CheckedError, Limit};

/// Länger darf das Öffnen im Editor nicht dauern.
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// Fehler beim Anlegen oder Öffnen einer solchen Datei.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TextFileError {
    /// Ordner oder Datei ließen sich nicht anlegen.
    #[error("{} nicht anlegbar: {source}", paths::tilde(.path))]
    Create {
        /// Pfad der Datei.
        path: PathBuf,
        /// Ursache.
        #[source]
        source: io::Error,
    },
    /// Der Editor ließ sich nicht öffnen.
    #[error("Editor für {} nicht zu öffnen: {source}", paths::tilde(.path))]
    Open {
        /// Pfad der Datei.
        path: PathBuf,
        /// Ursache (auch die Meldung von `open`).
        #[source]
        source: CheckedError,
    },
}

impl Explain for TextFileError {
    fn english(&self) -> String {
        match self {
            Self::Create { path, source } => {
                format!("cannot create {}: {source}", paths::tilde(path))
            }
            Self::Open { path, source } => format!(
                "cannot open an editor for {}: {}",
                paths::tilde(path),
                source.english()
            ),
        }
    }
}

/// Ein Lesefehler von toml: Zeile, Spalte und was toml bemängelt, aber
/// ohne den Ausschnitt der Datei, den toml sonst zeigt. Fehler landen im
/// Protokoll, und Einstellungen und Wörterbuch können Namen von Mandanten
/// enthalten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TomlError {
    /// Wo der Fehler steht, wenn toml es sagt.
    pub position: Option<Position>,
    /// Was toml bemängelt.
    pub message: String,
}

/// Eine Stelle in einer Datei.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Position {
    /// Zeile, ab 1.
    pub line: usize,
    /// Spalte in Zeichen, ab 1.
    pub column: usize,
}

impl TomlError {
    /// Der Fehler von toml beim Lesen von `text`.
    #[must_use]
    pub(crate) fn new(text: &str, error: &toml::de::Error) -> Self {
        let position = error
            .span()
            .and_then(|span| text.get(..span.start))
            .map(|before| {
                // Ohne Umbruch davor steht der Fehler in der ersten Zeile.
                let line_start = before.rsplit_once('\n').map_or(before, |(_, line)| line);
                Position {
                    line: before.matches('\n').count() + 1,
                    column: line_start.chars().count() + 1,
                }
            });
        Self {
            position,
            message: error.message().to_owned(),
        }
    }

    /// „ (Zeile 2, Spalte 5): Meldung“.
    #[must_use]
    pub(crate) fn german(&self) -> String {
        let at = self
            .position
            .map_or_else(String::new, |Position { line, column }| {
                format!(" (Zeile {line}, Spalte {column})")
            });
        format!("{at}: {}", self.message)
    }

    /// „ (line 2, column 5): message“.
    #[must_use]
    pub(crate) fn english(&self) -> String {
        let at = self
            .position
            .map_or_else(String::new, |Position { line, column }| {
                format!(" (line {line}, column {column})")
            });
        format!("{at}: {}", self.message)
    }
}

/// Legt die Datei mit `template` an, falls sie fehlt. `true`, wenn sie neu
/// angelegt wurde.
///
/// # Errors
/// [`TextFileError::Create`], wenn Ordner oder Datei nicht anlegbar sind.
pub(crate) fn ensure(path: &Path, template: &str) -> Result<bool, TextFileError> {
    let create_error = |source| TextFileError::Create {
        path: path.to_owned(),
        source,
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(create_error)?;
    }
    // `create_new`: Eine Datei, die es inzwischen gibt, bleibt unangetastet.
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            file.write_all(template.as_bytes()).map_err(create_error)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(create_error(error)),
    }
}

/// Legt die Datei `path` neu mit `text` an, ohne je eine vorhandene zu
/// überschreiben (`create_new`). Scheitert das Schreiben (volle Platte),
/// wird die halb geschriebene Datei wieder entfernt: Sie entstand eben
/// erst hier.
///
/// # Errors
/// [`io::ErrorKind::AlreadyExists`], wenn es die Datei schon gibt; sonst
/// der Fehler beim Anlegen oder Schreiben.
#[expect(
    clippy::disallowed_methods,
    reason = "entfernt nur die eben hier angelegte, halb geschriebene Datei"
)]
pub(crate) fn write_new(path: &Path, text: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let written = file.write_all(text.as_bytes());
    if written.is_err() {
        drop(file);
        // Gemeldet wird der Fehler beim Schreiben; scheitert auch das
        // Entfernen, bleibt nur ein Rest, den der Nutzer sehen kann.
        match fs::remove_file(path) {
            Ok(()) | Err(_) => {}
        }
    }
    written
}

/// Anfang der Kennzeile, die das Format einer solchen Datei nennt
/// (`# Dateiformat / file format: 2`). Ein Kommentar und kein Schlüssel,
/// damit auch ältere Diktator-Versionen die Datei lesen: Sie lehnen
/// unbekannte Schlüssel ab, Kommentare überlesen sie.
const FORMAT_MARK: &str = "# Dateiformat / file format:";

/// Welche Fassung des Formats eine Datei hat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    /// Eine ältere: ohne Kennzeile (vor ihrer Einführung geschrieben) oder
    /// mit einer kleineren Nummer.
    Legacy,
    /// Die, die diese Version schreibt.
    Current,
}

/// Die Kennzeile steht da, nennt aber kein bekanntes Format.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum FormatError {
    /// Hinter der Kennzeile steht keine Zahl.
    #[error("die Zeile „{FORMAT_MARK}“ nennt keine Zahl: {0}")]
    Malformed(#[source] std::num::ParseIntError),
    /// Eine Nummer, die diese Version nicht kennt – vermutlich hat eine
    /// neuere Version die Datei geschrieben. Sie bleibt, wie sie ist.
    #[error(
        "Dateiformat {found}, diese Version von Diktator kennt 1 bis {current} (vermutlich von einer neueren Version); die Datei bleibt, wie sie ist"
    )]
    Unknown {
        /// Die Nummer in der Datei.
        found: u16,
        /// Die Nummer, die diese Version schreibt.
        current: u16,
    },
}

impl Explain for FormatError {
    fn english(&self) -> String {
        match self {
            Self::Malformed(source) => {
                format!("the line “{FORMAT_MARK}” names no number: {source}")
            }
            Self::Unknown { found, current } => format!(
                "file format {found}, this version of Diktator knows 1 to {current} (probably written by a newer version); the file stays as it is"
            ),
        }
    }
}

/// Die Kennzeile für Format `current`, mit Zeilenende; steht als erste
/// Zeile in der Datei.
#[must_use]
pub(crate) fn format_line(current: u16) -> String {
    format!("{FORMAT_MARK} {current}\n")
}

/// Das Format von `text`, nach der Kennzeile im Kopf (den Kommentar- und
/// Leerzeilen vor dem ersten Eintrag; weiter unten zählt sie nicht).
/// Ohne Kennzeile und mit 1 bis `current - 1` [`Format::Legacy`].
///
/// # Errors
/// [`FormatError`], wenn die Kennzeile keine Zahl nennt, 0 oder eine
/// größere als `current`.
pub(crate) fn format_of(text: &str, current: u16) -> Result<Format, FormatError> {
    let mark = text
        .lines()
        .map(str::trim)
        .take_while(|line| line.is_empty() || line.starts_with('#'))
        .find_map(|line| line.strip_prefix(FORMAT_MARK));
    let Some(number) = mark else {
        return Ok(Format::Legacy);
    };
    let found: u16 = number.trim().parse().map_err(FormatError::Malformed)?;
    match found.cmp(&current) {
        std::cmp::Ordering::Equal => Ok(Format::Current),
        std::cmp::Ordering::Less if found > 0 => Ok(Format::Legacy),
        std::cmp::Ordering::Less | std::cmp::Ordering::Greater => {
            Err(FormatError::Unknown { found, current })
        }
    }
}

/// Endung der neuen Datei, die [`replace`] neben die alte schreibt.
const FRESH_SUFFIX: &str = ".neu";

/// Ersetzt den Inhalt der Datei. Erst wird daneben eine neue Datei
/// geschrieben und auf die Platte gebracht (`sync_all`), dann umbenannt:
/// Bricht das Schreiben ab oder fällt der Strom aus, bleibt die alte Datei
/// ganz. Scheitert ein Schritt, wird die neue Datei wieder entfernt.
///
/// Ist `path` eine symbolische Verknüpfung (etwa in einen Dotfiles-Ordner),
/// wird die Datei ersetzt, auf die sie zeigt; die Verknüpfung bleibt.
/// Fehlt die Datei, wird sie angelegt.
///
/// # Errors
/// Wenn `path` keinen Dateinamen hat ([`io::ErrorKind::InvalidInput`]),
/// sich nicht auflösen lässt, oder die neue Datei nicht geschrieben oder
/// nicht umbenannt werden kann.
#[expect(
    clippy::disallowed_methods,
    reason = "das eine atomare Ersetzen: benennt die eigene neue Datei um und entfernt sie bei einem Fehler"
)]
pub(crate) fn replace(path: &Path, text: &str) -> io::Result<()> {
    let target = resolve(path)?;
    let fresh = fresh_path(&target)?;
    let written = write_synced(&fresh, text).and_then(|()| fs::rename(&fresh, &target));
    if written.is_err() {
        // Der eigentliche Fehler wird gemeldet; dass auch das Aufräumen
        // scheitert (die Datei entstand gar nicht), ändert daran nichts.
        match fs::remove_file(&fresh) {
            Ok(()) | Err(_) => {}
        }
    }
    written
}

/// Entfernt eine neue Datei, die ein abgebrochener Lauf von [`replace`]
/// neben `path` (oder dem Ziel seiner Verknüpfung) hinterlassen hat. Nur
/// beim Start aufrufen, wenn sonst niemand schreibt.
///
/// # Errors
/// Wenn sich `path` nicht auflösen lässt oder die übrig gebliebene Datei
/// nicht entfernt werden kann. Fehlt sie, ist das kein Fehler.
#[expect(
    clippy::disallowed_methods,
    reason = "entfernt nur die neue Datei, die `replace` selbst anlegt"
)]
pub(crate) fn remove_stale(path: &Path) -> io::Result<()> {
    let fresh = fresh_path(&resolve(path)?)?;
    match fs::remove_file(&fresh) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        Ok(()) | Err(_) => Ok(()),
    }
}

/// Die Datei, die wirklich geschrieben wird: das Ziel einer Verknüpfung,
/// sonst `path` selbst. Fehlt die Datei, `path`.
fn resolve(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(target) => Ok(target),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path.to_owned()),
        Err(error) => Err(error),
    }
}

/// `datei.toml` → `datei.toml.neu` im selben Ordner (gleiches Dateisystem,
/// damit das Umbenennen atomar ist).
fn fresh_path(path: &Path) -> io::Result<PathBuf> {
    let Some(name) = path.file_name() else {
        // Ohne eigenen Text: Er wäre nur deutsch und nennte den Pfad.
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    };
    let mut name = OsString::from(name);
    name.push(FRESH_SUFFIX);
    Ok(path.with_file_name(name))
}

/// Schreibt `text` in eine neue Datei und wartet, bis er auf der Platte ist.
#[expect(
    clippy::disallowed_methods,
    reason = "kürzt nur die eigene neue Datei von `replace`, nie die Datei des Nutzers"
)]
fn write_synced(path: &Path, text: &str) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// Öffnet die Datei im Standard-Texteditor; legt sie vorher mit `template`
/// an, falls sie fehlt.
///
/// # Errors
/// [`TextFileError`], wenn die Datei nicht anlegbar ist oder der Editor
/// nicht startet.
pub(crate) fn open_in_editor(path: &Path, template: &str) -> Result<(), TextFileError> {
    ensure(path, template)?;
    process::run_checked(
        process::command("/usr/bin/open").arg("-t").arg(path),
        b"",
        Limit::After(OPEN_TIMEOUT),
    )
    .map(drop)
    .map_err(|source| TextFileError::Open {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn ensure_creates_once_and_keeps_an_existing_file() {
        let temp = TempDir::new("ensure");
        let dir = temp.path();
        let path = dir.join("unter/datei.toml");
        assert!(ensure(&path, "Vorlage").unwrap(), "fehlende Datei angelegt");
        replace(&path, "eigener Inhalt").unwrap();
        assert!(
            !ensure(&path, "Vorlage").unwrap(),
            "vorhandene Datei nicht neu angelegt"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "eigener Inhalt");
    }

    #[test]
    fn replace_swaps_the_content_without_leftovers() {
        let temp = TempDir::new("replace");
        let dir = temp.path();
        let path = temp.file("datei.toml", "alt");
        replace(&path, "neu").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "neu");
        let names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["datei.toml"]);
    }
    #[cfg(unix)]
    #[test]
    fn replace_writes_through_a_symlink() {
        let temp = TempDir::new("link");
        let dir = temp.path();
        let target = temp.file("dotfiles-settings.toml", "alt");
        let link = dir.join("settings.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        replace(&link, "neu").unwrap();
        let target_content = fs::read_to_string(&target).unwrap();
        let still_link = fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink();
        assert!(still_link, "die Verknüpfung bleibt");
        assert_eq!(target_content, "neu");
    }

    #[test]
    fn a_failed_replace_leaves_the_old_file_and_no_leftover() {
        let temp = TempDir::new("replace-failed");
        let dir = temp.path();
        // Ein Ordner lässt sich nicht durch eine Datei ersetzen: Das
        // Umbenennen scheitert, nachdem die neue Datei geschrieben ist.
        let path = dir.join("ordner");
        fs::create_dir_all(path.join("darin")).unwrap();
        assert!(replace(&path, "neu").is_err(), "Umbenennen scheitert");
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["ordner"]);
    }

    #[test]
    fn replace_without_a_file_name_is_refused() {
        let error = replace(Path::new("/"), "neu").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn replace_creates_a_missing_file() {
        let temp = TempDir::new("replace-missing");
        let dir = temp.path();
        let path = dir.join("neu.toml");
        replace(&path, "neu").unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "neu");
    }

    #[test]
    fn reads_the_format_line_in_the_head_only() {
        let current = 2;
        let head = format_line(current);
        assert_eq!(
            format_of(&format!("{head}# Kopf\n[a]\nb = 1\n"), current),
            Ok(Format::Current)
        );
        assert_eq!(
            format_of("# Kopf\n\n[a]\nb = 1\n", current),
            Ok(Format::Legacy)
        );
        assert_eq!(format_of("", current), Ok(Format::Legacy));
        assert_eq!(
            format_of(&format!("[a]\n{head}"), current),
            Ok(Format::Legacy),
            "eine Kennzeile nach dem ersten Eintrag zählt nicht"
        );
        assert_eq!(format_of(&format_line(1), current), Ok(Format::Legacy));
        assert_eq!(
            format_of(&format_line(3), current),
            Err(FormatError::Unknown { found: 3, current })
        );
        assert_eq!(
            format_of(&format_line(0), current),
            Err(FormatError::Unknown { found: 0, current })
        );
        assert!(
            matches!(
                format_of("# Dateiformat / file format: zwei\n", current),
                Err(FormatError::Malformed(_))
            ),
            "keine Zahl hinter der Kennzeile"
        );
    }

    #[test]
    fn start_up_removes_a_stale_fresh_file() {
        let temp = TempDir::new("stale");
        let dir = temp.path();
        let path = temp.file("datei.toml", "alt");
        temp.file("datei.toml.neu", "halb");
        remove_stale(&path).unwrap();
        remove_stale(&path).unwrap();
        let names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(names, ["datei.toml"]);
        assert_eq!(content, "alt");
    }
}
