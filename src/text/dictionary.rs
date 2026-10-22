//! Das Wörterbuch (`dictionary.toml`): eigene Ersetzungen und Jura-Regeln.
//!
//! Die Datei wird beim ersten Start mit Erklärungen angelegt und vor jedem
//! Diktat neu gelesen, wenn sie sich geändert hat – ein Neustart ist nicht
//! nötig.
//!
//! Das Format trägt eine Nummer ([`FORMAT_VERSION`], als Kommentar in der
//! ersten Zeile, siehe [`textfile::format_of`]); ein Beispiel jeder
//! früheren Fassung liegt unter `fixtures/dictionary/`.

use crate::note::{Explain, Note};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::legal::{self, CitationStyle};
use super::statutes::{self, Statutes};
use super::terms::{self, Terms};
use crate::paths::{self, PathsError};
use crate::textfile::{self, Format, FormatError, TextFileError, TomlError};

/// Nummer des Formats, das diese Version schreibt.
///
/// 1: ohne Kennzeile, bis einschließlich 973fa8b.
/// 2: mit Kennzeile; sonst wie 1.
pub(crate) const FORMAT_VERSION: u16 = 2;

/// Kopf der Datei mit Erklärungen, deutsch und englisch wie alles, was der
/// Nutzer liest; die Werte folgen in [`DictionaryEntries::render`].
const HEADER: &str = "\
# Diktator – Wörterbuch
#
# Bearbeitbar im Panel (Reiter „Wörterbuch“) oder hier. Änderungen gelten ab
# dem nächsten Diktat; ein Neustart ist nicht nötig. Ist die Datei
# fehlerhaft, meldet Diktator das und arbeitet mit dem letzten fehlerfreien
# Stand seit dem Start weiter; war sie schon beim Start fehlerhaft, gelten
# bis zur Korrektur die Standardregeln. Speichert das Panel, schreibt es die
# Datei neu (eigene Kommentare gehen dabei verloren).
#
# Diktator – dictionary
#
# Edit it in the panel (tab “Dictionary”) or here. Changes apply from the
# next dictation; no restart is needed. If the file is faulty, Diktator says
# so and keeps the last error-free state since it started; if it was faulty
# already at start, the default rules apply until it is fixed. When the
# panel saves, it rewrites the file (your own comments are lost).
";

/// Erklärung vor `[ersetzungen]`.
const REPLACEMENTS_HELP: &str = "\
# Ersetzungen: links, was das Modell schreibt, rechts, was eingefügt wird.
# Ersetzt werden nur ganze Wörter (auch mehrere hintereinander). Groß- und
# Kleinschreibung zählt; ein kleingeschriebenes Wort wird auch
# großgeschrieben erkannt (etwa am Satzanfang) und dann großgeschrieben
# ersetzt.
# Die Ersetzungen kommen vor den Jura-Regeln dran. Beispiel:
# \"Extunknichtigkeit\" = \"Ex-tunc-Nichtigkeit\"
#
# Replacements: on the left what the model writes, on the right what is
# inserted. Only whole words are replaced (several in a row too). Case
# matters; a lower-case word is also found capitalised (at the start of a
# sentence, say) and then replaced capitalised. Replacements run before the
# legal rules.
";

/// Erklärung vor `[fachbegriffe]`, mit den Grenzen aus [`terms`].
fn terms_help() -> String {
    let (min, long) = (terms::MIN_CHARS, terms::LONG_CHARS);
    format!(
        "\
# Fachbegriffe: Erkannte Wörter, die einem Begriff sehr ähnlich sind (ab {min}
# Buchstaben, 1 Zeichen Abweichung, ab {long} Buchstaben 2), werden zu diesem
# Begriff – „Subsomo“ → „Subsumo“. Gebeugte Formen („Subsumos“) bleiben.
# Nur einzelne Wörter; Wortgruppen gehören in die Ersetzungen.
#
# Technical terms: recognised words very similar to a term ({min} letters or
# more, 1 character off, from {long} letters 2) become that term – “Subsomo”
# → “Subsumo”. Inflected forms (“Subsumos”) stay. Single words only; groups
# of words belong in the replacements.
"
    )
}

/// Gesetze, die die Erklärung vor `[jura]` beim Namen nennt.
const NAMED_STATUTES: [&str; 5] = ["BGB", "StGB", "ZPO", "GG", "VwGO"];

