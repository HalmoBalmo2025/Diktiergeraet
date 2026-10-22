//! Zahlen in Normzitaten: Ziffern, deutsche Zahlwörter, Ordnungszahlen und
//! römische Ziffern.

/// Einer als Wort; „ein“ steht in Zusammensetzungen („einundzwanzig“).
const UNITS: [(&str, u32); 10] = [
    ("eins", 1),
    ("ein", 1),
    ("zwei", 2),
    ("drei", 3),
    ("vier", 4),
    ("fünf", 5),
    ("sechs", 6),
    ("sieben", 7),
    ("acht", 8),
    ("neun", 9),
];

/// Zehn bis neunzehn.
const TEENS: [(&str, u32); 10] = [
    ("zehn", 10),
    ("elf", 11),
    ("zwölf", 12),
    ("dreizehn", 13),
    ("vierzehn", 14),
    ("fünfzehn", 15),
    ("sechzehn", 16),
    ("siebzehn", 17),
    ("achtzehn", 18),
    ("neunzehn", 19),
];

/// Zehner ab zwanzig.
const TENS: [(&str, u32); 8] = [
    ("zwanzig", 20),
    ("dreißig", 30),
    ("vierzig", 40),
    ("fünfzig", 50),
    ("sechzig", 60),
    ("siebzig", 70),
    ("achtzig", 80),
    ("neunzig", 90),
];

/// Ordnungszahlen ohne Endung („erst-e“, „zweit-en“ …).
const ORDINAL_STEMS: [(&str, u32); 11] = [
    ("erst", 1),
    ("zweit", 2),
    ("dritt", 3),
    ("viert", 4),
    ("fünft", 5),
    ("sechst", 6),
    ("siebent", 7),
    ("siebt", 7),
    ("acht", 8),
    ("neunt", 9),
    ("zehnt", 10),
];

/// Endungen einer Ordnungszahl.
const ORDINAL_ENDINGS: [&str; 5] = ["e", "er", "en", "es", "em"];

