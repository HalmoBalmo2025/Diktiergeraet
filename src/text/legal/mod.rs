//! Normzitate in Klausurschreibweise.
//!
//! Cohere schreibt gesprochene Normen uneinheitlich: „Paragraph 280“,
//! „§ 823“ oder nur die Zahl („Nach 433 Absatz 2 Satz 1 BGB“), dazu
//! „St. G. B.“ und „VWGO“. Diese Regeln bringen alles in eine Form:
//! „§ 433 II 1 BGB“ (römisch) oder „§ 433 Abs. 2 S. 1 BGB“ (abgekürzt).
//!
//! Eine Zahl ohne „§“ oder „Paragraf“ davor gilt nur als Norm, wenn ihr ein
//! Normteil („Absatz 2“, „Satz 1“, „Nummer 3“ …) oder ein Gesetz folgt –
//! „50 Euro“ und „60 bis 90 Minuten“ bleiben, wie sie sind. Nach einem
//! Wort, das selbst zählt („Seite 5“, „Absatz 2“), ist sie keine Norm, nach
//! einem Nomen mitten im Satz („Abschnitt 3 Satz 2“) nur, wenn direkt ein
//! Gesetz folgt („die Gesellschafter 124 HGB“), und eine Jahreszahl
//! („Reform 2002 des BGB“) nur, wenn direkt ein Normteil folgt.
//!
//! Schon so geschriebene Zitate („§ 7 I Nr. 2 `VwGO`“, auch die eigene
//! Ausgabe) bleiben, wie sie sind: Römische Absätze und die Satznummer
//! danach werden als Teile gelesen, bestätigen aber eine bloße Zahl nicht
//! („9 V“ bleibt).
//!
//! Ein Zitat endet an einem Satzende („Absatz 1. Absatz 2 regelt …“) und an
//! einem Zeilenumbruch.

use serde::{Deserialize, Serialize};

mod chain;
#[cfg(test)]
mod tests;

use chain::{Chain, Count};

use super::numbers;
use super::statutes::{Statute, Statutes};
use super::words::{self, Word};

/// Wie Absätze und Sätze geschrieben werden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum CitationStyle {
    /// „§ 433 II 1 BGB“
    #[default]
    #[serde(rename = "römisch")]
    Roman,
    /// „§ 433 Abs. 2 S. 1 BGB“
    #[serde(rename = "abgekürzt")]
    Abbreviated,
}

impl CitationStyle {
    /// Wert in `dictionary.toml`.
    #[must_use]
    pub(crate) const fn file_value(self) -> &'static str {
        match self {
            Self::Roman => "römisch",
            Self::Abbreviated => "abgekürzt",
        }
    }
}

/// „Paragraf“ in allen Formen, die vor einer Nummer stehen.
const PARAGRAPH_WORDS: &[&str] = &[
    "paragraf",
    "paragraph",
    "paragrafen",
    "paragraphen",
    "paragrafs",
    "paragraphs",
];

/// „Artikel“ in allen Formen, die vor einer Nummer stehen.
const ARTICLE_WORDS: &[&str] = &["artikel", "artikels", "artikeln"];

/// Wörter, die zwei Nummern derselben Stufe verbinden.
const CONNECTORS: &[&str] = &["und", "oder", "bis", "sowie"];

/// Höchste Nummer, die nach „und“/„bis“ noch als Absatz, Satz usw. gilt
/// („Absatz 2 und 3“); größere Zahlen sind neue Paragrafen („177 und 179“).
const MAX_SUBDIVISION: u32 = 20;

/// Normteile mit Nummer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Subdivision {
    /// Absatz
    Subsection,
    /// Unterabsatz
    Subparagraph,
    /// Satz
    Sentence,
    /// Halbsatz
    HalfSentence,
    /// Nummer
    Item,
    /// Alternative
    Alternative,
    /// Variante
    Variant,
    /// Fall
    Case,
}

