//! Text in Wörter zerlegen und wieder zusammensetzen, ohne Satzzeichen oder
//! Leerraum zu verlieren.

/// Unten stehende Anführungszeichen („…“, ‚…‘): Sie öffnen immer.
const LOW_QUOTES: [char; 2] = ['„', '‚'];

/// Alle Anführungszeichen. Bis auf die unten stehenden öffnen oder
/// schließen sie je nach Sprache („…“ schließt mit “, das im Englischen
/// öffnet; »…« und «…» je nach Land); was gemeint ist, entscheidet die
/// Stellung ([`quote_opens`]).
const QUOTES: [char; 10] = ['„', '‚', '“', '”', '‘', '’', '»', '«', '"', '\''];

/// Öffnende Klammern und der Schrägstrich: Danach folgt kein Leerzeichen.
const OPENING_BRACKETS: [char; 4] = ['(', '[', '{', '/'];

/// Satzzeichen nach einem Wort, ohne Anführungszeichen.
const TRAILING_MARKS: [char; 9] = ['.', ',', ';', ':', '!', '?', ')', ']', '}'];

/// Abkürzungen (klein, ohne Punkt), deren Punkt kein Satzende ist („Birnen
/// usw. und Kiwis“), dazu die Normteile der Jura-Regeln („Abs.“, „S.“).
const ABBREVIATIONS: &[&str] = &[
    "abs", "alt", "art", "bspw", "bzw", "ca", "dr", "etc", "evtl", "ff", "ggf", "hs", "inkl",
    "lit", "nr", "prof", "s", "uabs", "usw", "var", "vgl", "vs",
];

/// Ist `word` (ohne Punkt) eine Abkürzung, deren Punkt kein Satzende ist?
/// Einzelne Buchstaben zählen („z. B.“, „d. h.“), Ziffern nicht („um 8.“),
/// und auch keine römische Ziffer hinter einer Nummer („§ 823 I.“);
/// `before` ist das Wort davor.
pub(crate) fn is_abbreviation(word: &str, before: Option<&str>) -> bool {
    let word = word.trim_start_matches(is_leading);
    let mut chars = word.chars();
    let letter = chars.next().is_some_and(char::is_alphabetic) && chars.next().is_none();
    let roman_after_number = matches!(word, "I" | "V" | "X")
        && before.is_some_and(|before| before.starts_with(|c: char| c.is_ascii_digit()));
    (letter && !roman_after_number) || ABBREVIATIONS.contains(&word.to_lowercase().as_str())
}

/// Endet `text` mit dem Punkt einer Abkürzung („usw.“, „z. B.“, „z.B.“)?
pub(crate) fn ends_with_abbreviation(text: &str) -> bool {
    let Some(rest) = text.strip_suffix('.') else {
        return false;
    };
    let mut words = rest.split_whitespace().rev();
    let Some(last) = words.next() else {
        return false;
    };
    // „z.B“: Nach einem Punkt im Wort zählt nur der letzte Teil.
    let word = last.rsplit_once('.').map_or(last, |(_, tail)| tail);
    is_abbreviation(word, words.next())
}

/// Ist `c` ein Anführungszeichen?
pub(crate) fn is_quote(c: char) -> bool {
    QUOTES.contains(&c)
}

/// Öffnende Klammer oder Schrägstrich?
pub(crate) fn is_opening_bracket(c: char) -> bool {
    OPENING_BRACKETS.contains(&c)
}

/// Öffnet das Anführungszeichen `quote`, wenn direkt davor `before` steht
/// (`None`: Textanfang)? Ein unten stehendes öffnet immer; jedes andere
/// öffnet am Anfang, nach Leerraum, nach einer öffnenden Klammer oder einem
/// unten stehenden Zeichen und schließt direkt nach einem Buchstaben, einer
/// Ziffer oder einem Satzzeichen („Hallo“ schließt, “Hello öffnet).
pub(crate) fn quote_opens(quote: char, before: Option<char>) -> bool {
    LOW_QUOTES.contains(&quote)
        || before
            .is_none_or(|c| c.is_whitespace() || is_opening_bracket(c) || LOW_QUOTES.contains(&c))
}

