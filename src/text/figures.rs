//! Zahlen als Ziffern (nur Deutsch).
//!
//! Cohere schreibt Zahlen mal als Ziffern, mal als Wörter – je nachdem,
//! was das Modell aus dem Zusammenhang für üblicher hält („2,5 GB“, aber
//! „zwei Komma fünf Gigabyte“). Diese Regel macht daraus einheitlich Ziffern,
//! so wie man es schreiben würde:
//!
//! - Kommazahlen: „zwei Komma fünf“ → „2,5“; vor „Komma“ muss ein echtes
//!   Zahlwort stehen, „ein Komma“ ist der Artikel („Da fehlt ein Komma.“)
//! - Zahlen mit Maßeinheit: „drei Gigabyte“ → „3 Gigabyte“, „null Grad“
//!   → „0 Grad“
//! - Zahlen ab 13: „dreiundzwanzig“ → „23“, „zweitausendsechsundzwanzig“
//!   → „2026“
//!
//! Kleine Zahlen ohne Einheit bleiben Wörter („drei Tage“, „ein Mann“),
//! ebenso „hundert“ und „tausend“ allein („tausend Dank“) und Zahlwörter ab
//! zehntausend („zwanzigtausend Euro“, siehe [`numbers::cardinal`]).

use super::numbers;
use super::words::{self, Word};

/// Maßeinheiten und Währungen, vor denen eine Zahl als Ziffer steht
/// (klein geschrieben).
const UNITS: [&str; 36] = [
    "prozent",
    "euro",
    "cent",
    "dollar",
    "franken",
    "pfund",
    "byte",
    "kilobyte",
    "megabyte",
    "gigabyte",
    "terabyte",
    "kb",
    "mb",
    "gb",
    "tb",
    "bit",
    "megabit",
    "gigabit",
    "hertz",
    "kilohertz",
    "megahertz",
    "gigahertz",
    "kilometer",
    "meter",
    "zentimeter",
    "millimeter",
    "km",
    "cm",
    "mm",
    "kilogramm",
    "gramm",
    "kg",
    "tonnen",
    "liter",
    "milliliter",
    "grad",
];

/// Zahlen ab hier werden auch ohne Einheit Ziffern.
const FIGURES_FROM: u32 = 13;

/// Allein stehend bleiben sie Wörter („hundert Leute“, „tausend Dank“).
const ROUND_WORDS: [&str; 2] = ["hundert", "tausend"];

/// Schreibt Zahlwörter als Ziffern, wo man Ziffern schreibt (siehe oben).
#[must_use]
pub(crate) fn to_figures(text: &str) -> String {
    let (leading, input) = words::split(text);
    let mut output: Vec<Word> = Vec::with_capacity(input.len());
    let mut rest = input.as_slice();
    while let [first, after_first @ ..] = rest {
        if let Some((decimal, after_decimal)) = decimal(rest) {
            output.push(decimal);
            rest = after_decimal;
        } else {
            output.push(figure(first, after_first.first()));
            rest = after_first;
        }
    }
    words::join(&leading, &output)
}

/// Eine Kommazahl am Anfang von `words` und der Rest: „zwei Komma fünf“ →
/// „2,5“, „null Komma null fünf“ → „0,05“.
fn decimal(words: &[Word]) -> Option<(Word, &[Word])> {
    let [first, comma, fraction, after_fraction @ ..] = words else {
        return None;
    };
    // Satzzeichen zwischen den Wörtern trennen die Zahl („zwei, Komma“,
    // „Komma (drei“).
    if !first.trail.is_empty()
        || !comma.lead.is_empty()
        || !comma.trail.is_empty()
        || !fraction.lead.is_empty()
        || !comma.is_any(&["komma"])
    {
        return None;
    }
    let whole = number(&first.core)?;
    let mut decimals = decimals(&fraction.core)?;
    let mut last = fraction;
    let mut rest = after_fraction;
    // Einzeln gesprochene Stellen: „fünf sieben“ → „57“.
    if digit_word(&fraction.core).is_some() {
        while let [next, after_next @ ..] = rest
            && last.trail.is_empty()
            && next.lead.is_empty()
            && let Some(digit) = digit_word(&next.core)
        {
            decimals.push_str(&digit.to_string());
            last = next;
            rest = after_next;
        }
    }
    let word = Word {
        lead: first.lead.clone(),
        core: format!("{whole},{decimals}"),
        trail: last.trail.clone(),
        space: last.space.clone(),
    };
    Some((word, rest))
}

