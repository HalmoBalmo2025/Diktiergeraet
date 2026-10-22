//! Die Einstellungen (`settings.toml`): Taste, zweite Sprache, Teile beim
//! Diktieren, Klänge, Modell, Protokoll, Überarbeiten und die Sprache der
//! Oberfläche.
//!
//! Bearbeitet werden sie im Panel (Reiter „Einstellungen“), das sie als
//! [`SettingsEntries`] schickt; Diktator schreibt sie samt Erklärungen in die
//! Datei; fast alles gilt sofort. Von Hand Geändertes gilt, sobald das
//! Panel die Datei liest. Nur Taste und Modelldatei werden beim Start
//! gelesen – weichen sie ab, zeigt das Panel einen Hinweis zum Neustart. Ist die Datei fehlerhaft, startet Diktator mit den
//! Standardwerten und meldet das.
//!
//! Das Format trägt eine Nummer ([`FORMAT_VERSION`], als Kommentar in der
//! ersten Zeile, siehe [`textfile::format_of`]). Dateien früherer Versionen
//! bleiben lesbar: Werte, die heute nicht mehr erlaubt sind, werden ersetzt
//! und gemeldet ([`Adjustment`]), statt die ganze Datei abzulehnen. Ein
//! Beispiel jeder früheren Fassung liegt unter `fixtures/settings/`.

use crate::note::Explain;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::audio::chunk::{MAX_PART_SECONDS, PartSecondsError, PartTiming};
use crate::feedback::{Logging, Sounds, Volume, VolumeError};
use crate::hotkey::TriggerKey;
use crate::language::{Language, NotSecondaryError, SecondaryLanguage};
use crate::note::{Note, UiLanguage};
use crate::paths::{self, PathsError};
use crate::polish::llama;
use crate::text::{ChapterHeadings, Timestamps};
use crate::textfile::{self, Format, FormatError, TextFileError, TomlError};

/// Nummer des Formats, das diese Version schreibt.
///
/// 1: ohne Kennzeile, bis einschließlich 973fa8b; `teile_ab_sekunden`
/// durfte bis 60 gehen (894e913) und `zweite` Deutsch sein. Beim Lesen
/// werden solche Werte ersetzt ([`Adjustment`]).
/// 2: mit Kennzeile; dieselben Schlüssel, Werte außerhalb der Grenzen sind
/// Fehler.
/// 3: `kapitel` in `[transkripte]`; fehlt der Schlüssel (bis 14c618b), gilt
/// an, wie damals.
pub(crate) const FORMAT_VERSION: u16 = 3;

/// Kopf der Datei.
const HEADER: &str = "\
# Diktator – Einstellungen
#
# Bearbeitbar im Panel (Reiter „Einstellungen“) oder hier. Im Panel gilt fast
# alles sofort; Taste und Modelldatei erst nach einem Neustart (das Panel
# zeigt dann einen Hinweis). Änderungen hier gelten, sobald das Panel sie
# zeigt, Taste und Modelldatei auch hier erst nach einem Neustart.
# Ist die Datei fehlerhaft, startet Diktator mit den Standardwerten.
# Speichert das Panel, schreibt es die Datei neu (eigene Kommentare gehen
# dabei verloren).
";

/// Womit fn + ⌥ überarbeitet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum PolishBackend {
    /// Apple Intelligence (Foundation Models, auf dem Gerät).
    #[default]
    #[serde(rename = "apple")]
    Apple,
    /// Ein lokales Modell aus `models/llm/` über llama.cpp.
    #[serde(rename = "lokal")]
    Local,
}

impl PolishBackend {
    /// Wert in `settings.toml`.
    #[must_use]
    pub(crate) const fn file_value(self) -> &'static str {
        match self {
            Self::Apple => "apple",
            Self::Local => "lokal",
        }
    }
}

/// Standard für `entladen_nach_minuten` (Spracherkennung).
const DEFAULT_UNLOAD_MINUTES: u16 = 30;

/// Standard für `entladen_nach_minuten` beim Überarbeiten: Das Sprachmodell
/// belegt mehrere GB und wird seltener gebraucht.
const DEFAULT_POLISH_UNLOAD_MINUTES: u16 = 10;