/// Was ein Normteil-Wort einleitet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartKind {
    /// Einen Teil mit Nummer („Absatz 2“).
    Numbered(Subdivision),
    /// Einen Buchstaben („Buchstabe b“).
    Letter,
}

/// Ein gelesener Normteil.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// „Absatz 2“, „erste Alternative“
    Numbered { level: Subdivision, value: u32 },
    /// „Buchstabe b“ (klein)
    Letter(char),
}

impl Part {
    /// Die Art des Teils, ohne Wert.
    const fn kind(self) -> PartKind {
        match self {
            Self::Numbered { level, value: _ } => PartKind::Numbered(level),
            Self::Letter(_) => PartKind::Letter,
        }
    }

    /// Ein Absatz?
    const fn is_subsection(self) -> bool {
        match self {
            Self::Numbered { level, value: _ } => match level {
                Subdivision::Subsection => true,
                Subdivision::Subparagraph
                | Subdivision::Sentence
                | Subdivision::HalfSentence
                | Subdivision::Item
                | Subdivision::Alternative
                | Subdivision::Variant
                | Subdivision::Case => false,
            },
            Self::Letter(_) => false,
        }
    }
}

/// Normteile, die ausgeschrieben gesprochen werden.
const PART_WORDS: &[(&str, PartKind)] = &[
    ("absatz", PartKind::Numbered(Subdivision::Subsection)),
    ("absatzes", PartKind::Numbered(Subdivision::Subsection)),
    ("absätze", PartKind::Numbered(Subdivision::Subsection)),
    ("absätzen", PartKind::Numbered(Subdivision::Subsection)),
    ("unterabsatz", PartKind::Numbered(Subdivision::Subparagraph)),
    ("satz", PartKind::Numbered(Subdivision::Sentence)),
    ("satzes", PartKind::Numbered(Subdivision::Sentence)),
    ("sätze", PartKind::Numbered(Subdivision::Sentence)),
    ("halbsatz", PartKind::Numbered(Subdivision::HalfSentence)),
    ("halbsatzes", PartKind::Numbered(Subdivision::HalfSentence)),
    ("nummer", PartKind::Numbered(Subdivision::Item)),
    ("nummern", PartKind::Numbered(Subdivision::Item)),
    ("buchstabe", PartKind::Letter),
    ("buchstaben", PartKind::Letter),
    ("alternative", PartKind::Numbered(Subdivision::Alternative)),
    ("variante", PartKind::Numbered(Subdivision::Variant)),
    ("fall", PartKind::Numbered(Subdivision::Case)),
];

/// Normteile als Abkürzung; nur mit Punkt, damit „alt“ und „var“ als
/// gewöhnliche Wörter unberührt bleiben.
const PART_ABBREVIATIONS: &[(&str, PartKind)] = &[
    ("abs", PartKind::Numbered(Subdivision::Subsection)),
    ("uabs", PartKind::Numbered(Subdivision::Subparagraph)),
    ("s", PartKind::Numbered(Subdivision::Sentence)),
    ("hs", PartKind::Numbered(Subdivision::HalfSentence)),
    ("halbs", PartKind::Numbered(Subdivision::HalfSentence)),
    ("nr", PartKind::Numbered(Subdivision::Item)),
    ("lit", PartKind::Letter),
    ("alt", PartKind::Numbered(Subdivision::Alternative)),
    ("var", PartKind::Numbered(Subdivision::Variant)),
];

/// Womit ein Zitat beginnt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Head {
    /// „§“, „Paragraf“: „§§“, sobald mehrere Nummern folgen.
    Paragraph,
    /// „§§“
    Paragraphs,
    /// „Art.“, „Artikel“
    Article,
    /// Nur die Zahl: ein Zitat nur, wenn ein Normteil oder Gesetz folgt.
    Bare,
}

/// Eine Paragrafen- oder Artikelnummer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Number {
    /// Wie sie geschrieben wird („823a“, „3-5“, „15“).
    text: String,
    /// Die (erste) Zahl.
    value: u32,
    /// Ein Bereich wie „3-5“ (dann „§§“).
    range: Range,
}