/// Römische Ziffern, absteigend; reicht bis 39 (XXXIX).
const ROMAN: [(u32, &str); 5] = [(10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")];

/// Größte Zahl, die römisch geschrieben wird. Absätze darüber gibt es kaum.
const ROMAN_MAX: u32 = 39;

/// Ziffern („823“) als Zahl.
pub(crate) fn digits(word: &str) -> Option<u32> {
    if word.is_empty() || !word.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    word.parse().ok()
}

/// Ziffern oder ein deutsches Zahlwort bis 9999 („fünfzehn“,
/// „dreihundertzwölf“) als Zahl.
pub(crate) fn cardinal(word: &str) -> Option<u32> {
    digits(word).or_else(|| below_10000(&word.to_lowercase()))
}

/// Ordnungszahl als Wort („erste“, „Zweiten“) als Zahl.
pub(crate) fn ordinal_word(word: &str) -> Option<u32> {
    let lower = word.to_lowercase();
    ORDINAL_STEMS.iter().find_map(|(stem, value)| {
        let ending = lower.strip_prefix(stem)?;
        ORDINAL_ENDINGS.contains(&ending).then_some(*value)
    })
}

/// Römische Schreibweise (1 → „I“, 14 → „XIV“); `None` außerhalb von 1–39.
pub(crate) fn roman(value: u32) -> Option<String> {
    if value == 0 || value > ROMAN_MAX {
        return None;
    }
    let mut rest = value;
    let mut result = String::new();
    for (step, letters) in ROMAN {
        while let Some(smaller) = rest.checked_sub(step) {
            result.push_str(letters);
            rest = smaller;
        }
    }
    Some(result)
}

/// Eine römische Zahl in der Schreibweise von [`roman`] („XIV“) als Zahl.
pub(crate) fn from_roman(word: &str) -> Option<u32> {
    (1..=ROMAN_MAX).find(|value| roman(*value).is_some_and(|written| written == word))
}

/// Zahlwort unter 10 000: vor „tausend“ steht höchstens „neun“
/// („zehntausend“ ist schon zu groß).
fn below_10000(word: &str) -> Option<u32> {
    match word.split_once("tausend") {
        Some((thousands, rest)) => {
            let thousands = multiplier(thousands).filter(|count| *count < 10)?;
            let rest = remainder(rest, below_1000)?;
            thousands.checked_mul(1000)?.checked_add(rest)
        }
        None => below_1000(word),
    }
}

/// Zahlwort unter 1000.
fn below_1000(word: &str) -> Option<u32> {
    match word.split_once("hundert") {
        Some((hundreds, rest)) => {
            let hundreds = multiplier(hundreds)?;
            let rest = remainder(rest, below_100)?;
            hundreds.checked_mul(100)?.checked_add(rest)
        }
        None => below_100(word),
    }
}

/// Zahl vor „hundert“/„tausend“: leer oder „ein“ heißt eins.
fn multiplier(word: &str) -> Option<u32> {
    match word {
        "" | "ein" => Some(1),
        other => below_100(other),
    }
}

/// Rest nach „hundert“/„tausend“: leer heißt null, ein „und“ davor zählt
/// nicht („hundertundeins“).
fn remainder(word: &str, parse: fn(&str) -> Option<u32>) -> Option<u32> {
    let word = word.strip_prefix("und").unwrap_or(word);
    match word {
        "" => Some(0),
        other => parse(other),
    }
}

/// Zahlwort unter 100.
fn below_100(word: &str) -> Option<u32> {
    lookup(&TEENS, word)
        .or_else(|| lookup(&TENS, word))
        .or_else(|| lookup(&UNITS, word).filter(|_| word != "ein"))
        .or_else(|| {
            let (unit, tens) = word.split_once("und")?;
            lookup(&UNITS, unit)?.checked_add(lookup(&TENS, tens)?)
        })
}

/// Sucht ein Wort in einer Tabelle.
fn lookup(table: &[(&str, u32)], word: &str) -> Option<u32> {
    table
        .iter()
        .find(|(name, _)| *name == word)
        .map(|(_, value)| *value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_digits_and_words() {
        assert_eq!(cardinal("823"), Some(823));
        assert_eq!(cardinal("eins"), Some(1));
        assert_eq!(cardinal("Fünfzehn"), Some(15));
        assert_eq!(cardinal("einundzwanzig"), Some(21));
        assert_eq!(cardinal("hundert"), Some(100));
        assert_eq!(cardinal("dreihundertzwölf"), Some(312));
        assert_eq!(cardinal("hundertundeins"), Some(101));
        assert_eq!(cardinal("tausendvier"), Some(1004));
        assert_eq!(cardinal("zweitausenddreihundertneunundachtzig"), Some(2389));
    }

    #[test]
    fn reads_ein_before_hundred_and_thousand() {
        assert_eq!(cardinal("einhundertdreiundzwanzig"), Some(123));
        assert_eq!(cardinal("eintausendzweihundert"), Some(1200));
        assert_eq!(cardinal("hundertdreiundzwanzig"), Some(123));
    }

    #[test]
    fn rejects_other_words() {
        assert_eq!(cardinal("ein"), None);
        assert_eq!(cardinal("und"), None);
        assert_eq!(cardinal("Absatz"), None);
        assert_eq!(cardinal("823a"), None);
        assert_eq!(cardinal(""), None);
        assert_eq!(digits("50.000"), None);
    }

    #[test]
    fn reads_ordinals() {
        assert_eq!(ordinal_word("erste"), Some(1));
        assert_eq!(ordinal_word("Zweiten"), Some(2));
        assert_eq!(ordinal_word("dritter"), Some(3));
        assert_eq!(ordinal_word("siebte"), Some(7));
        assert_eq!(ordinal_word("erst"), None);
        assert_eq!(ordinal_word("Achtung"), None);
    }

    #[test]
    fn writes_roman_numerals() {
        assert_eq!(roman(1).as_deref(), Some("I"));
        assert_eq!(roman(4).as_deref(), Some("IV"));
        assert_eq!(roman(9).as_deref(), Some("IX"));
        assert_eq!(roman(14).as_deref(), Some("XIV"));
        assert_eq!(roman(39).as_deref(), Some("XXXIX"));
        assert_eq!(roman(0), None);
        assert_eq!(roman(40), None);
    }

    #[test]
    fn reads_roman_numerals_back() {
        for value in 1..=ROMAN_MAX {
            assert_eq!(roman(value).as_deref().and_then(from_roman), Some(value));
        }
        for word in ["", "IIII", "iv", "XL", "VX", "Abs"] {
            assert_eq!(from_roman(word), None, "{word}");
        }
    }

    /// Vergleich: ein deutsches Zahlwort für `value` (1–9999), bewusst
    /// einfach gebaut.
    fn spell(value: u32) -> String {
        const ONES: [&str; 10] = [
            "", "ein", "zwei", "drei", "vier", "fünf", "sechs", "sieben", "acht", "neun",
        ];
        const TEENS_WORDS: [&str; 10] = [
            "zehn",
            "elf",
            "zwölf",
            "dreizehn",
            "vierzehn",
            "fünfzehn",
            "sechzehn",
            "siebzehn",
            "achtzehn",
            "neunzehn",
        ];
        const TENS_WORDS: [&str; 10] = [
            "", "", "zwanzig", "dreißig", "vierzig", "fünfzig", "sechzig", "siebzig", "achtzig",
            "neunzig",
        ];
        let digit = |n: u32| usize::try_from(n).unwrap();
        let under_100 = |n: u32| -> String {
            match (digit(n / 10), digit(n % 10)) {
                (0, 0) => String::new(),
                (0, 1) => "eins".to_owned(),
                (0, ones) => ONES[ones].to_owned(),
                (1, ones) => TEENS_WORDS[ones].to_owned(),
                (tens, 0) => TENS_WORDS[tens].to_owned(),
                (tens, ones) => format!("{}und{}", ONES[ones], TENS_WORDS[tens]),
            }
        };
        let (thousands, hundreds, rest) = (value / 1000, value / 100 % 10, value % 100);
        let mut word = String::new();
        if thousands > 0 {
            word.push_str(ONES[digit(thousands)]);
            word.push_str("tausend");
        }
        if hundreds > 0 {
            word.push_str(ONES[digit(hundreds)]);
            word.push_str("hundert");
        }
        word.push_str(&under_100(rest));
        word
    }

    #[test]
    fn reads_every_spelled_number_below_10000() {
        let wrong: Vec<(u32, String, Option<u32>)> = (1..10_000)
            .map(|value| (value, spell(value)))
            .map(|(value, word)| {
                let read = cardinal(&word);
                (value, word, read)
            })
            .filter(|(value, _, read)| *read != Some(*value))
            .collect();
        assert_eq!(wrong, []);
    }

    #[test]
    fn stops_at_9999() {
        assert_eq!(cardinal("neuntausendneunhundertneunundneunzig"), Some(9999));
        assert_eq!(cardinal("zehntausend"), None);
        assert_eq!(cardinal("neunundneunzigtausend"), None);
        assert_eq!(cardinal("hunderttausend"), None);
    }
}
