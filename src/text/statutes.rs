//! Gesetze, die in Normzitaten erkannt werden.

use super::words::Word;

/// Gesetze, die nach Paragrafen zählen.
const PARAGRAPH_LAWS: &[&str] = &[
    "AGG",
    "AktG",
    "AO",
    "ArbGG",
    "AsylG",
    "AufenthG",
    "BauGB",
    "BauNVO",
    "BBG",
    "BDSG",
    "BeamtStG",
    "BetrVG",
    "BGB",
    "BImSchG",
    "BRAO",
    "BtMG",
    "BVerfGG",
    "BWahlG",
    "EStG",
    "FamFG",
    "GBO",
    "GewO",
    "GKG",
    "GmbHG",
    "GVG",
    "HGB",
    "HwO",
    "IfSG",
    "InsO",
    "JGG",
    "KrWG",
    "KSchG",
    "LBO",
    "MarkenG",
    "OWiG",
    "PartG",
    "PatG",
    "PBefG",
    "PolG",
    "ProdHaftG",
    "RVG",
    "SGB",
    "SOG",
    "StGB",
    "StPO",
    "StVG",
    "StVO",
    "TzBfG",
    "UrhG",
    "UStG",
    "UWG",
    "VersG",
    "VVG",
    "VwGO",
    "VwVfG",
    "VwZG",
    "WaffG",
    "WEG",
    "ZPO",
    "ZVG",
];

/// Gesetze, die nach Artikeln zählen.
const ARTICLE_LAWS: &[&str] = &[
    "AEUV", "BayBO", "BayVerf", "DSGVO", "EGBGB", "EGStGB", "EMRK", "EUV", "GG", "GRCh", "PAG",
    "WRV",
];

/// Die Kürzel aller eingebauten Gesetze.
pub(crate) fn known_names() -> impl Iterator<Item = &'static str> {
    PARAGRAPH_LAWS.iter().chain(ARTICLE_LAWS).copied()
}

/// Ausgeschriebene Namen (klein, ohne Genitiv-Endung) und ihre Kürzel.
const FULL_NAMES: &[(&str, &str)] = &[
    ("grundgesetz", "GG"),
    ("strafgesetzbuch", "StGB"),
    ("handelsgesetzbuch", "HGB"),
    ("zivilprozessordnung", "ZPO"),
    ("strafprozessordnung", "StPO"),
    ("verwaltungsgerichtsordnung", "VwGO"),
    ("verwaltungsverfahrensgesetz", "VwVfG"),
];

/// Artikel vor einem ausgeschriebenen Gesetz („des Grundgesetzes“).
const ARTICLES: &[&str] = &["des", "der", "dem"];

/// Wie ein Gesetz seine Vorschriften zählt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unit {
    /// § 823 BGB
    Paragraph,
    /// Art. 1 GG
    Article,
}

/// Ein erkanntes Gesetz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Statute {
    /// Kürzel in üblicher Schreibweise („`StGB`“).
    pub(crate) name: String,
    /// Paragrafen oder Artikel.
    pub(crate) unit: Unit,
}

/// Die bekannten Gesetze, ergänzt um die aus dem Wörterbuch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Statutes {
    extra: Vec<String>,
}

impl Statutes {
    /// Bekannte Gesetze plus `extra`. Die zählen nach Paragrafen, bayerische
    /// („Bay…“) nach Artikeln wie fast alle Landesgesetze Bayerns.
    pub(crate) fn new(extra: &[String]) -> Self {
        Self {
            extra: extra.to_vec(),
        }
    }

    /// Das Gesetz zu einem Kürzel, unabhängig von Groß- und Kleinschreibung
    /// („`VWGO`“ → „`VwGO`“).
    pub(crate) fn abbreviation(&self, word: &str) -> Option<Statute> {
        known(word).or_else(|| {
            self.extra
                .iter()
                .find(|law| law.eq_ignore_ascii_case(word))
                .map(|name| Statute {
                    name: name.clone(),
                    unit: if name.starts_with("Bay") {
                        Unit::Article
                    } else {
                        Unit::Paragraph
                    },
                })
        })
    }