impl Number {
    /// „§“ für eine Nummer, „§§“ für einen Bereich.
    const fn count(&self) -> Count {
        match self.range {
            Range::Single => Count::One,
            Range::Span => Count::Several,
        }
    }
}

/// Steht eine Nummer allein oder für einen Bereich?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Range {
    /// „823“
    Single,
    /// „3-5“
    Span,
}

/// Was in einem Zitat auf die Nummer folgen kann.
#[derive(Debug)]
enum Item<'w> {
    /// „Absatz 2“, „erste Alternative“.
    Part {
        part: Part,
        /// Erstes Wort (für Satzzeichen davor).
        first: &'w Word,
        /// Letztes Wort (für Satzzeichen und Leerraum danach).
        last: &'w Word,
    },
    /// „II“ oder „II 1“: ein Absatz, schon römisch geschrieben, und die
    /// Satznummer danach. Bestätigt eine bloße Zahl nicht („9 V“).
    Written {
        roman: &'w Word,
        value: u32,
        sentence: Option<WrittenSentence<'w>>,
    },
    /// „und Absatz 3“: ein weiterer Normteil nach „und“.
    Connected {
        connector: &'w Word,
        part: Part,
        first: &'w Word,
        last: &'w Word,
    },
    /// „und 3“ auf derselben Stufe wie der Teil davor.
    Same {
        connector: Option<&'w Word>,
        level: Subdivision,
        value: u32,
        number: &'w Word,
    },
    /// „und 179“: eine weitere Nummer.
    Norm {
        connector: Option<&'w Word>,
        number: Number,
        word: &'w Word,
    },
    /// „analog“, „ff.“
    Modifier { text: &'static str, word: &'w Word },
    /// „BGB“, „des Grundgesetzes“.
    Statute {
        statute: Statute,
        first: &'w Word,
        last: &'w Word,
    },
    /// „in Verbindung mit (Artikel) 1“.
    Combined {
        first: &'w Word,
        last: &'w Word,
        head: Option<(Head, &'w Word)>,
        number: Number,
        word: &'w Word,
    },
}

/// Die Satznummer nach einem römischen Absatz („II 1“).
#[derive(Debug)]
struct WrittenSentence<'w> {
    word: &'w Word,
    value: u32,
}

impl<'w> Item<'w> {
    /// Wie das letzte Wort dieses Glieds endet.
    fn end(&self) -> End<'w> {
        match self {
            Self::Part {
                last,
                part: _,
                first: _,
            }
            | Self::Connected {
                last,
                connector: _,
                part: _,
                first: _,
            }
            | Self::Statute {
                last,
                statute: _,
                first: _,
            } => End::of(last),
            Self::Same {
                number,
                connector: _,
                level: _,
                value: _,
            } => End::of(number),
            Self::Written {
                roman,
                sentence,
                value: _,
            } => End::of(sentence.as_ref().map_or(*roman, |sentence| sentence.word)),
            Self::Norm {
                word,
                connector: _,
                number: _,
            }
            | Self::Combined {
                word,
                first: _,
                last: _,
                head: _,
                number: _,
            } => End::of(word),
            // Der Punkt von „ff.“ gehört zur Abkürzung.
            Self::Modifier { text, word } => End {
                trail: match text.strip_suffix('.') {
                    Some(_) => word.trail.strip_prefix('.').unwrap_or(&word.trail),
                    None => &word.trail,
                },
                space: &word.space,
            },
        }
    }
}