/// `word` als Ziffer, wenn eine Maßeinheit folgt (`next`) oder die Zahl
/// groß ist; sonst unverändert. „ein“ bleibt immer ein Wort („ein Euro“).
fn figure(word: &Word, next: Option<&Word>) -> Word {
    let unit_follows = word.trail.is_empty() && next.is_some_and(|next| next.is_any(&UNITS));
    let value = number(&word.core)
        .filter(|_| !word.core.chars().all(|c| c.is_ascii_digit()))
        .filter(|value| unit_follows || (*value >= FIGURES_FROM && !word.is_any(&ROUND_WORDS)));
    value.map_or_else(
        || word.clone(),
        |value| Word {
            core: value.to_string(),
            ..word.clone()
        },
    )
}

/// Ein Zahlwort oder Ziffern, auch „null“. „ein“ ist hier der Artikel, keine
/// Zahl („ein Komma zu viel“, „ein Euro“); nur „eins“ zählt.
fn number(word: &str) -> Option<u32> {
    if word.eq_ignore_ascii_case("null") {
        return Some(0);
    }
    numbers::cardinal(word)
}

/// Ein Zahlwort von „null“ bis „neun“ (keine Ziffer).
fn digit_word(word: &str) -> Option<u32> {
    number(word).filter(|value| *value < 10 && !word.chars().any(|c| c.is_ascii_digit()))
}

/// Die Stellen nach „Komma“: Ziffern bleiben, wie sie sind („05“), ein
/// Zahlwort wird zur Zahl („fünfundzwanzig“ → „25“).
fn decimals(word: &str) -> Option<String> {
    if !word.is_empty() && word.chars().all(|c| c.is_ascii_digit()) {
        return Some(word.to_owned());
    }
    number(word).map(|value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_decimals_as_figures() {
        assert_eq!(
            to_figures("Das gibt zwei Komma fünf Gigabyte frei."),
            "Das gibt 2,5 Gigabyte frei."
        );
        assert_eq!(to_figures("null Komma fünf Liter"), "0,5 Liter");
        assert_eq!(to_figures("eins Komma fünfundzwanzig"), "1,25");
        assert_eq!(to_figures("6 Komma 7 GB"), "6,7 GB");
        assert_eq!(to_figures("zwei Komma fünf sieben Prozent"), "2,57 Prozent");
        assert_eq!(to_figures("null Komma null fünf Prozent"), "0,05 Prozent");
        assert_eq!(
            to_figures("zwei Komma fünf, sieben Leute"),
            "2,5, sieben Leute"
        );
        assert_eq!(
            to_figures("zwei Komma drei (eins bis sechs)"),
            "2,3 (eins bis sechs)"
        );
    }

    #[test]
    fn writes_numbers_with_units_as_figures() {
        assert_eq!(to_figures("zehn Prozent mehr"), "10 Prozent mehr");
        assert_eq!(to_figures("drei Gigabyte"), "3 Gigabyte");
        assert_eq!(to_figures("Es kostet fünf Euro."), "Es kostet 5 Euro.");
        assert_eq!(to_figures("Es hat zehn Grad."), "Es hat 10 Grad.");
        assert_eq!(to_figures("Es hat null Grad."), "Es hat 0 Grad.");
        assert_eq!(to_figures("null Prozent"), "0 Prozent");
    }

    #[test]
    fn writes_large_numbers_as_figures() {
        assert_eq!(
            to_figures("Im Jahr zweitausendsechsundzwanzig kamen dreiundzwanzig Leute."),
            "Im Jahr 2026 kamen 23 Leute."
        );
        assert_eq!(to_figures("Dreizehn waren da."), "13 waren da.");
    }

    #[test]
    fn keeps_small_numbers_and_words() {
        for text in [
            "Ich bleibe drei Tage.",
            "Ein Mann und eine Frau.",
            "Tausend Dank, hundert Leute.",
            "Das ist ein Komma zu viel.",
            "Da fehlt ein Komma vier Wörter weiter.",
            "Es kostet ein Euro.",
            "Die Null steht.",
            "Er ist elf, sie zwölf.",
            "Zwei, Komma, fünf",
            "Es sind 25 Grad.",
            "Es kostet zwanzigtausend Euro.",
            "zwei Komma (drei Leute)",
        ] {
            assert_eq!(to_figures(text), text);
        }
    }
}