/// Kann vor einem Wort stehen: öffnende Klammern und Anführungszeichen.
fn is_leading(c: char) -> bool {
    matches!(c, '(' | '[' | '{') || is_quote(c)
}

/// Kann nach einem Wort stehen: Satzzeichen, schließende Klammern und
/// Anführungszeichen.
fn is_trailing(c: char) -> bool {
    TRAILING_MARKS.contains(&c) || is_quote(c)
}

/// Ein Wort mit den Satzzeichen davor und danach und dem Leerraum dahinter.
///
/// „(Abs.“ wird zu `lead` „(“, `core` „Abs“, `trail` „.“.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Word {
    /// Satzzeichen vor dem Wort.
    pub(crate) lead: String,
    /// Das Wort selbst.
    pub(crate) core: String,
    /// Satzzeichen nach dem Wort.
    pub(crate) trail: String,
    /// Leerraum nach dem Wort.
    pub(crate) space: String,
}

impl Word {
    /// Neues Wort ohne Satzzeichen davor.
    pub(crate) fn new(core: &str, trail: &str, space: &str) -> Self {
        Self {
            lead: String::new(),
            core: core.to_owned(),
            trail: trail.to_owned(),
            space: space.to_owned(),
        }
    }

    /// Ist das Wort (ohne Satzzeichen) eines von `options` (kleingeschrieben
    /// angegeben), unabhängig von Groß- und Kleinschreibung?
    pub(crate) fn is_any(&self, options: &[&str]) -> bool {
        options.contains(&self.core.to_lowercase().as_str())
    }

    /// Endet das Wort mit einem Punkt (Abkürzung oder Satzende)?
    pub(crate) fn has_dot(&self) -> bool {
        self.trail.starts_with('.')
    }

    /// Beginnt das Wort mit einem Großbuchstaben?
    pub(crate) fn starts_uppercase(&self) -> bool {
        self.core.chars().next().is_some_and(char::is_uppercase)
    }
}

/// Teilt `text` vor dem ersten Zeichen, auf das `stop` passt; passt keins,
/// ist alles der erste Teil.
fn split_before(text: &str, stop: impl Fn(char) -> bool) -> (&str, &str) {
    let end = text
        .char_indices()
        .find(|(_, c)| stop(*c))
        .map_or(text.len(), |(index, _)| index);
    text.split_at(end)
}

/// Teilt `text` in den Leerraum am Anfang und den Rest.
pub(crate) fn split_leading_whitespace(text: &str) -> (&str, &str) {
    split_before(text, |c| !c.is_whitespace())
}

/// Zerlegt Text in Wörter. Liefert den Leerraum vor dem ersten Wort und die
/// Wörter.
pub(crate) fn split(text: &str) -> (String, Vec<Word>) {
    let (leading, body) = split_leading_whitespace(text);
    let mut words = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        let (raw, after) = split_before(rest, char::is_whitespace);
        let (space, next) = split_leading_whitespace(after);
        words.push(parse(raw, space));
        rest = next;
    }
    (leading.to_owned(), words)
}

/// Setzt Wörter wieder zu Text zusammen.
pub(crate) fn join(leading: &str, words: &[Word]) -> String {
    let mut text = leading.to_owned();
    for word in words {
        text.push_str(&word.lead);
        text.push_str(&word.core);
        text.push_str(&word.trail);
        text.push_str(&word.space);
    }
    text
}