/// Was vor einer Zahl steht, die ein Zitat beginnen könnte.
#[derive(Debug, Clone, Copy)]
enum Before<'w> {
    /// Nichts: Textanfang.
    Nothing,
    /// Ein Zitat, direkt davor.
    Citation,
    /// Ein Wort am Satzanfang („Nach“, „Gemäß“).
    SentenceStart(&'w Word),
    /// Ein Wort mitten im Satz.
    MidSentence(&'w Word),
}

/// Großgeschriebene Wörter mitten im Satz, die keine Nomen sind (klein).
const CAPITALISED_PRONOUNS: &[&str] = &["sie", "ihr", "ihre", "ihrem", "ihren", "ihrer", "ihnen"];

/// Jahreszahlen, die ohne Kopf nur mit einem Normteil eine Norm sind.
const YEARS: std::ops::RangeInclusive<u32> = 1900..=2099;

/// Formt alle Normzitate in `text` um.
pub(crate) fn format(text: &str, style: CitationStyle, statutes: &Statutes) -> String {
    let (leading, list) = words::split(text);
    let list = statutes.normalise(&list);
    let parser = Parser { style, statutes };
    let mut out: Vec<Word> = Vec::with_capacity(list.len());
    let mut rest = list.as_slice();
    let mut before = Before::Nothing;
    while let Some((first, tail)) = rest.split_first() {
        if let Some((citation, remaining)) = parser.citation(before, rest) {
            if let Before::Citation = before {
                separate(&mut out);
            }
            out.extend(citation);
            rest = remaining;
            before = Before::Citation;
        } else {
            before = if out.last().is_none_or(ends_sentence) {
                Before::SentenceStart(first)
            } else {
                Before::MidSentence(first)
            };
            out.push(first.clone());
            rest = tail;
        }
    }
    words::join(&leading, &out)
}

/// Trennt zwei Zitate direkt hintereinander mit einem Komma, wie
/// [`Chain::separate`] es vor einem „§“ tut („§ 1 Abs. 1, § 2 BGB“), es sei
/// denn, dort stehen schon ein Satzzeichen oder ein Umbruch.
fn separate(out: &mut [Word]) {
    if let Some(word) = out
        .last_mut()
        .filter(|word| word.trail.is_empty() && !word.space.contains('\n'))
    {
        word.trail.push(',');
    }
}

/// Endet mit `word` ein Satz oder eine Zeile?
fn ends_sentence(word: &Word) -> bool {
    word.space.contains('\n') || word.trail.contains(['.', '!', '?', ':'])
}

/// Erkennt Zitate.
struct Parser<'s> {
    style: CitationStyle,
    statutes: &'s Statutes,
}

