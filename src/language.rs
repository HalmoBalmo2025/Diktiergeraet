//! Die Sprache einer Aufnahme.
//!
//! Cohere Transcribe erkennt die Sprache nicht selbst; sie wird deshalb vor
//! jeder Aufnahme festgelegt: fn = Deutsch, fn + ⇧ = die zweite Sprache
//! (Standard Englisch, in den Einstellungen wählbar, als
//! [`SecondaryLanguage`] nie Deutsch). Das Modell kennt die 14 Sprachen in
//! [`Language::ALL`] (laut `Capabilities::languages` der Modelldatei).

use std::fmt;

use crate::note::Explain;

use serde::{Deserialize, Serialize};

/// Eine Sprache, die Cohere Transcribe versteht.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum Language {
    /// Deutsch (`de`).
    #[serde(rename = "de")]
    German,
    /// Englisch (`en`).
    #[default]
    #[serde(rename = "en")]
    English,
    /// Französisch (`fr`).
    #[serde(rename = "fr")]
    French,
    /// Spanisch (`es`).
    #[serde(rename = "es")]
    Spanish,
    /// Italienisch (`it`).
    #[serde(rename = "it")]
    Italian,
    /// Portugiesisch (`pt`).
    #[serde(rename = "pt")]
    Portuguese,
    /// Niederländisch (`nl`).
    #[serde(rename = "nl")]
    Dutch,
    /// Polnisch (`pl`).
    #[serde(rename = "pl")]
    Polish,
    /// Griechisch (`el`).
    #[serde(rename = "el")]
    Greek,
    /// Arabisch (`ar`).
    #[serde(rename = "ar")]
    Arabic,
    /// Japanisch (`ja`).
    #[serde(rename = "ja")]
    Japanese,
    /// Chinesisch (`zh`).
    #[serde(rename = "zh")]
    Chinese,
    /// Vietnamesisch (`vi`).
    #[serde(rename = "vi")]
    Vietnamese,
    /// Koreanisch (`ko`).
    #[serde(rename = "ko")]
    Korean,
}

impl Language {
    /// Alle Sprachen, Deutsch und Englisch zuerst, dann alphabetisch nach
    /// deutschem Namen.
    pub(crate) const ALL: [Self; 14] = [
        Self::German,
        Self::English,
        Self::Arabic,
        Self::Chinese,
        Self::French,
        Self::Greek,
        Self::Italian,
        Self::Japanese,
        Self::Korean,
        Self::Dutch,
        Self::Polish,
        Self::Portuguese,
        Self::Spanish,
        Self::Vietnamese,
    ];

