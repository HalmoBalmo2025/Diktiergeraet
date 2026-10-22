//! Woher ein langes Transkript kommt: Video oder Datei.
//!
//! Ein `YouTube`-Video lädt yt-dlp + ffmpeg, eine Audio-/Videodatei auf dem
//! Mac dekodiert ffmpeg. Geladen wird auf einem eigenen Thread; das Ergebnis
//! geht als [`Loaded`] an die Steuerung. Mit einem [`TimeRange`] wird nur
//! dieser Bereich geladen.

use crate::note::{Explain, Note, UserText};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::audio::Pcm16k;
use crate::audio::decode::{self, DecodeError};
use crate::audio::range::TimeRange;
use crate::language::Language;
use crate::text::Chapter;
use crate::textfile;
use crate::youtube::{self, VideoUrl, YoutubeError};

/// Welche Sprache ein YouTube-Video bekommt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LanguageChoice {
    /// Wie `YouTube` sie meldet (falls das Modell sie kennt, sonst Englisch).
    Automatic,
    /// Fest gewählt.
    Fixed(Language),
}

/// Art der Quelle; jede hat ihr eigenes „letztes Transkript“.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceKind {
    /// Ein `YouTube`-Video.
    Youtube,
    /// Eine Audio- oder Videodatei.
    File,
}

impl SourceKind {
    /// Name für Meldungen.
    #[must_use]
    const fn label(self) -> &'static str {
        match self {
            Self::Youtube => "YouTube",
            Self::File => "Datei",
        }
    }

    /// Englischer Name für Meldungen.
    #[must_use]
    pub(crate) const fn english_label(self) -> &'static str {
        match self {
            Self::Youtube => "YouTube",
            Self::File => "File",
        }
    }
}

impl std::fmt::Display for SourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// So viele Transkripte derselben Datei werden nebeneinander abgelegt
/// („Vorlesung.txt“ bis „Vorlesung (99).txt“).
const MAX_SAVED: u32 = 99;

/// Woher ein laufendes Transkript stammt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Origin {
    /// Ein `YouTube`-Video.
    Youtube,
    /// Eine Datei; das Transkript wird zusätzlich daneben gespeichert.
    File(PathBuf),
}

impl Origin {
    /// Art der Quelle.
    #[must_use]
    pub(crate) const fn kind(&self) -> SourceKind {
        match self {
            Self::Youtube => SourceKind::Youtube,
            Self::File(_) => SourceKind::File,
        }
    }
}

/// Was transkribiert werden soll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// Ein YouTube-Video.
    Youtube {
        /// Der Link.
        url: VideoUrl,
        /// Sprache, automatisch oder fest.
        language: LanguageChoice,
        /// Nur dieser Bereich; ohne ihn das ganze Video.
        range: Option<TimeRange>,
        /// Danach in diese Sprache übersetzen.
        translate: Option<Language>,
    },
    /// Eine Audio- oder Videodatei.
    File {
        /// Pfad der Datei.
        path: PathBuf,
        /// Sprache (eine Datei verrät sie nicht).
        language: Language,
        /// Nur dieser Bereich; ohne ihn die ganze Datei.
        range: Option<TimeRange>,
    },
}

impl Source {
    /// Art der Quelle.
    #[must_use]
    pub(crate) const fn kind(&self) -> SourceKind {
        match self {
            Self::Youtube {
                url: _,
                language: _,
                range: _,
                translate: _,
            } => SourceKind::Youtube,
            Self::File {
                path: _,
                language: _,
                range: _,
            } => SourceKind::File,
        }
    }

    /// In welche Sprache das Transkript danach übersetzt wird.
    #[must_use]
    pub(crate) const fn translate(&self) -> Option<Language> {
        match self {
            Self::Youtube {
                translate,
                url: _,
                language: _,
                range: _,
            } => *translate,
            Self::File {
                path: _,
                language: _,
                range: _,
            } => None,
        }
    }

    /// Woher das Transkript stammt.
    #[must_use]
    pub(crate) fn origin(&self) -> Origin {
        match self {
            Self::Youtube {
                url: _,
                language: _,
                range: _,
                translate: _,
            } => Origin::Youtube,
            Self::File {
                path,
                language: _,
                range: _,
            } => Origin::File(path.clone()),
        }
    }

    /// Titel, soweit schon vor dem Laden bekannt (der Dateiname; ein
    /// Videotitel kommt erst mit den Metadaten).
    #[must_use]
    pub(crate) fn known_title(&self) -> Option<UserText> {
        match self {
            Self::Youtube {
                url: _,
                language: _,
                range: _,
                translate: _,
            } => None,
            Self::File {
                path,
                range,
                language: _,
            } => Some(titled(title_of(path), range.as_ref())),
        }
    }
}