impl Parser<'_> {
    /// Ein Zitat am Anfang von `words`: die umgeformten Wörter und der Rest.
    /// `before`: was davor steht (siehe die Moduldoku zu bloßen Zahlen).
    fn citation<'w>(
        &self,
        before: Before<'_>,
        words: &'w [Word],
    ) -> Option<(Vec<Word>, &'w [Word])> {
        let (first, tail) = words.split_first()?;
        let (head, after_head) = head_at(words).map_or((Head::Bare, words), |head| (head, tail));
        let (number_word, mut rest) = after_head.split_first()?;
        let number = norm_number(number_word, head)?;
        match head {
            Head::Bare if !self.bare_number_may_start(before, &number, rest) => return None,
            Head::Bare | Head::Paragraph | Head::Paragraphs | Head::Article => {}
        }
        let mut chain = Chain::new(head, first, number_word, &number);
        let mut end = End::of(number_word);
        while !end.closes(rest)
            && let Some((item, remaining)) = self.item(&chain, rest)
        {
            end = item.end();
            chain.push(item, self.style);
            rest = remaining;
        }
        if !end.closes(rest) && head_at(rest).is_some() {
            chain.separate();
        }
        Some((chain.finish()?, rest))
    }

    /// Darf eine bloße Zahl nach `before` ein Zitat beginnen, wenn danach
    /// `rest` folgt?
    fn bare_number_may_start(&self, before: Before<'_>, number: &Number, rest: &[Word]) -> bool {
        let statute_follows = || self.statute_at(Abbreviations::AsUsual, rest).is_some();
        let part_follows = || rest.first().is_some_and(|word| part_kind(word).is_some());
        let counts = |word: &Word| word.is_any(NOT_NORMS) || part_kind(word).is_some();
        let context = match before {
            Before::SentenceStart(word) => !counts(word),
            Before::MidSentence(word) if counts(word) => false,
            Before::MidSentence(word) if is_noun(word) => statute_follows(),
            Before::Nothing | Before::Citation | Before::MidSentence(_) => true,
        };
        context && (!YEARS.contains(&number.value) || part_follows())
    }

    /// Das nächste Glied des Zitats.
    fn item<'w>(&self, chain: &Chain, words: &'w [Word]) -> Option<(Item<'w>, &'w [Word])> {
        if chain.ends_with_statute() {
            return self.combined(words);
        }
        part(words)
            .or_else(|| self.written_part(chain, words))
            .or_else(|| ordinal_part(words))
            .or_else(|| modifier(words))
            .or_else(|| self.statute(chain.abbreviations(), words))
            .or_else(|| self.combined(words))
            .or_else(|| self.continuation(chain, words))
    }

    /// „II“ oder „II 1“ direkt nach der Nummer (oder „analog“, „ff.“): ein
    /// Absatz in der eigenen römischen Schreibweise und die Satznummer danach.
    fn written_part<'w>(&self, chain: &Chain, words: &'w [Word]) -> Option<(Item<'w>, &'w [Word])> {
        if !chain.takes_a_subsection() {
            return None;
        }
        let (roman, rest) = words.split_first()?;
        let value = numbers::from_roman(&roman.core)?;
        let sentence = match rest {
            [number, after @ ..]
                if roman.trail.is_empty()
                    && !End::of(roman).closes(rest)
                    && !self.counts_a_noun(number, after) =>
            {
                numbers::digits(&number.core)
                    .filter(|sentence| *sentence <= MAX_SUBDIVISION)
                    .map(|sentence| {
                        (
                            WrittenSentence {
                                word: number,
                                value: sentence,
                            },
                            after,
                        )
                    })
            }
            [..] => None,
        };
        let (sentence, rest) =
            sentence.map_or((None, rest), |(sentence, after)| (Some(sentence), after));
        let item = Item::Written {
            roman,
            value,
            sentence,
        };
        Some((item, rest))
    }

    /// „BGB“, „des Handelsgesetzbuches“.
    fn statute<'w>(
        &self,
        abbreviations: Abbreviations,
        words: &'w [Word],
    ) -> Option<(Item<'w>, &'w [Word])> {
        let (statute, used) = self.statute_at(abbreviations, words)?;
        let (taken, rest) = words.split_at_checked(used)?;
        let item = Item::Statute {
            statute,
            first: taken.first()?,
            last: taken.last()?,
        };
        Some((item, rest))
    }

    /// „und 3“ (dieselbe Stufe) oder „und 179 BGB“ (weitere Nummer); statt
    /// „und“ geht auch ein Komma am Ende des vorigen Glieds.
    fn continuation<'w>(&self, chain: &Chain, words: &'w [Word]) -> Option<(Item<'w>, &'w [Word])> {
        // „Absatz 2 Satz 1 und Absatz 3 Satz 2“: ein weiterer Normteil.
        if let [connector, after_connector @ ..] = words
            && connector.is_any(CONNECTORS)
            && let Some((Item::Part { part, first, last }, rest)) = part(after_connector)
        {
            let item = Item::Connected {
                connector,
                part,
                first,
                last,
            };
            return Some((item, rest));
        }
        let (connector, number_word, rest) = match words {
            [connector, number, rest @ ..] if connector.is_any(CONNECTORS) => {
                (Some(connector), number, rest)
            }
            [number, rest @ ..] if chain.ends_with_comma() => (None, number, rest),
            [..] => return None,
        };
        // „II und III“, „I, III“: die eigene römische Schreibweise.
        if let Some(value) = numbers::from_roman(&number_word.core)
            && let Some(level @ Subdivision::Subsection) = chain.same_level(value)
        {
            let item = Item::Same {
                connector,
                level,
                value,
                number: number_word,
            };
            return Some((item, rest));
        }
        let number = norm_number(number_word, Head::Bare)?;
        if let Some(level) = chain.same_level(number.value)
            && !self.counts_a_noun(number_word, rest)
        {
            let item = Item::Same {
                connector,
                level,
                value: number.value,
                number: number_word,
            };
            return Some((item, rest));
        }
        self.confirms(chain.abbreviations(), rest).then_some((
            Item::Norm {
                connector,
                number,
                word: number_word,
            },
            rest,
        ))
    }

    /// Zählt `number` das großgeschriebene Wort danach („§ 199 Absatz 1, 3
    /// Jahre“) statt einen weiteren Absatz? Nicht, wenn danach ein Normteil,
    /// ein Gesetz oder ein neuer Satz kommt.
    fn counts_a_noun(&self, number: &Word, rest: &[Word]) -> bool {
        let Some(next) = rest.first() else {
            return false;
        };
        !End::of(number).closes(rest)
            && next.starts_uppercase()
            && part_kind(next).is_none()
            && head_at(rest).is_none()
            && self.statute_at(Abbreviations::AnyCase, rest).is_none()
    }

    /// Folgt auf eine weitere Nummer etwas, das sie als Norm ausweist?
    fn confirms(&self, abbreviations: Abbreviations, words: &[Word]) -> bool {
        match words {
            [first, ..]
                if part_kind(first).is_some()
                    || self.statute_at(abbreviations, words).is_some() =>
            {
                true
            }
            [connector, number, rest @ ..]
                if connector.is_any(CONNECTORS) && norm_number(number, Head::Bare).is_some() =>
            {
                self.confirms(abbreviations, rest)
            }
            // „1 I 2 GG“: Ein römischer Absatz und die Satznummer bestätigen
            // nichts, aber danach kann es noch kommen.
            [roman, number, rest @ ..]
                if numbers::from_roman(&roman.core).is_some()
                    && numbers::digits(&number.core).is_some() =>
            {
                self.confirms(abbreviations, rest)
            }
            [roman, rest @ ..] if numbers::from_roman(&roman.core).is_some() => {
                self.confirms(abbreviations, rest)
            }
            [..] => false,
        }
    }

    /// Ein Gesetz am Anfang von `words` und die Zahl der verbrauchten
    /// Wörter; ein klein geschriebenes Kürzel nur, wo es jede Schreibweise
    /// sein darf.
    fn statute_at(&self, abbreviations: Abbreviations, words: &[Word]) -> Option<(Statute, usize)> {
        let (statute, used) = self.statutes.at_start(words)?;
        // Das letzte Wort trägt das Kürzel, auch nach einem Artikel („des
        // HGB“, aber nicht „der weg“).
        let written = &words.get(..used)?.last()?.core;
        let plain_word = written.eq_ignore_ascii_case(&statute.name)
            && *written != statute.name
            && written.chars().any(char::is_lowercase);
        match abbreviations {
            Abbreviations::AsUsual if plain_word => None,
            Abbreviations::AsUsual | Abbreviations::AnyCase => Some((statute, used)),
        }
    }

    /// „in Verbindung mit“, „i.V.m.“ oder „i. V. m.“, gefolgt von einer
    /// Norm. Eine bloße Zahl danach braucht wie am Zitatanfang einen
    /// Normteil oder ein Gesetz („i.V.m. 1 Absatz 1 GG“, nicht „in
    /// Verbindung mit 3 Zeugen“).
    fn combined<'w>(&self, words: &'w [Word]) -> Option<(Item<'w>, &'w [Word])> {
        let (first, last, rest) = match words {
            [a, b, c, rest @ ..]
                if a.is_any(&["in"]) && b.is_any(&["verbindung"]) && c.is_any(&["mit"]) =>
            {
                (a, c, rest)
            }
            [a, b, c, rest @ ..]
                if a.core == "i"
                    && b.core == "V"
                    && c.core == "m"
                    && a.has_dot()
                    && b.has_dot() =>
            {
                (a, c, rest)
            }
            [a, rest @ ..] if a.is_any(&["i.v.m", "ivm"]) => (a, a, rest),
            [..] => return None,
        };
        let (head, after_head) = match rest.split_first() {
            Some((word, tail)) => {
                head_at(rest).map_or((None, rest), |head| (Some((head, word)), tail))
            }
            None => (None, rest),
        };
        let (word, remaining) = after_head.split_first()?;
        let number = norm_number(word, head.map_or(Head::Bare, |(head, _)| head))?;
        if head.is_none() && !self.confirms(Abbreviations::AsUsual, remaining) {
            return None;
        }
        let item = Item::Combined {
            first,
            last,
            head,
            number,
            word,
        };
        Some((item, remaining))
    }
}