/// Erklärung vor `[jura]`, mit der Zahl der eingebauten Gesetze.
fn legal_help() -> String {
    let named = NAMED_STATUTES.join(", ");
    let others = statutes::known_names()
        .filter(|name| !NAMED_STATUTES.contains(name))
        .count();
    format!(
        "\
# Normzitate (nur bei deutschen Diktaten und Videos):
# „Paragraf 823 Absatz 1 Satz 2 BGB“ → „§ 823 I 2 BGB“.
# absatz: \"römisch\" (§ 823 I 2 BGB) oder \"abgekürzt\" (§ 823 Abs. 1 S. 2 BGB)
# gesetze: weitere Gesetzeskürzel, die nach einer Zahl als Gesetz gelten.
# Bekannt sind schon {named} und {others} weitere. Kürzel
# mit „Bay“ vorne zählen nach Artikeln (Art.), alle anderen nach Paragrafen.
#
# Legal citations (German dictations and videos only):
# “Paragraf 823 Absatz 1 Satz 2 BGB” → “§ 823 I 2 BGB”.
# absatz: \"römisch\" (§ 823 I 2 BGB) or \"abgekürzt\" (§ 823 Abs. 1 S. 2 BGB)
# gesetze: more statute abbreviations that count as a statute after a number.
# {named} and {others} more are built in. Abbreviations starting with
# “Bay” count articles (Art.), all others paragraphs (§).
"
    )
}

/// Inhalt einer neu angelegten Datei.
fn template() -> String {
    DictionaryEntries::default().render()
}

/// Fehler beim Lesen oder Anlegen des Wörterbuchs.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DictionaryError {
    /// Kein Ablageort für die Datei.
    #[error("Wörterbuch: {0}")]
    Paths(#[from] PathsError),
    /// Die Datei ließ sich nicht lesen.
    #[error("Wörterbuch {} nicht lesbar: {source}", paths::tilde(.path))]
    Read {
        /// Pfad der Datei.
        path: PathBuf,
        /// Ursache.
        #[source]
        source: io::Error,
    },
    /// Die Datei ließ sich nicht anlegen oder öffnen.
    #[error("Wörterbuch: {0}")]
    File(#[from] TextFileError),
    /// Die Datei ließ sich nicht schreiben.
    #[error("Wörterbuch {} nicht speicherbar: {source}", paths::tilde(.path))]
    Write {
        /// Pfad der Datei.
        path: PathBuf,
        /// Ursache.
        #[source]
        source: io::Error,
    },
    /// Die Datei ist kein gültiges Wörterbuch.
    #[error("Wörterbuch fehlerhaft{}", .0.german())]
    Parse(TomlError),
    /// Die Kennzeile nennt kein bekanntes Format.
    #[error("Wörterbuch nicht lesbar: {0}")]
    Format(#[from] FormatError),
}

impl Explain for DictionaryError {
    fn english(&self) -> String {
        match self {
            Self::Paths(source) => format!("dictionary: {}", source.english()),
            Self::Read { path, source } => {
                format!("dictionary {} not readable: {source}", paths::tilde(path))
            }
            Self::File(source) => format!("dictionary: {}", source.english()),
            Self::Write { path, source } => {
                format!(
                    "dictionary {} cannot be saved: {source}",
                    paths::tilde(path)
                )
            }
            Self::Parse(source) => format!("dictionary faulty{}", source.english()),
            Self::Format(source) => format!("dictionary not readable: {}", source.english()),
        }
    }
}

/// Liest den Dateiinhalt, in jedem bekannten Format.
///
/// # Errors
/// [`DictionaryError::Format`] bei einem unbekannten Format,
/// [`DictionaryError::Parse`], wenn der Text kein gültiges Wörterbuch ist.
fn file_content(text: &str) -> Result<FileContent, DictionaryError> {
    match textfile::format_of(text, FORMAT_VERSION)? {
        // Format 1 und 2 haben dieselben Schlüssel.
        Format::Legacy | Format::Current => toml::from_str(text)
            .map_err(|error| DictionaryError::Parse(TomlError::new(text, &error))),
    }
}

/// Die Datei, wie sie geschrieben steht.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileContent {
    #[serde(default, rename = "ersetzungen")]
    replacements: BTreeMap<String, String>,
    #[serde(default)]
    jura: LegalContent,
    #[serde(default, rename = "fachbegriffe")]
    terms: TermsContent,
}

/// Abschnitt `[fachbegriffe]`, wie er geschrieben steht.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct TermsContent {
    #[serde(rename = "liste")]
    list: Vec<String>,
}

/// Abschnitt `[jura]`, wie er geschrieben steht.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct LegalContent {
    #[serde(rename = "an")]
    enabled: bool,
    #[serde(rename = "absatz")]
    style: CitationStyle,
    #[serde(rename = "gesetze")]
    statutes: Vec<String>,
}