/// Fehler beim Lesen oder Schreiben der Einstellungen.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SettingsError {
    /// Kein Ablageort für die Datei.
    #[error("Einstellungen: {0}")]
    Paths(#[from] PathsError),
    /// Die Datei ließ sich nicht anlegen oder öffnen.
    #[error("Einstellungen: {0}")]
    File(#[from] TextFileError),
    /// Die Datei ließ sich nicht lesen.
    #[error("Einstellungen {} nicht lesbar: {source}", paths::tilde(.path))]
    Read {
        /// Pfad der Datei.
        path: PathBuf,
        /// Ursache.
        #[source]
        source: io::Error,
    },
    /// Die Datei ließ sich nicht schreiben.
    #[error("Einstellungen {} nicht speicherbar: {source}", paths::tilde(.path))]
    Write {
        /// Pfad der Datei.
        path: PathBuf,
        /// Ursache.
        #[source]
        source: io::Error,
    },
    /// Die Datei ist kein gültiges TOML oder enthält Unbekanntes; ohne den
    /// Ausschnitt der Datei (er käme ins Protokoll).
    #[error("Einstellungen fehlerhaft{}", .0.german())]
    Parse(TomlError),
    /// Lautstärke über 100 %.
    #[error("Einstellungen fehlerhaft: {0}")]
    Volume(#[from] VolumeError),
    /// Sekunden bis zum ersten Teil außerhalb von 0 bis [`MAX_PART_SECONDS`].
    #[error("Einstellungen fehlerhaft: teile_ab_sekunden: {0}")]
    PartSeconds(#[from] PartSecondsError),
    /// Deutsch als zweite Sprache.
    #[error("Einstellungen fehlerhaft: zweite: {0}")]
    Secondary(#[from] NotSecondaryError),
    /// Die Kennzeile nennt kein bekanntes Format.
    #[error("Einstellungen nicht lesbar: {0}")]
    Format(#[from] FormatError),
}

impl Explain for SettingsError {
    fn english(&self) -> String {
        match self {
            Self::Paths(source) => format!("settings: {}", source.english()),
            Self::File(source) => format!("settings: {}", source.english()),
            Self::Read { path, source } => {
                format!("settings {} not readable: {source}", paths::tilde(path))
            }
            Self::Write { path, source } => {
                format!("settings {} cannot be saved: {source}", paths::tilde(path))
            }
            Self::Parse(source) => format!("settings faulty{}", source.english()),
            Self::Volume(source) => format!("settings faulty: {}", source.english()),
            Self::PartSeconds(source) => {
                format!("settings faulty: teile_ab_sekunden: {}", source.english())
            }
            Self::Secondary(source) => format!("settings faulty: zweite: {}", source.english()),
            Self::Format(source) => format!("settings not readable: {}", source.english()),
        }
    }
}

/// Ein Wert aus einer Datei einer früheren Version, den diese Version nicht
/// mehr erlaubt und deshalb ersetzt hat. In der Datei steht er weiter, bis
/// das Panel sie speichert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Adjustment {
    /// `teile_ab_sekunden` über [`MAX_PART_SECONDS`] (bis 894e913 waren
    /// bis 60 erlaubt); es gilt [`MAX_PART_SECONDS`].
    #[error(
        "Einstellungen: teile_ab_sekunden = {written} ist nicht mehr erlaubt, es gilt {MAX_PART_SECONDS}."
    )]
    PartSeconds {
        /// Der Wert in der Datei.
        written: u16,
    },
    /// `zweite = "de"`, obwohl fn allein schon Deutsch ist; es gilt
    /// Englisch.
    #[error("Einstellungen: zweite = \"de\" geht nicht (fn allein ist Deutsch), es gilt Englisch.")]
    SecondaryGerman,
}

impl Explain for Adjustment {
    fn english(&self) -> String {
        match self {
            Self::PartSeconds { written } => format!(
                "settings: teile_ab_sekunden = {written} is no longer allowed, {MAX_PART_SECONDS} applies."
            ),
            Self::SecondaryGerman => {
                "settings: zweite = \"de\" is not possible (fn alone is German), English applies."
                    .to_owned()
            }
        }
    }
}

/// Gelesene Einstellungen und was beim Lesen ersetzt wurde.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Read<T> {
    /// Was gilt.
    pub value: T,
    /// Werte einer früheren Version, die ersetzt wurden; leer bei einer
    /// Datei dieser Version.
    pub adjustments: Vec<Adjustment>,
}

impl<T> Read<T> {
    /// Eine Meldung über alle ersetzten Werte; `None`, wenn keiner ersetzt
    /// wurde.
    #[must_use]
    pub(crate) fn note(&self) -> Option<Note> {
        if self.adjustments.is_empty() {
            return None;
        }
        let join = |text: fn(&Adjustment) -> String| {
            self.adjustments
                .iter()
                .map(text)
                .collect::<Vec<_>>()
                .join(" ")
        };
        Some(Note::new(
            join(|adjustment| {
                format!("{adjustment} Speichern im Panel trägt das in die Datei ein.")
            }),
            join(|adjustment| {
                format!(
                    "{} Saving in the panel writes this into the file.",
                    adjustment.english()
                )
            }),
        ))
    }
}

/// Wann das Modell entladen wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unload {
    /// Nie; das Modell bleibt geladen.
    Never,
    /// Nach so langer Zeit ohne Auftrag.
    AfterIdle(Duration),
}

/// Die geltenden Einstellungen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Settings {
    /// Taste zum Diktieren.
    pub key: TriggerKey,
    /// Klänge (gilt sofort, ohne Neustart).
    pub sounds: Sounds,
    /// Dateiname des Modells im Ordner `models/`.
    pub model_file: String,
    /// Wann das Modell entladen wird (gilt sofort, ohne Neustart).
    pub unload: Unload,
    /// Sprache für fn + ⇧ (gilt sofort, ohne Neustart).
    pub secondary: Language,
    /// Wann ein laufendes Diktat in Teilen transkribiert wird (gilt sofort).
    pub parts: PartTiming,
    /// Protokoll schreiben (gilt sofort, ohne Neustart).
    pub logging: Logging,
    /// Womit überarbeitet wird (gilt sofort, ohne Neustart).
    pub polish: PolishSettings,
    /// Zeitmarken in Video- und Datei-Transkripten (gilt sofort).
    pub timestamps: Timestamps,
    /// Kapitelüberschriften in Video-Transkripten (gilt sofort).
    pub chapters: ChapterHeadings,
    /// Sprache von Panel und Mitteilungen (gilt sofort).
    pub language: UiLanguage,
}

/// Womit fn + ⌥ überarbeitet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolishSettings {
    /// Apple oder lokal.
    pub backend: PolishBackend,
    /// Dateiname des lokalen Modells in `models/llm/`; `None` = das erste dort.
    pub model_file: Option<String>,
    /// Wann das lokale Modell entladen wird.
    pub unload: Unload,
}

impl PolishSettings {
    /// Die Modelldatei, die gilt: die gewählte, sonst die erste in
    /// `models/llm/`. Liegt dort keine passende, der Pfad, an dem die
    /// gewählte liegen sollte, und ist keine gewählt, der Ordner selbst –
    /// das Überarbeiten meldet dann, dass die Modelldatei fehlt
    /// (`LlamaError::MissingModel` mit diesem Pfad).
    ///
    /// # Errors
    /// Wie [`paths::app_dir`].
    pub(crate) fn model_path(&self) -> Result<PathBuf, PathsError> {
        let dir = paths::llm_dir()?;
        let available = llama::model_files();
        let chosen = self.model_file.as_deref();
        let found = available
            .iter()
            .find(|model| Some(model.file.as_str()) == chosen)
            .or_else(|| available.first())
            .map(|model| model.file.as_str());
        Ok(match found.or(chosen) {
            Some(file) => dir.join(file),
            None => dir,
        })
    }
}

impl Default for Settings {
    fn default() -> Self {
        SettingsEntries::default().settings()
    }
}

impl Settings {
    /// Liest die Einstellungen; legt die Datei vorher an, falls sie fehlt.
    ///
    /// # Errors
    /// [`SettingsError`], wenn die Datei nicht anlegbar, nicht lesbar oder
    /// fehlerhaft ist.
    pub(crate) fn load(path: &Path) -> Result<Read<Self>, SettingsError> {
        let read = SettingsEntries::read(path)?;
        Ok(Read {
            value: read.value.settings(),
            adjustments: read.adjustments,
        })
    }

    /// Braucht es einen Neustart, um von `running` zu diesen Einstellungen
    /// zu kommen? Nur Taste und Modelldatei; alles andere gilt sofort.
    #[must_use]
    pub(crate) fn needs_restart_from(&self, running: &Self) -> Restart {
        if self.key != running.key || self.model_file != running.model_file {
            Restart::Needed
        } else {
            Restart::NotNeeded
        }
    }