/// Ein Nomen mitten im Satz: großgeschrieben, mit Kleinbuchstaben, kein
/// Pronomen der Höflichkeitsform.
fn is_noun(word: &Word) -> bool {
    word.starts_uppercase()
        && word.core.chars().any(char::is_lowercase)
        && !word.is_any(CAPITALISED_PRONOUNS)
}

/// Wie ein Gesetzeskürzel geschrieben sein muss, damit es zählt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Abbreviations {
    /// „stgb“ oder „Stgb“ wie „`StGB`“.
    AnyCase,
    /// Wie üblich („`VwGO`“) oder in Großbuchstaben („VWGO“): Nach einer
    /// bloßen Zahl ist „weg“ das Wort, nicht das WEG („um 5 weg“).
    AsUsual,
}

/// Wörter, nach denen eine bloße Zahl etwas anderes zählt als Normen
/// (klein).
const NOT_NORMS: &[&str] = &[
    "abb",
    "abbildung",
    "aufgabe",
    "aufgaben",
    "auflage",
    "band",
    "fall",
    "folie",
    "folien",
    "fn",
    "frage",
    "fragen",
    "fußnote",
    "fußnoten",
    "heft",
    "kapitel",
    "minute",
    "minuten",
    "punkt",
    "rdnr",
    "randnummer",
    "randnummern",
    "rn",
    "s",
    "seite",
    "seiten",
    "tabelle",
    "teil",
    "uhr",
    "zeile",
    "zeilen",
];
/// Beginnt mit `words` ein Zitat („§“, „Paragraf“, „Artikel“, „Art.“)?
/// „Art.“ vor einem großgeschriebenen Wort ist das Nomen am Satzende („auf
/// diese Art. Drei Dinge …“).
fn head_at(words: &[Word]) -> Option<Head> {
    let (word, rest) = words.split_first()?;
    match word.core.as_str() {
        "§" => Some(Head::Paragraph),
        "§§" => Some(Head::Paragraphs),
        _ if word.is_any(PARAGRAPH_WORDS) => Some(Head::Paragraph),
        _ if word.is_any(ARTICLE_WORDS) => Some(Head::Article),
        "Art" if word.has_dot() && !rest.first().is_some_and(Word::starts_uppercase) => {
            Some(Head::Article)
        }
        _ => None,
    }
}

