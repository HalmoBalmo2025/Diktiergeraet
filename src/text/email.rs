//! Gesprochene E-Mail-Adressen zusammensetzen.
//!
//! Cohere schreibt eine diktierte Adresse meist als Wörter: „peter.hase at
//! iCloud.com“, „Peter punktmeier at icloud punkt com“. Daraus wird
//! „peter.hase@icloud.com“ bzw. „peter.meier@icloud.com“.
//!
//! Erkannt wird nur, was hinter „at“/„ät“/„@“ mit einer Endung wie „.de“
//! oder „punkt com“ aufhört; sonst bleibt der Text, wie er ist.

use super::words::{self, Word};

/// Wörter für „@“.
const ANCHORS: [&str; 3] = ["at", "ät", "@"];

/// Gesprochene Trennzeichen und was dafür geschrieben wird.
const SEPARATORS: [(&str, &str); 8] = [
    ("punkt", "."),
    ("dot", "."),
    ("unterstrich", "_"),
    ("underscore", "_"),
    ("bindestrich", "-"),
    ("minus", "-"),
    ("dash", "-"),
    ("hyphen", "-"),
];

/// So schreibt das Modell „punkt“ manchmal an das nächste Wort („punktmeier“).
const GLUED_DOT: &str = "punkt";

/// Wörter vor „at“, mit denen keine Adresse beginnt („look at google.com“).
const NOT_LOCAL: &[&str] = &[
    "also",
    "and",
    "are",
    "arrive",
    "arrived",
    "available",
    "be",
    "but",
    "directly",
    "even",
    "found",
    "her",
    "here",
    "him",
    "hosted",
    "is",
    "it",
    "just",
    "live",
    "lives",
    "look",
    "looked",
    "looking",
    "me",
    "meet",
    "not",
    "now",
    "online",
    "only",
    "or",
    "posted",
    "published",
    "see",
    "stay",
    "that",
    "them",
    "then",
    "there",
    "this",
    "us",
    "was",
    "were",
    "work",
    "works",
];

/// Kürzeste und längste erkannte Endung, je einschließlich („de“ bis
/// „museum“).
const TLD_LEN: std::ops::RangeInclusive<usize> = 2..=6;

/// Setzt alle gesprochenen E-Mail-Adressen im Text zusammen.
pub(crate) fn join(text: &str) -> String {
    let (leading, words) = words::split(text);
    let mut spans = Vec::new();
    let mut free_from = 0;
    for anchor in 0..words.len() {
        if anchor < free_from {
            continue;
        }
        if let Some(span) = address_at(&words, anchor, free_from) {
            free_from = span.end.saturating_add(1);
            spans.push(span);
        }
    }
    if spans.is_empty() {
        return text.to_owned();
    }
    let mut result = Vec::with_capacity(words.len());
    let mut spans = spans.into_iter().peekable();
    let mut lead = String::new();
    for (index, word) in words.into_iter().enumerate() {
        match spans.peek() {
            Some(span) if index == span.start => lead = word.lead,
            Some(span) if index > span.start && index < span.end => {}
            Some(span) if index == span.end => {
                result.push(Word {
                    lead: std::mem::take(&mut lead),
                    core: span.address.clone(),
                    trail: word.trail,
                    space: word.space,
                });
                spans.next();
            }
            Some(_) | None => result.push(word),
        }
    }
    words::join(&leading, &result)
}

/// Eine erkannte Adresse: Wörter `start..=end` werden zu `address`.
struct Span {
    /// Erstes Wort der Adresse.
    start: usize,
    /// Letztes Wort der Adresse.
    end: usize,
    /// Die zusammengesetzte Adresse.
    address: String,
}

/// Erkennt eine Adresse um das „at“ an der Stelle `anchor`; davor wird
/// nicht über `min_start` hinaus gesucht.
fn address_at(words: &[Word], anchor: usize, min_start: usize) -> Option<Span> {
    let at = words.get(anchor)?;
    if !at.is_any(&ANCHORS) || !inner(at) {
        return None;
    }
    let last_local = anchor.checked_sub(1).filter(|index| *index >= min_start)?;
    let start = local_start(words, last_local, min_start)?;
    let first = words.get(start)?;
    if start == last_local && (first.is_any(NOT_LOCAL) || is_brand(first)) {
        return None;
    }
    let first_domain = anchor.checked_add(1)?;
    let end = domain_end(words, first_domain)?;
    let local = render(words.get(start..anchor)?);
    let domain = render(words.get(first_domain..=end)?);
    (valid_part(&local) && valid_domain(&domain)).then(|| Span {
        start,
        end,
        address: format!("{local}@{domain}").to_lowercase(),
    })
}

/// Erstes Wort des Teils vor dem „@“, rückwärts ab `last`.
fn local_start(words: &[Word], last: usize, min_start: usize) -> Option<usize> {
    let before_at = words.get(last)?;
    if !before_at.trail.is_empty() || before_at.space.contains('\n') || !is_part(before_at) {
        return None;
    }
    let earlier = |index: usize, by: usize| index.checked_sub(by).filter(|i| *i >= min_start);
    let mut start = last;
    // Ziffern gehören zum Wort davor („muster 1990“).
    if is_digits(before_at)
        && let Some(previous) = earlier(start, 1)
        && words
            .get(previous)
            .is_some_and(|word| inner(word) && is_part(word) && !is_digits(word))
    {
        start = previous;
    }
    loop {
        let current = words.get(start)?;
        if !current.lead.is_empty() {
            break;
        }
        let joinable = |index: usize| {
            words
                .get(index)
                .is_some_and(|word| inner(word) && is_part(word))
        };
        if glued_dot(current).is_some()
            && let Some(previous) = earlier(start, 1).filter(|i| joinable(*i))
        {
            start = previous;
            continue;
        }
        let separated = earlier(start, 1)
            .and_then(|i| words.get(i))
            .is_some_and(|word| inner(word) && separator(word).is_some());
        match earlier(start, 2).filter(|i| separated && joinable(*i)) {
            Some(previous) => start = previous,
            None => break,
        }
    }
    Some(start)
}