/// Eine geladene Tonspur.
#[derive(Debug)]
pub(crate) struct Loaded {
    /// Titel des Videos bzw. Dateiname ohne Endung, bei einem Bereich mit
    /// diesem („Vorlesung (12:00–20:00)“).
    pub title: UserText,
    /// Sprache fürs Modell.
    pub language: Language,
    /// Die Tonspur.
    pub pcm: Pcm16k,
    /// Wo die Tonspur im Original beginnt (bei einem Bereich dessen
    /// Anfang); Zeitmarken zählen ab dem Anfang des Originals.
    pub begin: Duration,
    /// Kapitel (nur bei Videos, die welche haben), mit Zeiten im Original.
    pub chapters: Vec<Chapter>,
}

/// Fehler beim Laden.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LoadError {
    /// Abgebrochen.
    #[error("abgebrochen")]
    Cancelled,
    /// YouTube-Video nicht ladbar.
    #[error(transparent)]
    Youtube(YoutubeError),
    /// Datei nicht lesbar.
    #[error(transparent)]
    Decode(DecodeError),
    /// Die Datei enthält keinen Ton. Ihr Titel steht nur in der Meldung
    /// im Panel ([`Explain::note`]), nicht im Protokoll.
    #[error("„…“ enthält keine Tonspur")]
    NoAudio(UserText),
    /// Im gewählten Bereich ist kein Ton, meist weil er hinter dem Ende
    /// liegt.
    #[error("„…“: Der Bereich liegt hinter dem Ende")]
    OutsideRange(UserText),
}

impl Explain for LoadError {
    fn english(&self) -> String {
        match self {
            Self::Cancelled => "cancelled".to_owned(),
            Self::Youtube(source) => source.english(),
            Self::Decode(source) => source.english(),
            Self::NoAudio(_) => "“…” has no audio track".to_owned(),
            Self::OutsideRange(_) => "“…”: the range lies beyond the end".to_owned(),
        }
    }

    fn note(&self) -> Note {
        match self {
            Self::NoAudio(title) => Note::showing(|shown| {
                (
                    format!("„{}“ enthält keine Tonspur", shown.text(title)),
                    format!("“{}” has no audio track", shown.text(title)),
                )
            }),
            Self::OutsideRange(title) => Note::showing(|shown| {
                (
                    format!("„{}“: Der Bereich liegt hinter dem Ende", shown.text(title)),
                    format!("“{}”: the range lies beyond the end", shown.text(title)),
                )
            }),
            Self::Cancelled => Note::new(self.to_string(), self.english()),
            Self::Youtube(source) => source.note(),
            Self::Decode(source) => source.note(),
        }
    }
}

impl From<YoutubeError> for LoadError {
    fn from(error: YoutubeError) -> Self {
        match error {
            YoutubeError::Cancelled => Self::Cancelled,
            other @ (YoutubeError::MissingTool(_)
            | YoutubeError::Spawn { tool: _, source: _ }
            | YoutubeError::Refused { reason: _, hint: _ }
            | YoutubeError::Failed {
                tool: _,
                message: _,
            }
            | YoutubeError::TimedOut(_)
            | YoutubeError::Process(_)
            | YoutubeError::Crashed
            | YoutubeError::NoTitle
            | YoutubeError::Stalled) => Self::Youtube(other),
        }
    }
}

impl From<DecodeError> for LoadError {
    fn from(error: DecodeError) -> Self {
        match error {
            DecodeError::Cancelled => Self::Cancelled,
            other @ (DecodeError::MissingFfmpeg
            | DecodeError::Process(_)
            | DecodeError::Failed { message: _ }) => Self::Decode(other),
        }
    }
}

/// Lädt Titel, Sprache und Tonspur (läuft auf dem Lade-Thread).
///
/// # Errors
/// [`LoadError`], wenn Video oder Datei nicht ladbar sind oder abgebrochen
/// wurde.
pub(crate) fn load(source: &Source, cancel: &AtomicBool) -> Result<Loaded, LoadError> {
    let loaded = match source {
        Source::Youtube {
            url,
            language,
            range,
            translate: _,
        } => load_youtube(url, *language, range.as_ref(), cancel)?,
        Source::File {
            path,
            language,
            range,
        } => load_file(path, *language, range.as_ref(), cancel)?,
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(LoadError::Cancelled);
    }
    Ok(loaded)
}