    /// Pfad der gewählten Modelldatei.
    ///
    /// # Errors
    /// Wie [`paths::app_dir`].
    pub(crate) fn model_path(&self) -> Result<PathBuf, PathsError> {
        Ok(paths::models_dir()?.join(&self.model_file))
    }
}

/// Ob eine Einstellung erst nach einem Neustart gilt. Zum Panel geht es als
/// `true` (nötig) oder `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(into = "bool")]
pub(crate) enum Restart {
    /// Diktator läuft mit anderen Werten, als die Datei sagt.
    Needed,
    /// Alles in der Datei gilt schon.
    NotNeeded,
}

impl From<Restart> for bool {
    fn from(restart: Restart) -> Self {
        match restart {
            Restart::Needed => true,
            Restart::NotNeeded => false,
        }
    }
}

/// Ob Klänge spielen, getrennt von der Lautstärke: Die eingestellte
/// Lautstärke bleibt erhalten, während die Klänge aus sind. In Datei und
/// Panel `true` (an) oder `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub(crate) enum SoundSwitch {
    /// Klänge an.
    On,
    /// Keine Klänge.
    Off,
}

impl From<bool> for SoundSwitch {
    fn from(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }
}

impl From<SoundSwitch> for bool {
    fn from(switch: SoundSwitch) -> Self {
        match switch {
            SoundSwitch::On => true,
            SoundSwitch::Off => false,
        }
    }
}

/// Die Einstellungen als bearbeitbare Werte, so wie das Panel sie zeigt und
/// zurückschickt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettingsEntries {
    /// Taste zum Diktieren.
    pub key: TriggerKey,
    /// Klänge an?
    pub sounds: SoundSwitch,
    /// Lautstärke (als Zahl: Prozent, 0–100).
    pub volume: Volume,
    /// Dateiname des Modells im Ordner `models/`.
    pub model_file: String,
    /// Minuten bis zum Entladen; 0 = nie.
    pub unload_minutes: u16,
    /// Sprache für fn + ⇧ (nie Deutsch).
    pub secondary_language: SecondaryLanguage,
    /// Wann ein Diktat in Teilen erscheint (als Zahl: Sekunden, 0 = erst
    /// am Ende).
    pub parts: PartTiming,
    /// Protokoll schreiben?
    pub log: Logging,
    /// Womit überarbeitet wird.
    pub polish_backend: PolishBackend,
    /// Dateiname des lokalen Modells in `models/llm/`; leer = das erste dort.
    pub polish_model: String,
    /// Minuten bis zum Entladen des lokalen Modells; 0 = nie.
    pub polish_unload_minutes: u16,
    /// Zeitmarken in Video- und Datei-Transkripten?
    pub timestamps: Timestamps,
    /// Kapitel des Videos als Überschriften?
    pub chapters: ChapterHeadings,
    /// Sprache des Panels.
    pub ui_language: UiLanguage,
}

impl Default for SettingsEntries {
    fn default() -> Self {
        Self {
            key: TriggerKey::default(),
            sounds: SoundSwitch::On,
            volume: Volume::QUIET,
            model_file: paths::MODEL_FILE_NAME.to_owned(),
            unload_minutes: DEFAULT_UNLOAD_MINUTES,
            secondary_language: SecondaryLanguage::ENGLISH,
            parts: PartTiming::default(),
            log: Logging::On,
            polish_backend: PolishBackend::Apple,
            polish_model: String::new(),
            polish_unload_minutes: DEFAULT_POLISH_UNLOAD_MINUTES,
            timestamps: Timestamps::Off,
            chapters: ChapterHeadings::On,
            ui_language: UiLanguage::German,
        }
    }
}

impl SettingsEntries {
    /// Liest die Datei; legt sie vorher an, falls sie fehlt.
    ///
    /// # Errors
    /// [`SettingsError`], wenn die Datei nicht anlegbar, nicht lesbar oder
    /// fehlerhaft ist.
    pub(crate) fn read(path: &Path) -> Result<Read<Self>, SettingsError> {
        textfile::ensure(path, &Self::default().render())?;
        let text = fs::read_to_string(path).map_err(|source| SettingsError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&text)
    }

    /// Liest Einstellungen aus TOML-Text, auch aus Dateien früherer
    /// Versionen (siehe [`FORMAT_VERSION`]).
    ///
    /// # Errors
    /// [`SettingsError::Format`], wenn die Kennzeile kein bekanntes Format
    /// nennt; [`SettingsError::Parse`], [`SettingsError::Volume`],
    /// [`SettingsError::PartSeconds`] oder [`SettingsError::Secondary`],
    /// wenn der Text keine gültigen Einstellungen enthält.
    pub(crate) fn parse(text: &str) -> Result<Read<Self>, SettingsError> {
        let format = textfile::format_of(text, FORMAT_VERSION)?;
        let content: FileContent = toml::from_str(text)
            .map_err(|error| SettingsError::Parse(TomlError::new(text, &error)))?;
        let mut adjustments = Vec::new();
        let parts = part_timing(content.dictation.part_seconds, format, &mut adjustments)?;
        let secondary = secondary_language(content.languages.secondary, format, &mut adjustments)?;
        let volume = content.sounds.volume;
        let entries = Self {
            key: content.keys.dictation,
            sounds: content.sounds.enabled,
            volume: u8::try_from(volume)
                .ok()
                .ok_or(VolumeError(volume))
                .and_then(Volume::try_from)?,
            model_file: content.model.file,
            unload_minutes: content.model.unload_after_minutes,
            secondary_language: secondary,
            parts,
            log: content.log.enabled,
            polish_backend: content.polish.backend,
            polish_model: content.polish.model,
            polish_unload_minutes: content.polish.unload_after_minutes,
            timestamps: content.transcripts.timestamps,
            chapters: content.transcripts.chapters,
            ui_language: content.interface.language,
        };
        Ok(Read {
            value: entries,
            adjustments,
        })
    }

    /// Schreibt die Werte samt Erklärungen in die Datei.
    ///
    /// # Errors
    /// [`SettingsError::Write`], wenn die Datei nicht geschrieben werden kann.
    pub(crate) fn write(&self, path: &Path) -> Result<(), SettingsError> {
        textfile::replace(path, &self.render()).map_err(|source| SettingsError::Write {
            path: path.to_owned(),
            source,
        })
    }