/// Letztes Wort des Teils hinter dem „@“, vorwärts ab `first`.
fn domain_end(words: &[Word], first: usize) -> Option<usize> {
    let domain = words.get(first)?;
    if !domain.lead.is_empty() || !is_part(domain) {
        return None;
    }
    let mut end = first;
    loop {
        let current = words.get(end)?;
        if !inner(current) {
            break;
        }
        let separated = end
            .checked_add(1)
            .and_then(|i| words.get(i))
            .is_some_and(|word| inner(word) && separator(word).is_some());
        let next = end.checked_add(2).filter(|i| {
            separated
                && words
                    .get(*i)
                    .is_some_and(|word| word.lead.is_empty() && is_part(word))
        });
        match next {
            Some(index) => end = index,
            None => break,
        }
    }
    Some(end)
}

/// Kein Satzzeichen dran und kein Zeilenumbruch dahinter.
fn inner(word: &Word) -> bool {
    word.lead.is_empty() && word.trail.is_empty() && !word.space.contains('\n')
}

/// Kann das Wort Teil einer Adresse sein (kein Trenn- oder „at“-Wort)?
fn is_part(word: &Word) -> bool {
    !word.core.is_empty()
        && word
            .core
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && separator(word).is_none()
        && !word.is_any(&ANCHORS)
}

/// Markenname mit Binnenmajuskel („visit `OpenCourseWare` at ocw.mit.edu“)?
fn is_brand(word: &Word) -> bool {
    word.core.chars().all(char::is_alphabetic) && word.core.chars().skip(1).any(char::is_uppercase)
}

/// Nur Ziffern?
fn is_digits(word: &Word) -> bool {
    word.core.chars().all(|c| c.is_ascii_digit())
}

/// Das geschriebene Zeichen, wenn das Wort ein gesprochenes Trennzeichen ist.
fn separator(word: &Word) -> Option<&'static str> {
    let lower = word.core.to_lowercase();
    SEPARATORS
        .iter()
        .find(|(spoken, _)| *spoken == lower)
        .map(|(_, written)| *written)
}

/// „punktmeier“ → „meier“.
fn glued_dot(word: &Word) -> Option<String> {
    let lower = word.core.to_lowercase();
    lower
        .strip_prefix(GLUED_DOT)
        .filter(|rest| !rest.is_empty())
        .map(str::to_owned)
}

/// Setzt Wörter ohne Leerzeichen zusammen, Trennwörter als Zeichen.
fn render(words: &[Word]) -> String {
    words
        .iter()
        .map(|word| match (separator(word), glued_dot(word)) {
            (Some(written), _) => written.to_owned(),
            (None, Some(rest)) => format!(".{rest}"),
            (None, None) => word.core.clone(),
        })
        .collect()
}

/// Weder leer noch mit Trennzeichen am Rand oder doppelt.
fn valid_part(part: &str) -> bool {
    !part.is_empty()
        && !part.starts_with(['.', '-', '_'])
        && !part.ends_with(['.', '-', '_'])
        && !part.contains("..")
}

/// „icloud.com“: gültiger Teil mit Punkt und einer Endung aus Buchstaben.
fn valid_domain(domain: &str) -> bool {
    valid_part(domain)
        && domain.rsplit_once('.').is_some_and(|(_, tld)| {
            TLD_LEN.contains(&tld.chars().count()) && tld.chars().all(char::is_alphabetic)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_model_output() {
        assert_eq!(
            join("Meine Adresse ist peter.hase at iCloud.com."),
            "Meine Adresse ist peter.hase@icloud.com."
        );
        assert_eq!(
            join("Schreib an Max.Muster1990 at gmail.de"),
            "Schreib an max.muster1990@gmail.de"
        );
        assert_eq!(
            join("Peter punktmeier at icloud punkt com."),
            "peter.meier@icloud.com."
        );
    }

    #[test]
    fn joins_spoken_separators() {
        assert_eq!(
            join("Das ist max unterstrich muster ät web punkt de, oder?"),
            "Das ist max_muster@web.de, oder?"
        );
        assert_eq!(
            join("info at diktator minus app punkt de"),
            "info@diktator-app.de"
        );
        assert_eq!(
            join("Mail an max punkt muster 1990 at gmail punkt com bitte"),
            "Mail an max.muster1990@gmail.com bitte"
        );
        assert_eq!(
            join("Write to john dot doe at example dot org."),
            "Write to john.doe@example.org."
        );
    }

    #[test]
    fn leaves_other_text_alone() {
        for text in [
            "Wir treffen uns at home.",
            "Look at google.com for details.",
            "Peter at the office.",
            "Das Ergebnis at 5 punkt 3.",
            "Treffen at noon, then lunch.",
            "Er stand bei Punkt 3 at.",
            "Visit MIT OpenCourseWare at ocw.mit.edu.",
        ] {
            assert_eq!(join(text), text);
        }
    }

    #[test]
    fn stops_at_punctuation_and_lines() {
        assert_eq!(
            join("Hallo, peter at icloud.com"),
            "Hallo, peter@icloud.com"
        );
        assert_eq!(
            join("Zeile eins\npeter at web.de"),
            "Zeile eins\npeter@web.de"
        );
        assert_eq!(join("(peter at web.de)"), "(peter@web.de)");
    }
}