/// Wie das zuletzt gelesene Wort eines Zitats endet.
#[derive(Debug, Clone, Copy)]
struct End<'w> {
    /// Satzzeichen danach, ohne den Punkt einer Abkürzung („ff.“).
    trail: &'w str,
    /// Leerraum danach.
    space: &'w str,
}

impl<'w> End<'w> {
    /// Das Ende von `word`.
    fn of(word: &'w Word) -> Self {
        Self {
            trail: &word.trail,
            space: &word.space,
        }
    }

    /// Endet hier das Zitat, weil ein Satz oder eine Zeile endet? „!“, „?“
    /// und „;“ beenden es immer, ein Punkt oder Doppelpunkt vor einem
    /// großgeschriebenen Wort (`rest`) auch.
    fn closes(self, rest: &[Word]) -> bool {
        self.space.contains('\n')
            || self.trail.contains(CLAUSE_ENDS)
            || (self.trail.contains(['.', ':']) && rest.first().is_some_and(Word::starts_uppercase))
    }
}

/// Satzzeichen, nach denen ein Zitat immer endet.
const CLAUSE_ENDS: [char; 3] = ['!', '?', ';'];

/// Die Nummer nach dem Kopf. Nach einer bloßen Zahl nur Ziffern, sonst auch
/// Zahlwörter („Paragraf fünfzehn“).
fn norm_number(word: &Word, head: Head) -> Option<Number> {
    let core = word.core.as_str();
    if let Some((from, to)) = core.split_once(['-', '–']) {
        let value = numbers::digits(from)?;
        numbers::digits(to)?;
        return Some(Number {
            text: core.to_owned(),
            value,
            range: Range::Span,
        });
    }
    // `trim_end_matches` liefert einen Anfang von `core`, dessen Länge also
    // an einer Zeichengrenze trennt.
    let (digits, suffix) = core.split_at(
        core.trim_end_matches(|c: char| c.is_ascii_lowercase())
            .len(),
    );
    let value = match (numbers::digits(digits), head) {
        (Some(value), _) if suffix.chars().count() <= 1 => value,
        (_, Head::Paragraph | Head::Paragraphs | Head::Article) => numbers::cardinal(core)?,
        (_, Head::Bare) => return None,
    };
    let text = match numbers::digits(digits) {
        Some(_) => core.to_owned(),
        None => value.to_string(),
    };
    Some(Number {
        text,
        value,
        range: Range::Single,
    })
}