    /// Der Dateiinhalt mit Erklärungen.
    #[must_use]
    pub(crate) fn render(&self) -> String {
        format!("{}{}", self.render_dictation(), self.render_extras())
    }

    /// Kopf und die Abschnitte fürs Diktieren: Tasten, Sprachen, Teile,
    /// Klänge, Modell.
    fn render_dictation(&self) -> String {
        let model = toml::Value::String(self.model_file.clone());
        format!(
            r#"{format_line}{HEADER}
[tasten]
# Taste zum Diktieren (antippen; mit ⇧ in der zweiten Sprache):
{keys}diktat = "{key}"

[sprachen]
# Sprache für fn + ⇧ (fn allein ist immer Deutsch). Das Modell kennt:
{languages}zweite = "{secondary}"

[diktat]
# Ab so vielen Sekunden erscheint ein langes Diktat schon beim Sprechen in
# Teilen: an der nächsten längeren Pause (meist ein Satzende), ab dem
# Doppelten an jeder Sprechpause, ab dem Fünffachen auch ohne Pause,
# spätestens nach 28 s (am Stück nimmt das Modell höchstens 30 s).
# Später geschnitten liegt der Schnitt seltener mitten im Satz, wo das
# Modell oft falsche Satzzeichen setzt. 1 bis {max_part}; 0 = erst am Ende.
teile_ab_sekunden = {part_seconds}

[toene]
# Klänge bei Beginn und Ende der Aufnahme und bei Fehlern
an = {sounds}
# Lautstärke in Prozent (0 bis 100)
lautstaerke = {volume}

[modell]
# Datei im Ordner models/ neben dieser Datei, z. B. eine kleinere Stufe
# wie "cohere-transcribe-03-2026-Q5_K_M.gguf" (weniger Speicher).
datei = {model}
# Nach so vielen Minuten ohne Diktat wird das Modell entladen, damit der
# Speicher (rund 2,5 GB) frei wird. Das nächste Diktat lädt es neu und
# dauert dann gut eine Sekunde länger. 0 = nie entladen.
entladen_nach_minuten = {unload}

"#,
            format_line = textfile::format_line(FORMAT_VERSION),
            keys = key_list(),
            languages = language_list(),
            key = self.key.file_value(),
            secondary = self.secondary_language.language().code(),
            part_seconds = u8::from(self.parts),
            max_part = MAX_PART_SECONDS,
            sounds = bool::from(self.sounds),
            volume = self.volume.percent(),
            unload = self.unload_minutes,
        )
    }

    /// Die übrigen Abschnitte: Protokoll, Überarbeiten, Transkripte,
    /// Oberfläche.
    fn render_extras(&self) -> String {
        format!(
            r#"[protokoll]
# Was Diktator tut, in ~/Library/Logs/Diktator.log (zur Fehlersuche;
# ohne Text aus Diktaten oder um die Schreibmarke)
an = {log}

[ueberarbeiten]
# Womit fn + ⌥ überarbeitet: "apple" (Apple Intelligence) oder "lokal"
# (ein Sprachmodell auf diesem Mac über llama.cpp: brew install llama.cpp)
mit = "{backend}"
# GGUF-Datei im Ordner models/llm/; leer = die erste dort
modell = {polish_model}
# Nach so vielen Minuten ohne Überarbeiten wird das lokale Modell entladen
# (gibt mehrere GB frei); das nächste Mal lädt es einige Sekunden.
# 0 = nie entladen.
entladen_nach_minuten = {polish_unload}

[transkripte]
# Zeitmarken wie [12:30] vor jedem Absatz von YouTube- und Datei-Transkripten
zeitmarken = {timestamps}
# Kapitel eines YouTube-Videos als Überschriften (## Titel) im Transkript
kapitel = {chapters}

[oberflaeche]
# Sprache des Panels: "de" Deutsch oder "en" Englisch
sprache = "{ui_language}"
"#,
            log = bool::from(self.log),
            backend = self.polish_backend.file_value(),
            polish_model = toml::Value::String(self.polish_model.clone()),
            polish_unload = self.polish_unload_minutes,
            timestamps = bool::from(self.timestamps),
            chapters = bool::from(self.chapters),
            ui_language = self.ui_language.file_value(),
        )
    }

    /// Die daraus folgenden Einstellungen.
    #[must_use]
    pub(crate) fn settings(&self) -> Settings {
        let sounds = match self.sounds {
            SoundSwitch::On => Sounds::On(self.volume),
            SoundSwitch::Off => Sounds::Off,
        };
        Settings {
            key: self.key,
            sounds,
            model_file: self.model_file.clone(),
            unload: unload_after(self.unload_minutes),
            secondary: self.secondary_language.language(),
            parts: self.parts,
            logging: self.log,
            polish: PolishSettings {
                backend: self.polish_backend,
                model_file: Some(self.polish_model.trim())
                    .filter(|file| !file.is_empty())
                    .map(str::to_owned),
                unload: unload_after(self.polish_unload_minutes),
            },
            timestamps: self.timestamps,
            chapters: self.chapters,
            language: self.ui_language,
        }
    }
}

/// Die Modelldateien (`*.gguf`) im Ordner `models/`, alphabetisch.
///
/// # Errors
/// Wie [`paths::app_dir`]; ein fehlender Datenordner ist keine leere Liste.
pub(crate) fn available_models() -> Result<Vec<String>, PathsError> {
    Ok(paths::gguf_files(&paths::models_dir()?))
}

/// Die Tasten für `diktat` als Kommentar („"fn", … oder "…"“), aus
/// [`TriggerKey::ALL`], damit die Liste nie veraltet.
fn key_list() -> String {
    let values: Vec<String> = TriggerKey::ALL
        .iter()
        .map(|key| format!("\"{}\"", key.file_value()))
        .collect();
    let text = match values.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} oder {last}", rest.join(", ")),
        None => String::new(),
    };
    comment_lines(&[text])
}

/// Die zweiten Sprachen als Kommentar („"en" Englisch, …“), aus
/// [`SecondaryLanguage::all`].
fn language_list() -> String {
    let entries: Vec<String> = SecondaryLanguage::all()
        .map(|secondary| {
            let language = secondary.language();
            format!("\"{}\" {}", language.code(), language.name())
        })
        .collect();
    let mut items: Vec<String> = entries.iter().map(|entry| format!("{entry},")).collect();
    if let Some(last) = items.last_mut() {
        last.pop();
    }
    comment_lines(&items)
}