/// Video: Metadaten, dann die Tonspur.
fn load_youtube(
    url: &VideoUrl,
    choice: LanguageChoice,
    range: Option<&TimeRange>,
    cancel: &AtomicBool,
) -> Result<Loaded, LoadError> {
    let info = youtube::fetch_info(url, cancel)?;
    let language = match choice {
        LanguageChoice::Automatic => info.model_language(),
        LanguageChoice::Fixed(language) => language,
    };
    let title = titled(info.title, range);
    // Hinter dem Ende scheitert yt-dlp mit einer unverständlichen Meldung.
    let beyond_end = range
        .zip(info.duration)
        .is_some_and(|(range, duration)| range.start() >= duration);
    if beyond_end {
        return Err(LoadError::OutsideRange(title));
    }
    let pcm = youtube::fetch_audio(url, range, cancel)?;
    reject_empty(&pcm, range, title.clone())?;
    Ok(Loaded {
        title,
        language,
        pcm,
        begin: range.map(TimeRange::start).unwrap_or_default(),
        chapters: chapters_within(info.chapters, range),
    })
}

/// Datei: mit ffmpeg dekodieren.
fn load_file(
    path: &Path,
    language: Language,
    range: Option<&TimeRange>,
    cancel: &AtomicBool,
) -> Result<Loaded, LoadError> {
    let title = titled(title_of(path), range);
    let pcm = decode::decode_file(path, range, cancel)?;
    reject_empty(&pcm, range, title.clone())?;
    Ok(Loaded {
        title,
        language,
        pcm,
        begin: range.map(TimeRange::start).unwrap_or_default(),
        chapters: Vec::new(),
    })
}

/// Eine Tonspur ohne ein einziges Sample ist ein Fehler, für Videos wie
/// für Dateien: Mit Bereich liegt er hinter dem Ende, sonst ist kein Ton
/// darin. (Ein leeres Transkript ersetzte sonst die Zwischenablage durch
/// den bloßen Titel.)
fn reject_empty(pcm: &Pcm16k, range: Option<&TimeRange>, title: UserText) -> Result<(), LoadError> {
    match (pcm.is_empty(), range) {
        (true, Some(_)) => Err(LoadError::OutsideRange(title)),
        (true, None) => Err(LoadError::NoAudio(title)),
        (false, _) => Ok(()),
    }
}

/// Titel mit Bereich: „Vorlesung (12:00–20:00)“.
fn titled(title: String, range: Option<&TimeRange>) -> UserText {
    UserText::new(match range {
        Some(range) => format!("{title} ({})", range.label()),
        None => title,
    })
}

/// Die Kapitel, die im Bereich liegen, dazu das, in dem er beginnt (es wird
/// die erste Überschrift).
fn chapters_within(chapters: Vec<Chapter>, range: Option<&TimeRange>) -> Vec<Chapter> {
    let Some(range) = range else {
        return chapters;
    };
    let current = chapters
        .iter()
        .rposition(|chapter| chapter.start <= range.start());
    chapters
        .into_iter()
        .enumerate()
        .filter(|(index, chapter)| Some(*index) == current || range.contains(chapter.start))
        .map(|(_, chapter)| chapter)
        .collect()
}

/// Warum ein Transkript nicht neben seiner Datei gespeichert wurde.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SaveError {
    /// Der Pfad nennt keinen Ordner (`/` selbst).
    #[error("kein Ordner zur Datei")]
    NoFolder,
    /// Schon so viele Transkripte liegen daneben ([`MAX_SAVED`]).
    #[error("schon {MAX_SAVED} Transkripte neben der Datei")]
    TooMany,
    /// Der Ordner ist nicht beschreibbar oder die Platte voll.
    #[error("{0}")]
    Io(#[from] io::Error),
}

impl Explain for SaveError {
    fn english(&self) -> String {
        match self {
            Self::NoFolder => "no folder for the file".to_owned(),
            Self::TooMany => format!("already {MAX_SAVED} transcripts beside the file"),
            Self::Io(source) => source.to_string(),
        }
    }
}

/// Speichert `text` als Textdatei neben `source`: „Vorlesung.m4a“ →
/// „Vorlesung.txt“, gibt es die schon, „Vorlesung (2).txt“ usw. Nichts wird
/// überschrieben; scheitert das Schreiben, bleibt keine halbe Datei.
///
/// # Errors
/// [`SaveError`], wenn der Ordner fehlt oder nicht beschreibbar ist oder
/// schon zu viele Transkripte daneben liegen.
pub(crate) fn save_beside(source: &Path, text: &str) -> Result<PathBuf, SaveError> {
    let dir = source.parent().ok_or(SaveError::NoFolder)?;
    let stem = title_of(source);
    let content = format!("{text}\n");
    for number in 1..=MAX_SAVED {
        let name = match number {
            1 => format!("{stem}.txt"),
            number => format!("{stem} ({number}).txt"),
        };
        let path = dir.join(name);
        match textfile::write_new(&path, &content) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(SaveError::Io(error)),
        }
    }
    Err(SaveError::TooMany)
}