/// Art eines Normteil-Worts („Absatz“, „Abs.“).
fn part_kind(word: &Word) -> Option<PartKind> {
    let lower = word.core.to_lowercase();
    let find = |table: &[(&str, PartKind)]| {
        table
            .iter()
            .find(|(name, _)| *name == lower)
            .map(|(_, kind)| *kind)
    };
    find(PART_WORDS).or_else(|| find(PART_ABBREVIATIONS).filter(|_| word.has_dot()))
}

/// „Absatz 2“, „Nummer drei“, „Buchstabe a“.
fn part(words: &[Word]) -> Option<(Item<'_>, &[Word])> {
    let [keyword, number, rest @ ..] = words else {
        return None;
    };
    let part = match part_kind(keyword)? {
        PartKind::Letter => {
            let mut letters = number.core.chars();
            let letter = letters.next().filter(char::is_ascii_alphabetic)?;
            if letters.next().is_some() {
                return None;
            }
            Part::Letter(letter.to_ascii_lowercase())
        }
        PartKind::Numbered(level) => Part::Numbered {
            level,
            value: numbers::cardinal(&number.core)?,
        },
    };
    let item = Item::Part {
        part,
        first: keyword,
        last: number,
    };
    Some((item, rest))
}

/// „erste Alternative“, „1. Alternative“, „zweiter Halbsatz“.
fn ordinal_part(words: &[Word]) -> Option<(Item<'_>, &[Word])> {
    let [ordinal, keyword, rest @ ..] = words else {
        return None;
    };
    // Nur diese Teile werden als Ordnungszahl davor gesprochen.
    let PartKind::Numbered(
        level @ (Subdivision::Alternative
        | Subdivision::Variant
        | Subdivision::Case
        | Subdivision::HalfSentence),
    ) = part_kind(keyword)?
    else {
        return None;
    };
    let value = numbers::ordinal_word(&ordinal.core).or_else(|| {
        (ordinal.trail == ".")
            .then(|| numbers::digits(&ordinal.core))
            .flatten()
    })?;
    let item = Item::Part {
        part: Part::Numbered { level, value },
        first: ordinal,
        last: keyword,
    };
    Some((item, rest))
}

/// „analog“, „ff.“, „folgende“.
fn modifier(words: &[Word]) -> Option<(Item<'_>, &[Word])> {
    let (word, rest) = words.split_first()?;
    let text = match word.core.to_lowercase().as_str() {
        "analog" => "analog",
        "ff" | "folgende" => "ff.",
        _ => return None,
    };
    Some((Item::Modifier { text, word }, rest))
}