/// Kommentarzeilen („# …“) mit höchstens [`COMMENT_WIDTH`] Zeichen, die
/// Teile durch Leerzeichen getrennt; ein Teil wird nie umbrochen.
fn comment_lines(parts: &[String]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for part in parts {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + part.chars().count() <= COMMENT_WIDTH => {
                line.push(' ');
                line.push_str(part);
            }
            Some(_) | None => lines.push(format!("# {part}")),
        }
    }
    let mut text = String::new();
    for line in &lines {
        text.push_str(line);
        text.push('\n');
    }
    text
}

/// Breite der Kommentarzeilen in der Datei.
const COMMENT_WIDTH: usize = 76;

/// `teile_ab_sekunden` → [`PartTiming`]. In einer Datei einer früheren
/// Version wird ein Wert über [`MAX_PART_SECONDS`] bis 60 (so viel erlaubte
/// 894e913) zu [`MAX_PART_SECONDS`] und in `adjustments` vermerkt.
fn part_timing(
    seconds: u16,
    format: Format,
    adjustments: &mut Vec<Adjustment>,
) -> Result<PartTiming, SettingsError> {
    let strict = u8::try_from(seconds)
        .ok()
        .ok_or(PartSecondsError(seconds))
        .and_then(PartTiming::try_from);
    match (strict, format) {
        (Ok(parts), Format::Legacy | Format::Current) => Ok(parts),
        (Err(error), Format::Current) => Err(error.into()),
        (Err(error), Format::Legacy) => {
            if !(u16::from(MAX_PART_SECONDS)..=LEGACY_MAX_PART_SECONDS).contains(&seconds) {
                return Err(error.into());
            }
            adjustments.push(Adjustment::PartSeconds { written: seconds });
            Ok(PartTiming::try_from(MAX_PART_SECONDS)?)
        }
    }
}

/// Höchstwert für `teile_ab_sekunden` in 894e913 (Format 1).
const LEGACY_MAX_PART_SECONDS: u16 = 60;

/// `zweite` → [`SecondaryLanguage`]. In einer Datei einer früheren Version
/// wird Deutsch zu Englisch und in `adjustments` vermerkt.
fn secondary_language(
    language: Language,
    format: Format,
    adjustments: &mut Vec<Adjustment>,
) -> Result<SecondaryLanguage, SettingsError> {
    match (SecondaryLanguage::try_from(language), format) {
        (Ok(secondary), Format::Legacy | Format::Current) => Ok(secondary),
        (Err(error), Format::Current) => Err(error.into()),
        (Err(_), Format::Legacy) => {
            adjustments.push(Adjustment::SecondaryGerman);
            Ok(SecondaryLanguage::ENGLISH)
        }
    }
}

/// Minuten aus der Datei → [`Unload`]; 0 heißt nie.
fn unload_after(minutes: u16) -> Unload {
    match minutes {
        0 => Unload::Never,
        minutes => Unload::AfterIdle(Duration::from_mins(u64::from(minutes))),
    }
}

/// Die Datei, wie sie geschrieben steht. Fehlende Abschnitte und Werte
/// nehmen die Standardwerte aus [`SettingsEntries::default`].
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct FileContent {
    #[serde(rename = "tasten")]
    keys: KeysContent,
    #[serde(rename = "sprachen")]
    languages: LanguagesContent,
    #[serde(rename = "toene")]
    sounds: SoundsContent,
    #[serde(rename = "modell")]
    model: ModelContent,
    #[serde(rename = "protokoll")]
    log: LogContent,
    #[serde(rename = "ueberarbeiten")]
    polish: PolishContent,
    #[serde(rename = "transkripte")]
    transcripts: TranscriptsContent,
    #[serde(rename = "diktat")]
    dictation: DictationContent,
    #[serde(rename = "oberflaeche")]
    interface: InterfaceContent,
}

/// Abschnitt `[diktat]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct DictationContent {
    #[serde(rename = "teile_ab_sekunden")]
    part_seconds: u16,
}

impl Default for DictationContent {
    fn default() -> Self {
        Self {
            part_seconds: u16::from(u8::from(SettingsEntries::default().parts)),
        }
    }
}

/// Abschnitt `[oberflaeche]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct InterfaceContent {
    #[serde(rename = "sprache")]
    language: UiLanguage,
}

impl Default for InterfaceContent {
    fn default() -> Self {
        Self {
            language: SettingsEntries::default().ui_language,
        }
    }
}

/// Abschnitt `[transkripte]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct TranscriptsContent {
    #[serde(rename = "zeitmarken")]
    timestamps: Timestamps,
    #[serde(rename = "kapitel")]
    chapters: ChapterHeadings,
}

impl Default for TranscriptsContent {
    fn default() -> Self {
        Self {
            timestamps: SettingsEntries::default().timestamps,
            chapters: SettingsEntries::default().chapters,
        }
    }
}

/// Abschnitt `[ueberarbeiten]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct PolishContent {
    #[serde(rename = "mit")]
    backend: PolishBackend,
    #[serde(rename = "modell")]
    model: String,
    #[serde(rename = "entladen_nach_minuten")]
    unload_after_minutes: u16,
}

impl Default for PolishContent {
    fn default() -> Self {
        let defaults = SettingsEntries::default();
        Self {
            backend: defaults.polish_backend,
            model: defaults.polish_model,
            unload_after_minutes: defaults.polish_unload_minutes,
        }
    }
}

/// Abschnitt `[protokoll]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct LogContent {
    #[serde(rename = "an")]
    enabled: Logging,
}

impl Default for LogContent {
    fn default() -> Self {
        Self {
            enabled: SettingsEntries::default().log,
        }
    }
}

/// Abschnitt `[tasten]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct KeysContent {
    #[serde(rename = "diktat")]
    dictation: TriggerKey,
}

impl Default for KeysContent {
    fn default() -> Self {
        Self {
            dictation: SettingsEntries::default().key,
        }
    }
}

/// Abschnitt `[sprachen]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct LanguagesContent {
    #[serde(rename = "zweite")]
    secondary: Language,
}

impl Default for LanguagesContent {
    fn default() -> Self {
        Self {
            secondary: SettingsEntries::default().secondary_language.language(),
        }
    }
}

/// Abschnitt `[toene]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct SoundsContent {
    #[serde(rename = "an")]
    enabled: SoundSwitch,
    #[serde(rename = "lautstaerke")]
    volume: u16,
}