impl Default for LegalContent {
    fn default() -> Self {
        Self {
            enabled: true,
            style: CitationStyle::default(),
            statutes: Vec::new(),
        }
    }
}

/// Der Inhalt der Datei als bearbeitbare Werte, so wie das Panel ihn zeigt
/// und zurückschickt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DictionaryEntries {
    /// Eigene Ersetzungen (alphabetisch).
    pub replacements: Vec<ReplacementEntry>,
    /// Jura-Regeln an?
    pub legal: bool,
    /// Schreibweise der Absätze.
    pub style: CitationStyle,
    /// Zusätzliche Gesetzeskürzel.
    pub statutes: Vec<String>,
    /// Fachbegriffe, zu denen ähnlich erkannte Wörter werden.
    #[serde(default)]
    pub terms: Vec<String>,
}

/// Eine eigene Ersetzung.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplacementEntry {
    /// Was das Modell schreibt.
    pub from: String,
    /// Was eingefügt wird.
    pub to: String,
}

impl Default for DictionaryEntries {
    fn default() -> Self {
        Self::from_content(FileContent::default())
    }
}

impl DictionaryEntries {
    /// Liest die Datei; legt sie vorher an, falls sie fehlt.
    ///
    /// # Errors
    /// [`DictionaryError`], wenn die Datei nicht anlegbar, nicht lesbar oder
    /// fehlerhaft ist.
    pub(crate) fn read(path: &Path) -> Result<Self, DictionaryError> {
        ensure_file(path)?;
        let text = fs::read_to_string(path).map_err(|source| DictionaryError::Read {
            path: path.to_owned(),
            source,
        })?;
        Ok(Self::from_content(file_content(&text)?))
    }

    /// Schreibt die Werte samt Erklärungen in die Datei. Leere Ersetzungen
    /// und Kürzel fallen weg; steht dasselbe Wort zweimal da, gilt die
    /// letzte Ersetzung (die neueste aus dem Panel).
    ///
    /// # Errors
    /// [`DictionaryError`], wenn die Datei nicht geschrieben werden kann.
    pub(crate) fn write(&self, path: &Path) -> Result<(), DictionaryError> {
        let text = self.render();
        // Sicherheitsnetz: Was geschrieben wird, muss sich wieder lesen lassen.
        Dictionary::parse(&text)?;
        textfile::replace(path, &text).map_err(|source| DictionaryError::Write {
            path: path.to_owned(),
            source,
        })
    }

    /// Der Dateiinhalt mit Erklärungen.
    #[must_use]
    pub(crate) fn render(&self) -> String {
        let unique: BTreeMap<&str, &str> = self
            .replacements
            .iter()
            .map(|entry| (entry.from.trim(), entry.to.trim()))
            .filter(|(from, _)| !from.is_empty())
            .collect();
        let replacements: Vec<String> = unique
            .into_iter()
            .map(|(from, to)| format!("{} = {}", quote(from), quote(to)))
            .collect();
        let quoted = |list: &[String]| -> String {
            list.iter()
                .map(|entry| entry.trim())
                .filter(|entry| !entry.is_empty())
                .map(quote)
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "{}{HEADER}\n{REPLACEMENTS_HELP}[ersetzungen]\n{}\n\n{}[fachbegriffe]\nliste = [{}]\n\n{}[jura]\nan = {}\nabsatz = {}\ngesetze = [{}]\n",
            textfile::format_line(FORMAT_VERSION),
            replacements.join("\n"),
            terms_help(),
            quoted(&self.terms),
            legal_help(),
            self.legal,
            quote(self.style.file_value()),
            quoted(&self.statutes)
        )
    }

    /// Aus dem gelesenen Dateiinhalt. Schlüssel werden getrimmt wie in
    /// [`Dictionary`]; ergeben zwei dasselbe Wort („ bgbat“, „bgbat“), gilt
    /// wie dort die erste, damit das Panel zeigt, was gilt, und speichern
    /// kann.
    fn from_content(content: FileContent) -> Self {
        let mut replacements: Vec<ReplacementEntry> = Vec::new();
        for (from, to) in content.replacements {
            let from = from.trim();
            if !from.is_empty() && !replacements.iter().any(|entry| entry.from == from) {
                replacements.push(ReplacementEntry {
                    from: from.to_owned(),
                    to,
                });
            }
        }
        replacements.sort_by(|left, right| left.from.cmp(&right.from));
        Self {
            replacements,
            legal: content.jura.enabled,
            style: content.jura.style,
            statutes: content.jura.statutes,
            terms: content.terms.list,
        }
    }
}