/// Dateiname ohne Endung als Titel.
fn title_of(path: &Path) -> String {
    path.file_stem()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// Liest eine Datei aus dem Text der Zwischenablage, etwa nach „Als Pfadname
/// kopieren“ (⌥⌘C) im Finder: ein vorhandener Pfad, auch mit `~` oder in
/// Anführungszeichen.
#[must_use]
pub(crate) fn path_from_text(text: &str, home: Option<&Path>) -> Option<PathBuf> {
    let trimmed = text.trim().trim_matches(['"', '\'']);
    let path = match (trimmed.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        (Some(_), None) | (None, _) => PathBuf::from(trimmed),
    };
    (path.is_absolute() && path.is_file()).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_beside_the_file_without_overwriting() {
        let temp = crate::testing::TempDir::new("save");
        let dir = temp.path();
        let source = dir.join("Vorlesung BGB.m4a");
        let first = save_beside(&source, "Erstes").unwrap();
        let second = save_beside(&source, "Zweites").unwrap();
        assert_eq!(first, dir.join("Vorlesung BGB.txt"));
        assert_eq!(second, dir.join("Vorlesung BGB (2).txt"));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "Erstes\n");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "Zweites\n");
    }

    #[test]
    fn too_many_transcripts_are_reported_in_both_languages() {
        let dir = crate::testing::TempDir::new("save-many");
        let source = dir.path().join("Eins.m4a");
        for _ in 0..MAX_SAVED {
            save_beside(&source, "alt").unwrap();
        }
        let error = save_beside(&source, "neu").unwrap_err();
        assert!(matches!(error, SaveError::TooMany), "{error:?}");
        assert_eq!(error.english(), "already 99 transcripts beside the file");
        assert!(matches!(
            save_beside(Path::new("/"), "x"),
            Err(SaveError::NoFolder)
        ));
    }

    #[test]
    fn range_keeps_the_current_and_the_inner_chapters() {
        let chapter = |minute: u64, title: &str| Chapter {
            start: Duration::from_secs(minute * 60),
            title: title.to_owned(),
        };
        let chapters = vec![
            chapter(0, "Einführung"),
            chapter(10, "Vertragsschluss"),
            chapter(15, "Angebot"),
            chapter(25, "Annahme"),
        ];
        let range = TimeRange::parse("12:00", "20:00").unwrap();
        assert_eq!(
            chapters_within(chapters.clone(), range.as_ref()),
            [chapter(10, "Vertragsschluss"), chapter(15, "Angebot")]
        );
        assert_eq!(chapters_within(chapters.clone(), None), chapters);
        let from_start = TimeRange::parse("", "5").unwrap();
        assert_eq!(
            chapters_within(chapters, from_start.as_ref()),
            [chapter(0, "Einführung")]
        );
    }

    #[test]
    fn empty_audio_is_an_error_with_and_without_a_range() {
        let empty = Pcm16k::from_samples(Vec::new());
        let title = UserText::new("Video");
        assert!(matches!(
            reject_empty(&empty, None, title.clone()),
            Err(LoadError::NoAudio(_))
        ));
        let range = TimeRange::parse("1:00", "").unwrap();
        assert!(matches!(
            reject_empty(&empty, range.as_ref(), title.clone()),
            Err(LoadError::OutsideRange(_))
        ));
        reject_empty(&Pcm16k::from_samples(vec![0.0]), None, title).unwrap();
    }

    #[test]
    fn title_names_the_range() {
        let range = TimeRange::parse("12:00", "20:00").unwrap();
        assert_eq!(
            titled("Vorlesung".to_owned(), range.as_ref()).shown(),
            "Vorlesung (12:00–20:00)"
        );
        assert_eq!(titled("Vorlesung".to_owned(), None).shown(), "Vorlesung");
    }

    #[test]
    fn title_is_file_stem() {
        assert_eq!(
            title_of(Path::new("/x/Vorlesung BGB AT 3.m4a")),
            "Vorlesung BGB AT 3"
        );
    }

    #[test]
    fn reads_paths_from_text() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let file = format!("{manifest}/Cargo.toml");
        assert_eq!(
            path_from_text(&format!(" \"{file}\"\n"), None),
            Some(PathBuf::from(&file))
        );
        assert_eq!(
            path_from_text("~/Cargo.toml", Some(Path::new(manifest))),
            Some(PathBuf::from(&file))
        );
        assert_eq!(path_from_text("https://youtu.be/abc", None), None);
        assert_eq!(path_from_text(manifest, None), None);
    }
}