/// Trennt Satzzeichen vom Wort.
fn parse(raw: &str, space: &str) -> Word {
    let (lead, without_lead) = split_before(raw, |c| !is_leading(c));
    // `trim_end_matches` liefert einen Anfang von `without_lead`, also
    // trennt dessen Länge an einer Zeichengrenze.
    let (core, trail) = without_lead.split_at(without_lead.trim_end_matches(is_trailing).len());
    Word {
        lead: lead.to_owned(),
        core: core.to_owned(),
        trail: trail.to_owned(),
        space: space.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_text_intact() {
        for text in [
            "",
            "  Hallo Welt.  ",
            "(§ 823 Abs. 1 BGB), siehe „oben“!\n\nNeuer Absatz …",
            "i.V.m. St. G. B.,",
            "»Zitat« und “quote” ‘x’",
        ] {
            let (leading, words) = split(text);
            assert_eq!(join(&leading, &words), text);
        }
    }

    #[test]
    fn separates_punctuation() {
        let (_, words) = split("(Abs. 1, BGB).");
        assert_eq!(words[0].lead, "(");
        assert_eq!(words[0].core, "Abs");
        assert_eq!(words[0].trail, ".");
        assert_eq!(words[1].core, "1");
        assert_eq!(words[1].trail, ",");
        assert_eq!(words[2].core, "BGB");
        assert_eq!(words[2].trail, ").");
        assert_eq!(words[2].space, "");
    }

    #[test]
    fn tells_opening_from_closing_quotes() {
        assert!(quote_opens('„', Some('r')), "„ öffnet immer");
        assert!(!quote_opens('“', Some('o')), "“ nach „Hallo“ schließt");
        assert!(quote_opens('“', Some(' ')), "“ nach Leerraum öffnet");
        assert!(quote_opens('"', None), "am Anfang öffnet jedes");
        assert!(
            !quote_opens('"', Some('.')),
            "nach einem Satzzeichen schließt es"
        );
        assert!(quote_opens('»', Some('(')), "nach einer Klammer öffnet es");
    }

    #[test]
    fn knows_abbreviation_periods() {
        for text in [
            "Birnen usw.",
            "zum Beispiel z. B.",
            "z.B.",
            "Dr.",
            "(Abs.",
            "d. h.",
        ] {
            assert!(ends_with_abbreviation(text), "{text}");
        }
        for text in [
            "um 8.",
            "um 10.",
            "aus § 823 I.",
            "Hause.",
            "Na ja...",
            "usw",
        ] {
            assert!(!ends_with_abbreviation(text), "{text}");
        }
    }

    #[test]
    fn compares_without_case() {
        let (_, words) = split("Absatz ÄNDERUNG");
        assert!(
            words[0].is_any(&["absatz"]),
            "„Absatz“ passt trotz Großschreibung"
        );
        assert!(
            words[1].is_any(&["änderung"]),
            "„ÄNDERUNG“ passt samt Umlaut"
        );
        assert!(
            !words[0].is_any(&["satz"]),
            "nur ganze Wörter passen, keine Teile"
        );
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        /// Zeichen, aus denen die Texte bestehen: Leerraum, Buchstaben,
        /// Satzzeichen und Anführungszeichen.
        const CHARS: &[char] = &[
            ' ', '\n', '\t', '\u{a0}', 'a', 'Ä', '.', ',', '(', ')', '"', '„', '“', '’', '»', '«',
            '!', '§', '1',
        ];

        proptest! {
            /// Zerlegen und Zusammensetzen ergibt denselben Text.
            #[test]
            fn split_and_join_round_trip(chars in prop::collection::vec(prop::sample::select(CHARS), 0..24)) {
                let text: String = chars.into_iter().collect();
                let (leading, words) = split(&text);
                prop_assert_eq!(join(&leading, &words), text);
            }

            /// Kein Wort ist leer, und Leerraum steht nur zwischen Wörtern.
            #[test]
            fn words_hold_no_whitespace(chars in prop::collection::vec(prop::sample::select(CHARS), 0..24)) {
                let text: String = chars.into_iter().collect();
                let (_, words) = split(&text);
                for word in words {
                    let written = format!("{}{}{}", word.lead, word.core, word.trail);
                    prop_assert!(!written.is_empty() && !written.contains(char::is_whitespace));
                    prop_assert!(word.space.chars().all(char::is_whitespace));
                }
            }
        }
    }
}