/// Ein TOML-Text in Anführungszeichen, mit maskierten Sonderzeichen.
fn quote(text: &str) -> String {
    toml::Value::String(text.to_owned()).to_string()
}

/// Eine Wortersetzung.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Replacement {
    from: String,
    to: String,
}

/// Jura-Regeln, wenn eingeschaltet.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Legal {
    style: CitationStyle,
    statutes: Statutes,
}

/// Ein gelesenes Wörterbuch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Dictionary {
    /// Längste zuerst, damit „BGB AT“ vor „BGB“ greift.
    replacements: Vec<Replacement>,
    terms: Terms,
    legal: Option<Legal>,
}

impl Default for Dictionary {
    /// Ohne Datei: keine Ersetzungen, Jura-Regeln römisch.
    fn default() -> Self {
        Self::from_content(&FileContent::default())
    }
}

impl Dictionary {
    /// Liest ein Wörterbuch aus TOML-Text.
    ///
    /// # Errors
    /// [`DictionaryError::Parse`], wenn der Text kein gültiges Wörterbuch ist.
    pub(crate) fn parse(text: &str) -> Result<Self, DictionaryError> {
        Ok(Self::from_content(&file_content(text)?))
    }

    /// Ersetzt die eigenen Wörter (ganze Wörter, längere zuerst).
    pub(crate) fn replace(&self, text: &str) -> String {
        replace_words(text, &self.replacements)
    }

    /// Macht aus Wörtern, die einem Fachbegriff sehr ähnlich sind, den
    /// Begriff.
    pub(crate) fn fix_terms(&self, text: &str) -> String {
        self.terms.apply(text)
    }

    /// Bringt Normzitate in die eingestellte Schreibweise, wenn die
    /// Jura-Regeln an sind.
    pub(crate) fn cite(&self, text: &str) -> String {
        self.legal.as_ref().map_or_else(
            || text.to_owned(),
            |legal| legal::format(text, legal.style, &legal.statutes),
        )
    }

    /// Wandelt den Dateiinhalt in die Regeln um.
    fn from_content(content: &FileContent) -> Self {
        let mut replacements: Vec<Replacement> = content
            .replacements
            .iter()
            // Leerraum am Rand eines von Hand geschriebenen Schlüssels
            // („ bgbat“) gehört nicht zum Wort.
            .map(|(from, to)| (from.trim(), to))
            .filter(|(from, _)| !from.is_empty())
            .flat_map(|(from, to)| {
                let written = Replacement {
                    from: from.to_owned(),
                    to: to.clone(),
                };
                // Etwa am Satzanfang schreibt das Modell auch ein kleines Wort groß.
                let capitalized = capitalized(from)
                    .filter(|upper| upper != from && !content.replacements.contains_key(upper))
                    .map(|upper| Replacement {
                        from: upper,
                        // Leer bleibt leer: eine Ersetzung, die das Wort streicht.
                        to: capitalized(to).unwrap_or_default(),
                    });
                std::iter::once(written).chain(capitalized)
            })
            .collect();
        replacements.sort_by_key(|replacement| std::cmp::Reverse(replacement.from.chars().count()));
        let legal = content.jura.enabled.then(|| Legal {
            style: content.jura.style,
            statutes: Statutes::new(&content.jura.statutes),
        });
        Self {
            replacements,
            terms: Terms::new(&content.terms.list),
            legal,
        }
    }
}

/// Was beim Nachsehen nach der Datei herauskam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refresh {
    /// Unverändert seit dem letzten Lesen.
    Unchanged,
    /// Neu gelesen.
    Reloaded,
    /// Fehlte und wurde mit Erklärungen angelegt.
    Created,
}

/// Das Wörterbuch auf der Festplatte; liest es neu, wenn es sich ändert.
#[derive(Debug)]
pub(crate) struct DictionaryFile {
    path: PathBuf,
    /// Änderungszeit beim letzten gelungenen Lesen, auch eines fehlerhaften
    /// Stands (damit ein Inhaltsfehler nur einmal gemeldet wird).
    modified: Option<SystemTime>,
    dictionary: Dictionary,
    /// Woher die geltenden Regeln stammen.
    rules: Rules,
}

/// Woher die geltenden Regeln eines [`DictionaryFile`] stammen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rules {
    /// Die Standardregeln: Die Datei war seit dem Start nie fehlerfrei.
    Defaults,
    /// Der letzte fehlerfreie Stand der Datei.
    File,
}