    /// Die Wörter mit zusammengefassten buchstabierten Gesetzen („St. G. B.“
    /// → „`StGB`“) und korrigierter Großschreibung („`VWGO`“ → „`VwGO`“);
    /// läuft über den ganzen Text, bevor Zitate gesucht werden.
    pub(crate) fn normalise(&self, list: &[Word]) -> Vec<Word> {
        merge_spelled(list, self)
    }

    /// Ein Gesetz am Anfang von `words`: Kürzel („BGB“, „stgb“) oder
    /// ausgeschriebener Name, beide auch mit Artikel („des HGB“, „des Grundgesetzes“,
    /// „Bürgerlichen Gesetzbuch“). Liefert das Gesetz und die Zahl der
    /// verbrauchten Wörter.
    pub(crate) fn at_start(&self, words: &[Word]) -> Option<(Statute, usize)> {
        match words {
            [article, rest @ ..] if article.is_any(ARTICLES) => {
                let (statute, used) = rest
                    .first()
                    .and_then(|word| self.abbreviation(&word.core))
                    .map(|statute| (statute, 1))
                    .or_else(|| full_name(rest))?;
                Some((statute, used.checked_add(1)?))
            }
            [first, ..] => self
                .abbreviation(&first.core)
                .map(|statute| (statute, 1))
                .or_else(|| full_name(words)),
            [] => None,
        }
    }
}

/// Höchstzahl der Buchstaben einer buchstabierten Abkürzung („St. G. B.“).
const MAX_SPELLED: usize = 6;

/// Fasst buchstabierte Gesetze zusammen („St. G. B.“ → „`StGB`“) und
/// korrigiert großgeschriebene („`VWGO`“ → „`VwGO`“).
fn merge_spelled(list: &[Word], statutes: &Statutes) -> Vec<Word> {
    let mut out = Vec::with_capacity(list.len());
    let mut rest = list;
    while let Some((first, tail)) = rest.split_first() {
        if let Some((merged, remaining)) = spelled_statute(rest, statutes) {
            out.push(merged);
            rest = remaining;
        } else {
            out.push(capitalized_statute(first, statutes));
            rest = tail;
        }
    }
    out
}

/// „St. G. B.“ am Anfang von `words`.
fn spelled_statute<'w>(words: &'w [Word], statutes: &Statutes) -> Option<(Word, &'w [Word])> {
    let letters = words
        .iter()
        .take(MAX_SPELLED)
        .take_while(|word| {
            word.has_dot()
                && word.lead.is_empty()
                && (1..=2).contains(&word.core.chars().count())
                && word.core.chars().all(char::is_alphabetic)
        })
        .count();
    (2..=letters).rev().find_map(|count| {
        let (spelled, rest) = words.split_at_checked(count)?;
        let joined: String = spelled.iter().map(|word| word.core.as_str()).collect();
        // Nur in der Schreibweise des Gesetzes („St. G. B.“): „a. o.
        // Mitglied“ und „a. a. O.“ sind keine Gesetze.
        let statute = statutes
            .abbreviation(&joined)
            .filter(|statute| statute.name == joined)?;
        let (first, last) = (spelled.first()?, spelled.last()?);
        // Der Punkt des letzten Buchstabens gehört zur Abkürzung, es sei
        // denn, danach beginnt ein neuer Satz.
        let sentence_end = last.trail == "." && rest.first().is_none_or(Word::starts_uppercase);
        let trail = if sentence_end {
            last.trail.as_str()
        } else {
            last.trail.strip_prefix('.').unwrap_or(&last.trail)
        };
        let merged = Word {
            lead: first.lead.clone(),
            ..Word::new(&statute.name, trail, &last.space)
        };
        Some((merged, rest))
    })
}