impl Default for SoundsContent {
    fn default() -> Self {
        let defaults = SettingsEntries::default();
        Self {
            enabled: defaults.sounds,
            volume: u16::from(defaults.volume.percent()),
        }
    }
}

/// Abschnitt `[modell]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct ModelContent {
    #[serde(rename = "datei")]
    file: String,
    #[serde(rename = "entladen_nach_minuten")]
    unload_after_minutes: u16,
}

impl Default for ModelContent {
    fn default() -> Self {
        let defaults = SettingsEntries::default();
        Self {
            file: defaults.model_file,
            unload_after_minutes: defaults.unload_minutes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn template_gives_defaults() {
        let template = SettingsEntries::default().render();
        assert_eq!(
            SettingsEntries::parse(&template).unwrap().value,
            SettingsEntries::default()
        );
        assert_eq!(
            SettingsEntries::parse("").unwrap().value,
            SettingsEntries::default()
        );
    }

    #[test]
    fn written_values_come_back() {
        let entries = SettingsEntries {
            key: TriggerKey::RightCommand,
            sounds: SoundSwitch::Off,
            volume: Volume::try_from(80).unwrap(),
            model_file: "modell \"klein\".gguf".to_owned(),
            unload_minutes: 0,
            secondary_language: SecondaryLanguage::try_from(Language::Spanish).unwrap(),
            parts: PartTiming::AtEnd,
            log: Logging::Off,
            polish_backend: PolishBackend::Local,
            polish_model: "gemma-4-12B-it-Q4_0.gguf".to_owned(),
            polish_unload_minutes: 3,
            timestamps: Timestamps::On,
            chapters: ChapterHeadings::Off,
            ui_language: UiLanguage::English,
        };
        assert_eq!(
            SettingsEntries::parse(&entries.render()).unwrap().value,
            entries
        );
        let settings = entries.settings();
        assert_eq!(settings.secondary, Language::Spanish);
        assert_eq!(settings.chapters, ChapterHeadings::Off);
        assert_eq!(
            Settings {
                secondary: Language::Korean,
                sounds: Sounds::On(Volume::QUIET),
                unload: Unload::AfterIdle(Duration::from_secs(60)),
                ..settings.clone()
            }
            .needs_restart_from(&settings),
            Restart::NotNeeded,
            "Zweitsprache, Töne und Entladen gelten ohne Neustart"
        );
        assert_eq!(
            Settings {
                key: TriggerKey::Fn,
                ..settings.clone()
            }
            .needs_restart_from(&settings),
            Restart::Needed,
            "eine andere Taste braucht einen Neustart"
        );
        assert_eq!(settings.sounds, Sounds::Off);
        assert_eq!(settings.unload, Unload::Never);
        assert_eq!(settings.parts, PartTiming::AtEnd);
        assert_eq!(
            Settings {
                parts: PartTiming::default(),
                ..settings.clone()
            }
            .needs_restart_from(&settings),
            Restart::NotNeeded,
            "die Teile gelten ohne Neustart"
        );
    }

    #[test]
    fn default_parts_start_after_five_seconds() {
        assert_eq!(
            SettingsEntries::default().settings().parts,
            PartTiming::After(crate::audio::chunk::PartSeconds::DEFAULT)
        );
        let entries = SettingsEntries::parse("[diktat]\nteile_ab_sekunden = 20\n")
            .unwrap()
            .value;
        assert_eq!(u8::from(entries.parts), 20);
        assert_eq!(entries.ui_language, UiLanguage::German);
    }

    #[test]
    fn empty_polish_model_means_the_first() {
        assert_eq!(
            SettingsEntries::default().settings().polish.model_file,
            None
        );
        let entries = SettingsEntries {
            polish_model: "gemma.gguf".to_owned(),
            ..SettingsEntries::default()
        };
        assert_eq!(
            entries.settings().polish.model_file.as_deref(),
            Some("gemma.gguf")
        );
    }

    #[test]
    fn reads_partial_polish_section() {
        let entries = SettingsEntries::parse("[ueberarbeiten]\nmit = \"lokal\"\nmodell = \"\"\n")
            .unwrap()
            .value;
        assert_eq!(entries.polish_backend, PolishBackend::Local);
        assert_eq!(entries.polish_unload_minutes, DEFAULT_POLISH_UNLOAD_MINUTES);
    }

    #[test]
    fn errors_read_in_both_languages() {
        let error = SettingsEntries::parse("[toene]\nlautstaerke = 150\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "Einstellungen fehlerhaft: lautstaerke = 150, erlaubt sind 0 bis 100"
        );
        assert_eq!(
            error.english(),
            "settings faulty: lautstaerke = 150, allowed are 0 to 100"
        );
    }

    #[test]
    fn rejects_mistakes() {
        assert!(
            matches!(
                SettingsEntries::parse("[toene]\nlautstaerke = 150\n"),
                Err(SettingsError::Volume(VolumeError(150)))
            ),
            "Lautstärke über 100 wird abgelehnt"
        );
        assert!(
            matches!(
                SettingsEntries::parse("[tasten]\ndiktat = \"linke_maustaste\"\n"),
                Err(SettingsError::Parse(_))
            ),
            "eine unbekannte Taste wird abgelehnt"
        );
        assert!(
            matches!(
                SettingsEntries::parse("[diktat]\nteile_ab_sekunden = 300\n"),
                Err(SettingsError::PartSeconds(PartSecondsError(300)))
            ),
            "mehr als 20 Sekunden bis zum ersten Teil werden abgelehnt"
        );
        assert!(
            matches!(
                SettingsEntries::parse("[oberflaeche]\nsprache = \"fr\"\n"),
                Err(SettingsError::Parse(_))
            ),
            "eine unbekannte Sprache der Oberfläche wird abgelehnt"
        );
        let current = textfile::format_line(FORMAT_VERSION);
        assert!(
            matches!(
                SettingsEntries::parse(&format!("{current}[sprachen]\nzweite = \"de\"\n")),
                Err(SettingsError::Secondary(_))
            ),
            "Deutsch als zweite Sprache wird abgelehnt"
        );
        assert!(
            matches!(
                SettingsEntries::parse(&format!("{current}[diktat]\nteile_ab_sekunden = 30\n")),
                Err(SettingsError::PartSeconds(PartSecondsError(30)))
            ),
            "im heutigen Format gilt die heutige Grenze"
        );
        assert!(
            matches!(
                SettingsEntries::parse("[toene]\nlaut = 10\n"),
                Err(SettingsError::Parse(_))
            ),
            "ein unbekannter Schlüssel wird abgelehnt"
        );
    }

    #[test]
    fn a_parse_error_carries_no_file_content() {
        // Ein Tippfehler in einer Zeile, die einen Namen enthält.
        let error =
            SettingsEntries::parse("[ueberarbeiten]\nmodell = \"Mandantin Gerlinde Wöhrl\n")
                .unwrap_err();
        let note = error.note();
        for shown in [error.to_string(), note.en().to_owned()] {
            assert!(!shown.contains("Gerlinde"), "Inhalt der Datei: {shown}");
            assert!(
                shown.contains("Zeile 2") || shown.contains("line 2"),
                "{shown}"
            );
        }
    }

    /// Je eine Datei jeder früheren Fassung, so wie diese Version sie
    /// geschrieben hat (mit Werten, die keine Standardwerte sind).
    const PAST_VERSIONS: [(&str, &str); 5] = [
        ("ebf5a60", include_str!("../fixtures/settings/ebf5a60.toml")),
        ("ccf2eeb", include_str!("../fixtures/settings/ccf2eeb.toml")),
        ("894e913", include_str!("../fixtures/settings/894e913.toml")),
        ("8fcb7c3", include_str!("../fixtures/settings/8fcb7c3.toml")),
        ("14c618b", include_str!("../fixtures/settings/14c618b.toml")),
    ];

    /// Was alle Beispiele gemeinsam haben.
    fn written_by_the_past() -> SettingsEntries {
        SettingsEntries {
            key: TriggerKey::RightCommand,
            sounds: SoundSwitch::Off,
            volume: Volume::try_from(40).unwrap(),
            model_file: "cohere-transcribe-03-2026-Q5_K_M.gguf".to_owned(),
            unload_minutes: 15,
            secondary_language: SecondaryLanguage::try_from(Language::Spanish).unwrap(),
            parts: PartTiming::default(),
            log: Logging::Off,
            polish_backend: PolishBackend::Local,
            polish_model: "gemma-4-12B-it-Q4_0.gguf".to_owned(),
            polish_unload_minutes: 5,
            timestamps: Timestamps::On,
            // Vor `kapitel`: Überschriften standen immer im Transkript.
            chapters: ChapterHeadings::On,
            ui_language: UiLanguage::German,
        }
    }

    #[test]
    fn reads_every_past_version() {
        for (commit, text) in PAST_VERSIONS {
            let read = SettingsEntries::parse(text);
            assert!(read.is_ok(), "Datei von {commit} bleibt lesbar: {read:?}");
            let read = read.unwrap();
            let (expected, adjusted) = match commit {
                // Vor [diktat] und [oberflaeche].
                "ebf5a60" | "ccf2eeb" => (written_by_the_past(), Vec::new()),
                // `teile_ab_sekunden = 30`, damals erlaubt.
                "894e913" => (
                    SettingsEntries {
                        parts: PartTiming::try_from(MAX_PART_SECONDS).unwrap(),
                        ui_language: UiLanguage::English,
                        ..written_by_the_past()
                    },
                    vec![Adjustment::PartSeconds { written: 30 }],
                ),
                _ => (
                    SettingsEntries {
                        parts: PartTiming::try_from(12).unwrap(),
                        ui_language: UiLanguage::English,
                        ..written_by_the_past()
                    },
                    Vec::new(),
                ),
            };
            assert_eq!(read.value, expected, "Datei von {commit}");
            assert_eq!(read.adjustments, adjusted, "Datei von {commit}");
        }
    }

    /// settings.toml wie von 894e913 geschrieben, Schieber auf 30 s (damals
    /// 1 bis 60 erlaubt): Die übrigen Einstellungen bleiben, der Wert wird
    /// ersetzt und gemeldet.
    #[test]
    fn reads_a_file_written_by_the_previous_version() {
        let read = SettingsEntries::parse(PAST_VERSIONS[2].1);
        assert!(
            read.is_ok(),
            "eine Datei der Vorversion bleibt lesbar: {read:?}"
        );
        let read = read.unwrap();
        assert_eq!(read.value.key, TriggerKey::RightCommand);
        let note = read.note().unwrap();
        assert_eq!(
            note.de(),
            "Einstellungen: teile_ab_sekunden = 30 ist nicht mehr erlaubt, es gilt 20. Speichern im Panel trägt das in die Datei ein."
        );
        assert_eq!(
            note.en(),
            "settings: teile_ab_sekunden = 30 is no longer allowed, 20 applies. Saving in the panel writes this into the file."
        );
        assert!(
            matches!(
                SettingsEntries::parse("[diktat]\nteile_ab_sekunden = 61\n"),
                Err(SettingsError::PartSeconds(PartSecondsError(61)))
            ),
            "mehr als 60 war nie erlaubt"
        );
    }

    #[test]
    fn german_as_second_language_in_an_old_file_becomes_english() {
        let read = SettingsEntries::parse("[sprachen]\nzweite = \"de\"\n").unwrap();
        assert_eq!(read.value.secondary_language, SecondaryLanguage::ENGLISH);
        assert_eq!(read.adjustments, [Adjustment::SecondaryGerman]);
        assert!(read.note().is_some(), "das wird gemeldet");
    }

    #[test]
    fn the_current_version_writes_its_format_and_reads_without_notes() {
        let text = written_by_the_past().render();
        assert!(
            text.starts_with(&textfile::format_line(FORMAT_VERSION)),
            "die Kennzeile steht vorn"
        );
        let read = SettingsEntries::parse(&text).unwrap();
        assert_eq!(read.value, written_by_the_past());
        assert_eq!(read.note(), None);
    }

    #[test]
    fn a_file_of_a_newer_version_is_refused_and_kept() {
        let dir = TempDir::new("settings-newer");
        let newer = format!(
            "{}[tasten]\ndiktat = \"fn\"\n",
            textfile::format_line(FORMAT_VERSION + 1)
        );
        let path = dir.file("settings.toml", &newer);
        let read = SettingsEntries::read(&path);
        let after = fs::read_to_string(&path).unwrap();
        assert!(
            matches!(
                read,
                Err(SettingsError::Format(FormatError::Unknown {
                    found: 4,
                    current: 3
                }))
            ),
            "ein unbekanntes Format wird abgelehnt: {read:?}"
        );
        assert_eq!(after, newer, "die Datei bleibt, wie sie ist");
    }

    /// Speichert das Panel, gehen eigene Kommentare verloren; der Kopf der
    /// Datei sagt das (wie im Wörterbuch).
    #[test]
    fn panel_save_keeps_comments_written_by_hand() {
        let dir = TempDir::new("settings-notes");
        let path = dir.file(
            "settings.toml",
            format!("{}\n# Meine Notiz\n", SettingsEntries::default().render()),
        );
        let entries = SettingsEntries::read(&path).unwrap().value;
        entries.write(&path).unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert!(
            after.contains("Meine Notiz") || after.contains("eigene Kommentare gehen"),
            "eine eigene Notiz bleibt, oder der Kopf warnt wie im Wörterbuch"
        );
    }

    #[test]
    fn a_new_model_file_needs_a_restart() {
        let running = Settings::default();
        assert_eq!(
            Settings {
                model_file: "cohere-transcribe-03-2026-Q5_K_M.gguf".to_owned(),
                ..running.clone()
            }
            .needs_restart_from(&running),
            Restart::Needed,
            "eine andere Modelldatei braucht einen Neustart"
        );
    }

    #[test]
    fn unload_minutes_become_minutes() {
        assert_eq!(
            Settings::default().unload,
            Unload::AfterIdle(Duration::from_mins(30))
        );
        assert_eq!(
            Settings::default().polish.unload,
            Unload::AfterIdle(Duration::from_mins(10))
        );
    }

    #[test]
    fn the_template_lists_every_key_and_language() {
        let template = SettingsEntries::default().render();
        let readme = include_str!("../README.md");
        let row = |key: &str| {
            readme
                .lines()
                .find(|line| line.contains(key))
                .unwrap_or_default()
                .to_owned()
        };
        let keys_row = row("| `diktat`:");
        let languages_row = row("| `zweite`:");
        for key in TriggerKey::ALL {
            let value = format!("\"{}\"", key.file_value());
            assert!(template.contains(&value), "Vorlage nennt {value}");
            assert!(
                keys_row.contains(&format!("`{value}`")),
                "README nennt {value}"
            );
        }
        for secondary in SecondaryLanguage::all() {
            let language = secondary.language();
            let entry = format!("\"{}\" {}", language.code(), language.name());
            assert!(template.contains(&entry), "Vorlage nennt {entry}");
            let code = format!("`{}`", language.code());
            assert!(languages_row.contains(&code), "README nennt {code}");
        }
        assert!(
            !language_list().contains("\"de\""),
            "Deutsch ist keine zweite Sprache"
        );
        assert!(
            template
                .lines()
                .all(|line| line.chars().count() <= COMMENT_WIDTH + 2),
            "keine überlange Zeile"
        );
    }

    /// Das Panel schickt Zahlen; was außerhalb der Grenzen liegt, wird schon
    /// beim Lesen der Nachricht abgelehnt und erreicht nie die Datei.
    #[test]
    #[expect(
        clippy::disallowed_types,
        reason = "baut Nachrichten mit Werten, die `SettingsEntries` gar nicht darstellen kann"
    )]
    fn panel_values_out_of_range_never_reach_the_file() {
        let panel = |field: &str, value: serde_json::Value| {
            let mut json = serde_json::to_value(SettingsEntries::default()).unwrap();
            json[field] = value;
            serde_json::from_value::<SettingsEntries>(json)
        };
        assert!(
            panel("parts", 15.into()).is_ok(),
            "ein gültiger Wert geht durch"
        );
        for (field, value) in [
            ("parts", serde_json::Value::from(90)),
            ("parts", (MAX_PART_SECONDS + 1).into()),
            ("volume", 200.into()),
            ("volume", (-1).into()),
            ("secondary_language", "de".into()),
            ("unload_minutes", 70_000.into()),
        ] {
            assert!(
                panel(field, value.clone()).is_err(),
                "{field} = {value} abgelehnt"
            );
        }
    }