impl DictionaryFile {
    /// Noch nicht gelesenes Wörterbuch unter `path`; bis zum ersten
    /// [`refresh`](Self::refresh) gelten die Standardregeln.
    #[must_use]
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            modified: None,
            dictionary: Dictionary::default(),
            rules: Rules::Defaults,
        }
    }

    /// Die Meldung zu `error` aus [`refresh`](Self::refresh), mit den
    /// Regeln, die nun gelten: Nach einem Neustart sind das die
    /// Standardregeln, nicht der letzte fehlerfreie Stand von vorher.
    #[must_use]
    pub(crate) fn failure_note(&self, error: &DictionaryError) -> Note {
        let (de, en) = match self.rules {
            Rules::Defaults => (
                "Seit dem Start gelten die Standardregeln.",
                "The default rules apply since the start.",
            ),
            Rules::File => (
                "Es gilt der letzte fehlerfreie Stand.",
                "The last error-free version applies.",
            ),
        };
        error
            .note()
            .around(|text| format!("{text} {de}"), |text| format!("{text} {en}"))
    }

    /// Die geltenden Regeln.
    #[must_use]
    pub(crate) const fn dictionary(&self) -> &Dictionary {
        &self.dictionary
    }

    /// Liest die Datei neu, wenn sie sich geändert hat, und legt sie an,
    /// wenn sie fehlt. Bei einem Fehler bleiben die bisherigen Regeln.
    ///
    /// # Errors
    /// [`DictionaryError`], wenn die Datei nicht lesbar, nicht anlegbar oder
    /// fehlerhaft ist.
    pub(crate) fn refresh(&mut self) -> Result<Refresh, DictionaryError> {
        let created = ensure_file(&self.path)?;
        let modified = fs::metadata(&self.path)
            .and_then(|metadata| metadata.modified())
            .map_err(|source| self.read_error(source))?;
        if self.modified == Some(modified) {
            return Ok(Refresh::Unchanged);
        }
        // Erst nach dem Lesen merken: Schlägt das Lesen fehl (etwa wegen
        // fehlender Rechte), wird es beim nächsten Mal wieder versucht. Ein
        // fehlerhafter Inhalt dagegen wird nur einmal gemeldet.
        let text = fs::read_to_string(&self.path).map_err(|source| self.read_error(source))?;
        self.modified = Some(modified);
        self.dictionary = Dictionary::parse(&text)?;
        self.rules = Rules::File;
        Ok(if created {
            Refresh::Created
        } else {
            Refresh::Reloaded
        })
    }

    /// Lesefehler mit Pfad.
    fn read_error(&self, source: io::Error) -> DictionaryError {
        DictionaryError::Read {
            path: self.path.clone(),
            source,
        }
    }
}

/// Legt das Wörterbuch mit Erklärungen an, falls es fehlt. `true`, wenn es
/// neu angelegt wurde.
///
/// # Errors
/// [`DictionaryError::File`], wenn Ordner oder Datei nicht anlegbar sind.
fn ensure_file(path: &Path) -> Result<bool, DictionaryError> {
    Ok(textfile::ensure(path, &template())?)
}

/// Öffnet das Wörterbuch im Standard-Texteditor; legt es vorher an, falls
/// es fehlt.
///
/// # Errors
/// [`DictionaryError::File`], wenn die Datei nicht anlegbar ist oder der
/// Editor nicht startet.
pub(crate) fn open_in_editor(path: &Path) -> Result<(), DictionaryError> {
    Ok(textfile::open_in_editor(path, &template())?)
}

/// Ersetzt ganze Wörter in einem Durchgang. An jeder Stelle gewinnt die
/// erste passende Ersetzung in `replacements` (die längste, siehe
/// [`Dictionary`]); ersetzter Text wird nicht noch einmal ersetzt.
fn replace_words(text: &str, replacements: &[Replacement]) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    let mut previous: Option<char> = None;
    loop {
        let mut chars = rest.chars();
        let Some(next) = chars.next() else {
            break;
        };
        let word_start = !previous.is_some_and(char::is_alphanumeric);
        let hit = replacements
            .iter()
            .filter(|_| word_start)
            .find_map(|replacement| {
                let after = rest.strip_prefix(replacement.from.as_str())?;
                let word_end = !after.chars().next().is_some_and(char::is_alphanumeric);
                word_end.then_some((replacement, after))
            });
        if let Some((replacement, after)) = hit {
            result.push_str(&replacement.to);
            previous = replacement.from.chars().next_back();
            rest = after;
        } else {
            result.push(next);
            previous = Some(next);
            rest = chars.as_str();
        }
    }
    result
}