/// „`VWGO`“ → „`VwGO`“; andere Wörter bleiben.
fn capitalized_statute(word: &Word, statutes: &Statutes) -> Word {
    let all_caps =
        word.core.chars().count() >= 2 && word.core.chars().all(|c| c.is_ascii_uppercase());
    match statutes.abbreviation(&word.core).filter(|_| all_caps) {
        Some(statute) => Word {
            core: statute.name,
            ..word.clone()
        },
        None => word.clone(),
    }
}

/// Ausgeschriebener Gesetzesname am Anfang von `words`.
fn full_name(words: &[Word]) -> Option<(Statute, usize)> {
    match words {
        [adjective, noun, ..]
            if adjective.core.to_lowercase().starts_with("bürgerlich")
                && stem(&noun.core) == "gesetzbuch" =>
        {
            Some((known("BGB")?, 2))
        }
        [first, ..] => {
            let stem = stem(&first.core);
            let (_, abbreviation) = FULL_NAMES.iter().find(|(name, _)| *name == stem)?;
            Some((known(abbreviation)?, 1))
        }
        [] => None,
    }
}

/// Ein fest eingebautes Gesetz zu einem Kürzel.
fn known(word: &str) -> Option<Statute> {
    let find = |laws: &[&str], unit| {
        laws.iter()
            .find(|law| law.eq_ignore_ascii_case(word))
            .map(|law| Statute {
                name: (*law).to_owned(),
                unit,
            })
    };
    find(ARTICLE_LAWS, Unit::Article).or_else(|| find(PARAGRAPH_LAWS, Unit::Paragraph))
}

/// Kleingeschriebenes Wort ohne Genitiv-Endung („Grundgesetzes“ →
/// „grundgesetz“).
fn stem(word: &str) -> String {
    let lower = word.to_lowercase();
    let stem = lower
        .strip_suffix("es")
        .or_else(|| lower.strip_suffix('s'))
        .filter(|stem| FULL_NAMES.iter().any(|(name, _)| name == stem) || *stem == "gesetzbuch")
        .unwrap_or(&lower);
    stem.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::words;

    fn first(text: &str) -> Option<(Statute, usize)> {
        let (_, list) = words::split(text);
        Statutes::new(&["BayPAG".to_owned()]).at_start(&list)
    }

    #[test]
    fn finds_abbreviations_in_any_case() {
        assert_eq!(first("VWGO").map(|(s, _)| s.name), Some("VwGO".to_owned()));
        assert_eq!(first("stgb,").map(|(s, _)| s.name), Some("StGB".to_owned()));
        assert_eq!(first("GG").map(|(s, _)| s.unit), Some(Unit::Article));
        assert_eq!(first("BGB").map(|(s, _)| s.unit), Some(Unit::Paragraph));
        assert_eq!(
            first("baypag").map(|(s, _)| (s.name, s.unit)),
            Some(("BayPAG".to_owned(), Unit::Article))
        );
    }

    #[test]
    fn finds_full_names() {
        assert_eq!(
            first("des Handelsgesetzbuches").map(|(s, n)| (s.name, n)),
            Some(("HGB".to_owned(), 2))
        );
        assert_eq!(
            first("Grundgesetz.").map(|(s, n)| (s.name, s.unit, n)),
            Some(("GG".to_owned(), Unit::Article, 1))
        );
        assert_eq!(
            first("des HGB").map(|(s, n)| (s.name, n)),
            Some(("HGB".to_owned(), 2))
        );
        assert_eq!(
            first("Bürgerlichen Gesetzbuchs").map(|(s, n)| (s.name, n)),
            Some(("BGB".to_owned(), 2))
        );
    }

    #[test]
    fn ignores_other_words() {
        assert_eq!(first("Euro"), None);
        assert_eq!(first("des Käufers"), None);
        assert_eq!(first(""), None);
    }
}