    /// Jede Dauer bis zum Entladen mit jedem Modell zum Überarbeiten und
    /// jeder Sprache des Panels.
    fn unload_backend_and_language() -> Vec<(u16, PolishBackend, UiLanguage)> {
        [0, 1, u16::MAX]
            .into_iter()
            .flat_map(|minutes| {
                [PolishBackend::Apple, PolishBackend::Local]
                    .into_iter()
                    .flat_map(move |backend| {
                        [UiLanguage::German, UiLanguage::English]
                            .map(|language| (minutes, backend, language))
                    })
            })
            .collect()
    }

    /// Was das Panel schicken kann, ergibt eine Datei, die sich wieder so
    /// liest: Speichern macht die Datei nie unlesbar.
    #[test]
    fn every_panel_value_renders_a_file_that_loads() {
        let names = [
            "",
            "m.gguf",
            "mit \"Anführung\"",
            "zwei\nZeilen",
            "a\\b",
            "ünï ☃",
        ];
        let mut cases = Vec::new();
        for key in TriggerKey::ALL {
            for secondary in SecondaryLanguage::all() {
                cases.push(SettingsEntries {
                    key,
                    secondary_language: secondary,
                    ..SettingsEntries::default()
                });
            }
        }
        for seconds in 0..=MAX_PART_SECONDS {
            for percent in [0, 1, 100] {
                cases.push(SettingsEntries {
                    parts: PartTiming::try_from(seconds).unwrap(),
                    volume: Volume::try_from(percent).unwrap(),
                    sounds: SoundSwitch::from(seconds % 2 == 0),
                    log: Logging::from(seconds % 3 == 0),
                    timestamps: Timestamps::from(seconds % 5 == 0),
                    chapters: ChapterHeadings::from(seconds % 7 == 0),
                    ..SettingsEntries::default()
                });
            }
        }
        let combinations = unload_backend_and_language();
        for name in names {
            for &(minutes, polish_backend, ui_language) in &combinations {
                cases.push(SettingsEntries {
                    model_file: name.to_owned(),
                    polish_model: name.to_owned(),
                    unload_minutes: minutes,
                    polish_unload_minutes: minutes,
                    polish_backend,
                    ui_language,
                    ..SettingsEntries::default()
                });
            }
        }
        for entries in cases {
            let read = SettingsEntries::parse(&entries.render())
                .unwrap_or_else(|error| panic!("{entries:?} ergibt eine unlesbare Datei: {error}"));
            assert_eq!(read.value, entries);
            assert!(read.adjustments.is_empty(), "nichts ersetzt: {entries:?}");
        }
    }
}
