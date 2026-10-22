//! Fachbegriffe: Cohere kann man keine Wörter vorgeben, also werden
//! erkannte Wörter, die einem Begriff aus dem Wörterbuch sehr ähnlich sind,
//! danach zu diesem Begriff („Subsomo“ → „Subsumo“).
//!
//! Damit nichts Richtiges verbogen wird, gilt das nur für längere Wörter
//! und kleine Abweichungen; gebeugte Formen („Subsumos“) bleiben stehen.
//! Begriffe sind einzelne Wörter – Wortgruppen gehören in die Ersetzungen.

use super::words;

/// Kürzere Begriffe und Wörter bleiben außen vor (zu viele Verwechslungen).
pub(crate) const MIN_CHARS: usize = 5;

/// Ab so vielen Zeichen dürfen zwei statt einem abweichen.
pub(crate) const LONG_CHARS: usize = 8;

/// Die Fachbegriffe aus dem Wörterbuch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Terms {
    /// Die Begriffe, wie sie geschrieben werden sollen.
    terms: Vec<String>,
}

impl Terms {
    /// Die Begriffe aus `list`; leere und solche mit Leerzeichen fallen weg.
    pub(crate) fn new(list: &[String]) -> Self {
        Self {
            terms: list
                .iter()
                .map(|term| term.trim())
                .filter(|term| !term.is_empty() && !term.contains(char::is_whitespace))
                .map(str::to_owned)
                .collect(),
        }
    }

    /// Ersetzt Wörter, die einem Begriff sehr ähnlich sind, durch ihn.
    pub(crate) fn apply(&self, text: &str) -> String {
        if self.terms.is_empty() {
            return text.to_owned();
        }
        let (leading, mut words) = words::split(text);
        for word in &mut words {
            if let Some(term) = self.closest(&word.core) {
                word.core.clone_from(term);
            }
        }
        words::join(&leading, &words)
    }

    /// Der Begriff, dem `word` am nächsten ist, wenn er nah genug ist und
    /// anders geschrieben wird. Ist `word` selbst ein Begriff (genau so
    /// geschrieben), bleibt es: „Schmitt“ wird nicht zum ähnlichen
    /// „Schmidt“.
    fn closest(&self, word: &str) -> Option<&String> {
        if word.chars().count() < MIN_CHARS || self.terms.iter().any(|term| term == word) {
            return None;
        }
        let lower = word.to_lowercase();
        self.terms
            .iter()
            .filter_map(|term| {
                let term_lower = term.to_lowercase();
                if inflected(&lower, &term_lower) {
                    return None;
                }
                let distance = strsim::levenshtein(&lower, &term_lower);
                let shorter = word.chars().count().min(term.chars().count());
                let allowed = if shorter >= LONG_CHARS { 2 } else { 1 };
                (shorter >= MIN_CHARS && distance <= allowed).then_some((distance, term))
            })
            .min_by_key(|(distance, _)| *distance)
            .map(|(_, term)| term)
    }
}

/// Ist das eine Wort eine gebeugte Form des anderen? Beginnt eins mit dem
/// anderen, auch über Umlaute hinweg („subsumos“ – „subsumo“,
/// „tatbestände“ – „tatbestand“, „vermieter“ – „vermieterin“), bleibt das
/// Wort stehen. Beide sind kleingeschrieben.
fn inflected(word: &str, term: &str) -> bool {
    let plain = |text: &str| -> String {
        text.chars()
            .map(|c| match c {
                'ä' => 'a',
                'ö' => 'o',
                'ü' => 'u',
                other => other,
            })
            .collect()
    };
    let (word, term) = (plain(word), plain(term));
    word != term && (word.starts_with(&term) || term.starts_with(&word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(list: &[&str]) -> Terms {
        Terms::new(
            &list
                .iter()
                .map(|term| (*term).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn corrects_close_misspellings() {
        let terms = terms(&["Subsumo", "Diktator", "Gutachtenstil"]);
        assert_eq!(
            terms.apply("Ich lerne mit Subsomo und dem Diktatur-freien Gutachtensteil."),
            "Ich lerne mit Subsumo und dem Diktatur-freien Gutachtenstil."
        );
        assert_eq!(terms.apply("subsumo, bitte."), "Subsumo, bitte.");
    }

    #[test]
    fn leaves_other_words_alone() {
        let terms = terms(&["Subsumo", "Kolibri"]);
        // Gebeugt, zu weit weg oder zu kurz.
        assert_eq!(terms.apply("Des Subsumos Karten"), "Des Subsumos Karten");
        assert_eq!(
            terms.apply("Die Subsumtion folgt."),
            "Die Subsumtion folgt."
        );
        assert_eq!(terms.apply("Kolibris fliegen."), "Kolibris fliegen.");
        let short = self::terms(&["Abc"]);
        assert_eq!(short.apply("Abd"), "Abd");
    }

    #[test]
    fn leaves_inflected_forms_alone() {
        let terms = terms(&["Tatbestand", "Vermieterin"]);
        assert_eq!(
            terms.apply("Die Tatbestände sind erfüllt."),
            "Die Tatbestände sind erfüllt."
        );
        assert_eq!(
            terms.apply("Der Vermieter kündigt."),
            "Der Vermieter kündigt."
        );
        assert_eq!(
            terms.apply("Der Tatbestnad ist erfüllt."),
            "Der Tatbestand ist erfüllt."
        );
    }

    #[test]
    fn ignores_empty_and_multiword_terms() {
        assert_eq!(terms(&["", "  ", "ex tunc"]), Terms::default());
    }

    #[test]
    fn an_exact_term_is_not_replaced_by_a_similar_one() {
        let terms = terms(&["Schmidt", "Schmitt"]);
        assert_eq!(terms.apply("Herr Schmitt kommt."), "Herr Schmitt kommt.");
        assert_eq!(terms.apply("Herr Schmidt kommt."), "Herr Schmidt kommt.");
        assert_eq!(terms.apply("Herr schmitt kommt."), "Herr Schmitt kommt.");
    }
}
