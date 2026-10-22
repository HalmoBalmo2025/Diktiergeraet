//! Rechtschreibprüfung von macOS – über das Swift-Hilfsprogramm
//! `diktator-spell` (Quelltext in `helpers/spell.swift`).
//!
//! Das Sicherheitsnetz fürs Überarbeiten fragt damit, welche Wörter des
//! Diktats schon richtig geschrieben sind; die darf das Sprachmodell nicht
//! mehr verändern („nach Hause“ bleibt, auch wenn es „nach Haus“ vorschlägt).

use std::collections::BTreeSet;
use std::time::Duration;

use super::guard;
use crate::helper::{self, Helper, HelperError};
use crate::language::Language;

/// Länger wird nicht gewartet; dann gilt kein Wort als geprüft.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Warum nicht geprüft werden konnte.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SpellingError {
    /// Das Hilfsprogramm fehlt, antwortet nicht oder kennt die Sprache
    /// nicht (z. B. Chinesisch, Japanisch).
    #[error(transparent)]
    Helper(#[from] HelperError),
    /// Der Thread der Prüfung ist abgestürzt.
    #[error("Rechtschreibprüfung abgestürzt")]
    Crashed,
}

/// Die Wörter aus `text`, die macOS in `language` für richtig geschrieben
/// hält – als Vergleichskern ([`guard::core`]).
///
/// # Errors
/// [`SpellingError`], wenn das Hilfsprogramm fehlt, die Sprache nicht
/// kennt oder nicht antwortet.
pub(crate) fn correct_words(
    text: &str,
    language: Language,
) -> Result<BTreeSet<String>, SpellingError> {
    let mut words: Vec<&str> = text
        .split_whitespace()
        .map(|raw| raw.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|word| word.chars().any(char::is_alphabetic))
        .collect();
    words.sort_unstable();
    words.dedup();
    if words.is_empty() {
        return Ok(BTreeSet::new());
    }
    let answers = helper::run(
        Helper::Spell,
        &[language.code()],
        &words.join("\n"),
        TIMEOUT,
    )?;
    Ok(words
        .into_iter()
        .zip(answers.lines())
        .filter(|(_, answer)| *answer == "1")
        .map(|(word, _)| guard::core(word))
        .collect())
}