    /// ISO-639-1-Code, den das Modell als Sprachhinweis bekommt.
    #[must_use]
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::German => "de",
            Self::English => "en",
            Self::French => "fr",
            Self::Spanish => "es",
            Self::Italian => "it",
            Self::Portuguese => "pt",
            Self::Dutch => "nl",
            Self::Polish => "pl",
            Self::Greek => "el",
            Self::Arabic => "ar",
            Self::Japanese => "ja",
            Self::Chinese => "zh",
            Self::Vietnamese => "vi",
            Self::Korean => "ko",
        }
    }

    /// Kurzes Kürzel für Anzeige und Protokoll („DE“, „ES“).
    #[must_use]
    pub(crate) fn label(self) -> String {
        self.code().to_uppercase()
    }

    /// Deutscher Name („Spanisch“).
    #[must_use]
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::German => "Deutsch",
            Self::English => "Englisch",
            Self::French => "Französisch",
            Self::Spanish => "Spanisch",
            Self::Italian => "Italienisch",
            Self::Portuguese => "Portugiesisch",
            Self::Dutch => "Niederländisch",
            Self::Polish => "Polnisch",
            Self::Greek => "Griechisch",
            Self::Arabic => "Arabisch",
            Self::Japanese => "Japanisch",
            Self::Chinese => "Chinesisch",
            Self::Vietnamese => "Vietnamesisch",
            Self::Korean => "Koreanisch",
        }
    }

    /// Englischer Name (für Anweisungen an Sprachmodelle und die englische
    /// Oberfläche).
    #[must_use]
    pub(crate) const fn english_name(self) -> &'static str {
        match self {
            Self::German => "German",
            Self::English => "English",
            Self::French => "French",
            Self::Spanish => "Spanish",
            Self::Italian => "Italian",
            Self::Portuguese => "Portuguese",
            Self::Dutch => "Dutch",
            Self::Polish => "Polish",
            Self::Greek => "Greek",
            Self::Arabic => "Arabic",
            Self::Japanese => "Japanese",
            Self::Chinese => "Chinese",
            Self::Vietnamese => "Vietnamese",
            Self::Korean => "Korean",
        }
    }

    /// Was zwischen zwei Sätzen oder Teilen steht: nichts bei Sprachen
    /// ohne Leerzeichen, sonst ein Leerzeichen.
    #[must_use]
    pub(crate) const fn gap(self) -> &'static str {
        if self.without_spaces() { "" } else { " " }
    }

    /// Schreibt die Sprache Wörter ohne Leerzeichen dazwischen?
    #[must_use]
    pub(crate) const fn without_spaces(self) -> bool {
        match self {
            Self::Japanese | Self::Chinese => true,
            Self::German
            | Self::English
            | Self::French
            | Self::Spanish
            | Self::Italian
            | Self::Portuguese
            | Self::Dutch
            | Self::Polish
            | Self::Greek
            | Self::Arabic
            | Self::Vietnamese
            | Self::Korean => false,
        }
    }

    /// Die Sprache zu einem Sprachcode wie `es`, `es-MX` oder `pt-BR`.
    #[must_use]
    pub(crate) fn from_code(code: &str) -> Option<Self> {
        // Ohne Region ist der ganze Code die Sprache.
        let primary = code
            .split_once(['-', '_'])
            .map_or(code, |(primary, _region)| primary)
            .to_lowercase();
        Self::ALL
            .into_iter()
            .find(|language| language.code() == primary)
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

/// Die Sprache für fn + ⇧: jede außer Deutsch, denn Deutsch hat fn allein.
/// So kann weder die Datei noch das Panel Deutsch als zweite Sprache
/// wählen (die Auswahl im Panel wäre dann leer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Language", into = "Language")]
pub(crate) struct SecondaryLanguage(Language);

impl SecondaryLanguage {
    /// Englisch, der Standard.
    pub(crate) const ENGLISH: Self = Self(Language::English);

    /// Die Sprache dahinter.
    #[must_use]
    pub(crate) const fn language(self) -> Language {
        self.0
    }

    /// Alle möglichen zweiten Sprachen, in der Reihenfolge von
    /// [`Language::ALL`].
    pub(crate) fn all() -> impl Iterator<Item = Self> {
        // `.ok()` lässt genau Deutsch weg; das ist hier gewollt.
        Language::ALL
            .into_iter()
            .filter_map(|language| Self::try_from(language).ok())
    }
}

impl TryFrom<Language> for SecondaryLanguage {
    type Error = NotSecondaryError;

    /// Jede Sprache außer Deutsch.
    fn try_from(language: Language) -> Result<Self, Self::Error> {
        match language {
            Language::German => Err(NotSecondaryError),
            Language::English
            | Language::French
            | Language::Spanish
            | Language::Italian
            | Language::Portuguese
            | Language::Dutch
            | Language::Polish
            | Language::Greek
            | Language::Arabic
            | Language::Japanese
            | Language::Chinese
            | Language::Vietnamese
            | Language::Korean => Ok(Self(language)),
        }
    }
}

impl From<SecondaryLanguage> for Language {
    fn from(secondary: SecondaryLanguage) -> Self {
        secondary.0
    }
}

/// Deutsch als zweite Sprache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("„de“ geht nicht als zweite Sprache: Deutsch hat schon fn allein")]
pub(crate) struct NotSecondaryError;

impl Explain for NotSecondaryError {
    fn english(&self) -> String {
        "“de” cannot be the second language: fn alone is already German".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_codes_with_regions() {
        assert_eq!(Language::from_code("de"), Some(Language::German));
        assert_eq!(Language::from_code("EN"), Some(Language::English));
        assert_eq!(Language::from_code("pt-BR"), Some(Language::Portuguese));
        assert_eq!(Language::from_code("zh_Hans"), Some(Language::Chinese));
        assert_eq!(Language::from_code("tr"), None);
    }

    /// Platz jeder Sprache in einer Tabelle; der Compiler verlangt jede.
    const fn index(language: Language) -> usize {
        match language {
            Language::German => 0,
            Language::English => 1,
            Language::French => 2,
            Language::Spanish => 3,
            Language::Italian => 4,
            Language::Portuguese => 5,
            Language::Dutch => 6,
            Language::Polish => 7,
            Language::Greek => 8,
            Language::Arabic => 9,
            Language::Japanese => 10,
            Language::Chinese => 11,
            Language::Vietnamese => 12,
            Language::Korean => 13,
        }
    }

    #[test]
    fn all_lists_every_variant() {
        let mut seen = [false; Language::ALL.len()];
        for language in Language::ALL {
            seen[index(language)] = true;
        }
        assert!(
            seen.iter().all(|seen| *seen),
            "ALL fehlt eine Sprache: {seen:?}"
        );
    }

    #[test]
    fn the_secondary_language_is_never_german() {
        let all: Vec<Language> = SecondaryLanguage::all().map(Language::from).collect();
        let expected: Vec<Language> = Language::ALL
            .into_iter()
            .filter(|language| *language != Language::German)
            .collect();
        assert_eq!(all, expected);
        assert_eq!(
            SecondaryLanguage::try_from(Language::German),
            Err(NotSecondaryError)
        );
        assert!(
            serde_json::from_str::<SecondaryLanguage>("\"de\"").is_err(),
            "Deutsch kommt auch über JSON nicht als zweite Sprache hinein"
        );
        assert_eq!(
            serde_json::from_str::<SecondaryLanguage>("\"ko\"").unwrap(),
            SecondaryLanguage::try_from(Language::Korean).unwrap()
        );
    }

    #[test]
    fn codes_are_unique_and_round_trip() {
        for language in Language::ALL {
            assert_eq!(Language::from_code(language.code()), Some(language));
            let json = serde_json::to_string(&language).unwrap();
            assert_eq!(json, format!("\"{}\"", language.code()));
        }
    }
}