/// Erster Buchstabe groß; `None` bei leerem Text.
fn capitalized(word: &str) -> Option<String> {
    let mut chars = word.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_survive_writing_and_reading() {
        let entries = DictionaryEntries {
            replacements: vec![
                ReplacementEntry {
                    from: "BGBAT".to_owned(),
                    to: "BGB AT".to_owned(),
                },
                ReplacementEntry {
                    from: "sagt \"so\"".to_owned(),
                    to: "Zitat \\ Pfad".to_owned(),
                },
                ReplacementEntry {
                    from: "  ".to_owned(),
                    to: "leer".to_owned(),
                },
            ],
            legal: false,
            style: CitationStyle::Abbreviated,
            statutes: vec!["KAGB".to_owned(), String::new()],
            terms: vec!["Subsumo".to_owned(), " ".to_owned()],
        };
        let text = entries.render();
        let read = DictionaryEntries::from_content(file_content(&text).unwrap());
        assert_eq!(read.terms, ["Subsumo"]);
        assert_eq!(
            Dictionary::parse(&text)
                .unwrap()
                .fix_terms("Mit Subsomo lernen."),
            "Mit Subsumo lernen."
        );
        assert_eq!(read.replacements.len(), 2);
        assert_eq!(read.replacements[0].to, "BGB AT");
        assert_eq!(read.replacements[1].from, "sagt \"so\"");
        assert_eq!(read.replacements[1].to, "Zitat \\ Pfad");
        assert!(
            !read.legal,
            "die Jura-Regeln bleiben nach Schreiben und Lesen aus"
        );
        assert_eq!(read.style, CitationStyle::Abbreviated);
        assert_eq!(read.statutes, vec!["KAGB".to_owned()]);
        assert_eq!(
            DictionaryEntries::from_content(file_content(&template()).unwrap()),
            DictionaryEntries::default()
        );
    }

    #[test]
    fn template_is_valid_and_matches_defaults() {
        assert_eq!(
            Dictionary::parse(&template()).unwrap(),
            Dictionary::default()
        );
    }

    #[test]
    fn replaces_whole_words_only() {
        let dictionary = Dictionary::parse(
            r#"
            [ersetzungen]
            "Extunknichtigkeit" = "Ex-tunc-Nichtigkeit"
            "BGBAT" = "BGB AT"
            "ag" = "AG"
            "kläger" = "Kläger"
            "#,
        )
        .unwrap();
        assert_eq!(
            dictionary.replace("Die Extunknichtigkeit im BGBAT, sagt die ag."),
            "Die Ex-tunc-Nichtigkeit im BGB AT, sagt die AG."
        );
        assert_eq!(dictionary.replace("Tag, Magie"), "Tag, Magie");
        assert_eq!(dictionary.replace("kläger und Kläger"), "Kläger und Kläger");
    }

    #[test]
    fn trims_hand_written_keys() {
        let dictionary = Dictionary::parse(
            r#"
            [ersetzungen]
            " bgbat " = "BGB AT"
            "#,
        )
        .unwrap();
        assert_eq!(dictionary.replace("da, bgbat"), "da, BGB AT");
    }

    #[test]
    fn prefers_longer_replacements() {
        let dictionary = Dictionary::parse(
            r#"
            [ersetzungen]
            "Schmidt" = "Schmitt"
            "Helmut Schmidt" = "Helmut Schmidt"
            "#,
        )
        .unwrap();
        assert_eq!(
            dictionary.replace("Helmut Schmidt und Frau Schmidt"),
            "Helmut Schmidt und Frau Schmitt"
        );
    }

    #[test]
    fn reads_legal_settings() {
        let dictionary = Dictionary::parse(
            r#"
            [jura]
            absatz = "abgekürzt"
            gesetze = ["BayPAG"]
            "#,
        )
        .unwrap();
        assert_eq!(
            dictionary.cite("Artikel 11 Absatz 1 BayPAG"),
            "Art. 11 Abs. 1 BayPAG"
        );
        let off = Dictionary::parse("[jura]\nan = false\n").unwrap();
        assert_eq!(off.cite("Paragraf 1 BGB"), "Paragraf 1 BGB");
    }

    #[test]
    fn help_names_the_real_limits_everywhere() {
        let (min, long) = (terms::MIN_CHARS, terms::LONG_CHARS);
        let help = terms_help();
        assert!(help.contains(&format!("(ab {min}\n")), "{help}");
        assert!(help.contains(&format!("ab {long} Buchstaben 2")), "{help}");
        let readme = include_str!("../../README.md");
        assert!(
            readme.contains(&format!(
                "(ab {min} Buchstaben, 1 Zeichen Abweichung, ab {long} Buchstaben 2)"
            )),
            "README nennt andere Grenzen als terms.rs"
        );
        let panel = include_str!("../../helpers/ui/DictionaryTab.swift");
        assert!(
            panel.contains(&format!("(ab {min} Buchstaben)"))
                && panel.contains(&format!("({min} letters or more)")),
            "das Panel nennt andere Grenzen als terms.rs"
        );
        for name in NAMED_STATUTES {
            assert!(
                statutes::known_names().any(|known| known == name),
                "{name} ist kein eingebautes Gesetz"
            );
        }
    }

    #[test]
    fn help_is_written_in_german_and_english() {
        let template = template();
        for (german, english) in [
            ("Wörterbuch", "dictionary"),
            ("Ersetzungen:", "Replacements:"),
            ("Fachbegriffe:", "Technical terms:"),
            ("Normzitate", "Legal citations"),
        ] {
            assert!(
                template.contains(german) && template.contains(english),
                "{german} / {english}"
            );
        }
    }

    #[test]
    fn error_message_carries_no_dictionary_content() {
        let error = Dictionary::parse("[ersetzungen]\n\"Mandantin Gerlinde Wöhrl\" = \"Wöhrl\n")
            .unwrap_err();
        for logged in [error.to_string(), error.english(), format!("{error:?}")] {
            assert!(
                !logged.contains("Gerlinde") && !logged.contains("Wöhrl"),
                "Inhalt des Wörterbuchs im Protokoll: {logged}"
            );
        }
        assert!(
            error.to_string().contains("Zeile 2"),
            "die Stelle wird genannt: {error}"
        );
    }

    /// Ein leerer Ordner nur für einen Test.
    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("diktator-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "löscht nur den Ordner, den `test_dir` für diesen Test angelegt hat"
    )]
    fn remove_test_dir(dir: &Path) {
        fs::remove_dir_all(dir).unwrap();
    }

    /// Ersetzt die Datei und setzt ihre Änderungszeit eine Weile hinter die
    /// bisherige, damit `refresh` die Änderung sicher sieht.
    fn rewrite(path: &Path, text: &str) {
        let before = fs::metadata(path).unwrap().modified().unwrap();
        textfile::replace(path, text).unwrap();
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(before + std::time::Duration::from_secs(5))
            .unwrap();
    }

    #[test]
    fn panel_can_add_a_word_next_to_a_hand_written_key() {
        let dir = test_dir("dictionary-hand");
        let path = dir.join("dictionary.toml");
        textfile::ensure(&path, "[ersetzungen]\n\" bgbat\" = \"BGB AT\"\n").unwrap();
        let mut entries = DictionaryEntries::read(&path).unwrap();
        assert_eq!(
            entries.replacements[0].from, "bgbat",
            "das Panel sieht, was gilt"
        );
        // Wie `DictionaryTab.addReplacement`: gleiche Wörter ersetzen.
        entries.replacements.retain(|entry| entry.from != "bgbat");
        entries.replacements.push(ReplacementEntry {
            from: "bgbat".to_owned(),
            to: "BGB-AT".to_owned(),
        });
        let saved = entries.write(&path);
        let read = DictionaryEntries::read(&path).unwrap();
        remove_test_dir(&dir);
        assert!(saved.is_ok(), "das Panel kann speichern: {saved:?}");
        assert_eq!(read.replacements.len(), 1);
        assert_eq!(read.replacements[0].to, "BGB-AT");
    }

    #[test]
    fn render_keeps_the_last_of_two_equal_words() {
        let mut entries = DictionaryEntries::default();
        for to in ["alt", "neu"] {
            entries.replacements.push(ReplacementEntry {
                from: " ag ".to_owned(),
                to: to.to_owned(),
            });
        }
        let read = Dictionary::parse(&entries.render()).unwrap();
        assert_eq!(read.replace("die ag"), "die neu");
    }

    #[test]
    fn dictionary_file_reloads_only_on_change_and_keeps_rules_on_error() {
        let dir = test_dir("dictionary-file");
        let path = dir.join("dictionary.toml");
        let mut file = DictionaryFile::new(path.clone());
        assert_eq!(file.refresh().unwrap(), Refresh::Created);
        assert_eq!(file.refresh().unwrap(), Refresh::Unchanged);
        rewrite(&path, "[ersetzungen]\n\"ag\" = \"AG\"\n");
        assert_eq!(file.refresh().unwrap(), Refresh::Reloaded);
        assert_eq!(file.dictionary().replace("die ag"), "die AG");
        rewrite(&path, "[ersetzung]\n");
        let error = file.refresh().unwrap_err();
        assert!(
            file.failure_note(&error)
                .de()
                .ends_with("Es gilt der letzte fehlerfreie Stand."),
            "{:?}",
            file.failure_note(&error)
        );
        assert_eq!(file.refresh().unwrap(), Refresh::Unchanged, "nur einmal");
        assert_eq!(
            file.dictionary().replace("die ag"),
            "die AG",
            "alte Regeln bleiben"
        );
        remove_test_dir(&dir);
    }

    #[test]
    fn a_faulty_file_after_a_restart_says_the_defaults_apply() {
        let dir = test_dir("dictionary-restart");
        let path = dir.join("dictionary.toml");
        textfile::replace(&path, "[ersetzung]\n").unwrap();
        let mut file = DictionaryFile::new(path);
        let error = file.refresh().unwrap_err();
        let note = file.failure_note(&error);
        remove_test_dir(&dir);
        assert!(
            note.de()
                .ends_with("Seit dem Start gelten die Standardregeln.")
                && note
                    .en()
                    .ends_with("The default rules apply since the start."),
            "{note:?}"
        );
    }

    #[test]
    fn a_failed_read_is_tried_again() {
        let dir = test_dir("dictionary-unreadable");
        // Ein Ordner an der Stelle der Datei lässt sich nicht lesen.
        let path = dir.join("dictionary.toml");
        fs::create_dir_all(&path).unwrap();
        let mut file = DictionaryFile::new(path);
        let first = file.refresh();
        let second = file.refresh();
        remove_test_dir(&dir);
        assert!(
            matches!(first, Err(DictionaryError::Read { path: _, source: _ })),
            "nicht lesbar: {first:?}"
        );
        assert!(
            matches!(second, Err(DictionaryError::Read { path: _, source: _ })),
            "beim nächsten Mal wieder versucht: {second:?}"
        );
    }

    #[test]
    fn rejects_typos() {
        Dictionary::parse("[jura]\nabsatz = \"roemisch\"\n").unwrap_err();
        Dictionary::parse("[ersetzung]\n").unwrap_err();
    }

    /// Je eine Datei jeder früheren Fassung, so wie diese Version sie
    /// geschrieben hat.
    const PAST_VERSIONS: [(&str, &str); 2] = [
        (
            "ebf5a60",
            include_str!("../../fixtures/dictionary/ebf5a60.toml"),
        ),
        (
            "5329d9e",
            include_str!("../../fixtures/dictionary/5329d9e.toml"),
        ),
    ];

    #[test]
    fn reads_every_past_version() {
        let expected = DictionaryEntries {
            replacements: vec![
                ReplacementEntry {
                    from: "bgbat".to_owned(),
                    to: "BGB AT".to_owned(),
                },
                ReplacementEntry {
                    from: "extunknichtigkeit".to_owned(),
                    to: "Ex-tunc-Nichtigkeit".to_owned(),
                },
            ],
            legal: false,
            style: CitationStyle::Abbreviated,
            statutes: vec!["KAGB".to_owned()],
            terms: vec!["Subsumo".to_owned()],
        };
        for (commit, text) in PAST_VERSIONS {
            let read = file_content(text).map(DictionaryEntries::from_content);
            assert!(
                read.is_ok(),
                "Wörterbuch von {commit} bleibt lesbar: {read:?}"
            );
            assert_eq!(read.unwrap(), expected, "Wörterbuch von {commit}");
            assert!(Dictionary::parse(text).is_ok(), "Wörterbuch von {commit}");
        }
        let current = expected.render();
        assert!(
            current.starts_with(&textfile::format_line(FORMAT_VERSION)),
            "die Kennzeile steht vorn"
        );
        assert_eq!(
            DictionaryEntries::from_content(file_content(&current).unwrap()),
            expected
        );
    }

    #[test]
    fn a_dictionary_of_a_newer_version_is_refused() {
        let newer = format!(
            "{}[ersetzungen]\n\"ag\" = \"AG\"\n",
            textfile::format_line(FORMAT_VERSION + 1)
        );
        assert!(
            matches!(
                Dictionary::parse(&newer),
                Err(DictionaryError::Format(FormatError::Unknown {
                    found: 3,
                    current: 2
                }))
            ),
            "ein unbekanntes Format wird abgelehnt"
        );
    }
}
