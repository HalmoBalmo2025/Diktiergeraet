//! Erfundene Schlusssätze und Geräusch-Vermerke entfernen.
//!
//! In einer Pause am Ende eines Diktats schreibt Cohere manchmal einen Satz,
//! der nie gesagt wurde – offenbar aus Parlamentsreden gelernt: „Herr
//! Präsident!“. Steht so ein Satz allein am Ende, fällt er weg.
//!
//! Hört das Modell fast nur Stille oder den Klick der Taste, schreibt es
//! auch Untertitel-Vermerke wie „… Musik …“ oder „\[Applaus\]“ (aus Videos
//! gelernt). Besteht ein Teil nur daraus, fällt er weg; in Klammern oder
//! Sternchen stehende Vermerke fallen auch mitten im Text weg.
//!
//! Allgemeiner fängt [`implausibly_long`] erfundene Teile: Mehr Text, als in
//! der gehörten Sprechzeit gesagt werden kann, ist nicht gesprochen worden.

use std::time::Duration;

use super::words;

/// Schlusssätze, die das Modell in Stille erfindet (kleingeschrieben, ohne
/// Satzzeichen am Ende).
const PHANTOMS: [&str; 4] = [
    "herr präsident",
    "frau präsidentin",
    "herr präsident, meine damen und herren",
    "vielen dank, herr präsident",
];

/// Wörter in Geräusch-Vermerken (klein).
const NOISE: [&str; 22] = [
    "musik",
    "music",
    "applaus",
    "applause",
    "beifall",
    "lachen",
    "lacht",
    "gelächter",
    "laughter",
    "laughs",
    "geräusch",
    "geräusche",
    "hintergrundgeräusche",
    "noise",
    "stille",
    "silence",
    "husten",
    "räuspern",
    "unverständlich",
    "inaudible",
    "seufzt",
    "♪",
];

/// So viele Zeichen (ohne Leerzeichen) schafft auch schnelles Sprechen
/// nicht pro Sekunde: zügiges Deutsch liegt bei etwa 15, sehr schnelles bei
/// gut 20. Die gemessene Sprechzeit enthält zudem etwas Stille am Rand.
const MAX_CHARS_PER_SECOND: u128 = 30;

/// Klammern, in denen Vermerke stehen.
#[derive(Debug, Clone, Copy)]
struct Mark {
    open: char,
    close: char,
}

/// Die Klammern der Vermerke („\[Applaus\]“, „(Lachen)“, „*Musik*“).
const MARKS: [Mark; 3] = [
    Mark {
        open: '[',
        close: ']',
    },
    Mark {
        open: '(',
        close: ')',
    },
    Mark {
        open: '*',
        close: '*',
    },
];

/// Satzzeichen, vor denen nach einem entfernten Vermerk kein Leerzeichen
/// stehen bleibt („Er kam (Lachen).“ → „Er kam.“).
const CLOSING_MARKS: [char; 6] = ['.', ',', '!', '?', ';', ':'];

/// Längster Vermerk in Klammern (Zeichen), der geprüft wird.
const MAX_MARK_LEN: usize = 40;

/// Besteht `words` nur aus Geräusch-Wörtern?
fn only_noise(words: &str) -> bool {
    let mut words = words
        .split(|c: char| !(c.is_alphanumeric() || c == '♪'))
        .filter(|word| !word.is_empty())
        .peekable();
    words.peek().is_some() && words.all(|word| NOISE.contains(&word.to_lowercase().as_str()))
}

/// Entfernt Geräusch-Vermerke: den ganzen Text, wenn er nur aus einem
/// besteht („… Musik …“), sonst Vermerke in Klammern („\[Applaus\]“).
/// Aufgeräumt wird nur der Leerraum an der Stelle eines entfernten Vermerks;
/// der übrige Text bleibt, wie er ist („Merci !“, „ …“).
#[must_use]
pub(crate) fn drop_noise(text: &str) -> String {
    if only_noise(text) {
        return String::new();
    }
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    let mut dropped = Dropped::Nothing;
    while let Some((before, mark, after_open)) = first_open(rest) {
        result.push_str(before);
        match after_open.split_once(mark.close) {
            Some((inner, after_mark))
                if inner.chars().count() <= MAX_MARK_LEN && only_noise(inner) =>
            {
                rest = close_gap(&mut result, after_mark);
                dropped = Dropped::Marks;
            }
            Some(_) | None => {
                result.push(mark.open);
                rest = after_open;
            }
        }
    }
    result.push_str(rest);
    // Was nach dem Entfernen übrig bleibt, kann selbst nur noch Geräusch
    // oder Satzzeichen sein („♪ [Musik] ♪“).
    let only_rest = match dropped {
        Dropped::Nothing => false,
        Dropped::Marks => only_noise(&result) || !result.chars().any(char::is_alphanumeric),
    };
    if only_rest { String::new() } else { result }
}

/// Ob [`drop_noise`] einen Vermerk entfernt hat.
#[derive(Debug, Clone, Copy)]
enum Dropped {
    Nothing,
    Marks,
}

/// Der erste Vermerk-Anfang in `text`: Text davor, die Klammer, Text danach.
fn first_open(text: &str) -> Option<(&str, Mark, &str)> {
    MARKS
        .iter()
        .filter_map(|mark| {
            text.split_once(mark.open)
                .map(|(before, after)| (before, *mark, after))
        })
        .min_by_key(|(before, _, _)| before.len())
}

/// Schließt die Lücke eines entfernten Vermerks zwischen `result` (Text
/// davor) und `after` (Text danach) und liefert den Rest ab dem nächsten
/// Wort. Vom Leerraum links und rechts bleibt einer, der mit mehr
/// Zeilenumbrüchen (bei Gleichstand der linke); vor einem Satzzeichen und
/// am Rand bleibt keiner.
fn close_gap<'t>(result: &mut String, after: &'t str) -> &'t str {
    let left_space = result.split_off(result.trim_end().len());
    let (right_space, right) = words::split_leading_whitespace(after);
    let newlines = |space: &str| space.matches('\n').count();
    let gap = if result.is_empty() || right.is_empty() || right.starts_with(CLOSING_MARKS) {
        ""
    } else if newlines(right_space) > newlines(&left_space) || left_space.is_empty() {
        right_space
    } else {
        &left_space
    };
    result.push_str(gap);
    right
}

/// Ist `text` zu lang für `speech` Sprechzeit, also erfunden (etwa ein
/// ganzer Satz zu einem Tastenklick)?
#[must_use]
pub(crate) fn implausibly_long(text: &str, speech: Duration) -> bool {
    let chars = text.chars().filter(|c| !c.is_whitespace()).count();
    let chars = u128::try_from(chars).unwrap_or(u128::MAX);
    chars.saturating_mul(1000) > speech.as_millis().saturating_mul(MAX_CHARS_PER_SECOND)
}

/// Zeichen, an denen der letzte Satz eines Diktats beginnt, wenn danach
/// ein erfundener Schlusssatz steht.
const PHANTOM_SENTENCE_BREAKS: [char; 3] = ['.', '!', '?'];

/// Entfernt einen erfundenen Schlusssatz.
#[must_use]
pub(crate) fn drop_phantom_ending(text: &str) -> String {
    let body = text.trim_end().trim_end_matches(PHANTOM_SENTENCE_BREAKS);
    let mut sentences: Vec<&str> = body.split_inclusive(PHANTOM_SENTENCE_BREAKS).collect();
    let Some(last) = sentences.pop() else {
        return text.to_owned();
    };
    if PHANTOMS.contains(&last.trim().to_lowercase().as_str()) {
        sentences.concat().trim_end().to_owned()
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_text_too_long_for_the_speech() {
        let seconds = Duration::from_secs_f64;
        // Ein Satz zu einem Tastenklick.
        assert!(
            implausibly_long("Herr Präsident!", seconds(0.4)),
            "zwei Wörter in 0,4 s sind zu viel"
        );
        assert!(
            implausibly_long("Vielen Dank für Ihre Aufmerksamkeit.", seconds(0.8)),
            "ein ganzer Satz in 0,8 s ist zu viel"
        );
        // Normales und sehr schnelles Sprechen.
        assert!(
            !implausibly_long("Ich gehe heute nach Hause.", seconds(2.0)),
            "normales Sprechtempo ist plausibel"
        );
        assert!(
            !implausibly_long(
                "Das ist ein ziemlich schnell gesprochener Satz mit vielen Wörtern.",
                seconds(2.5)
            ),
            "sehr schnelles Sprechen ist noch plausibel"
        );
        assert!(
            !implausibly_long("Ja.", seconds(0.5)),
            "ein kurzes Wort zu kurzem Ton ist plausibel"
        );
    }

    #[test]
    fn drops_invented_last_sentence() {
        assert_eq!(
            drop_phantom_ending("Ich komme morgen. Herr Präsident!"),
            "Ich komme morgen."
        );
        assert_eq!(drop_phantom_ending("Herr Präsident!"), "");
        assert_eq!(drop_phantom_ending("Gut so? Frau Präsidentin."), "Gut so?");
    }

    #[test]
    fn drops_noise_marks() {
        assert_eq!(drop_noise("... Musik ..."), "");
        assert_eq!(drop_noise("[Musik]"), "");
        assert_eq!(drop_noise("*Applaus*"), "");
        assert_eq!(drop_noise("♪ ♪"), "");
        assert_eq!(drop_noise("Danke. [Applaus]"), "Danke.");
        assert_eq!(drop_noise("Er kam (Lachen) und ging."), "Er kam und ging.");
        assert_eq!(drop_noise("Er kam (Lachen)."), "Er kam.");
        assert_eq!(drop_noise("[Musik] Hallo [Applaus] [Musik]"), "Hallo");
        assert_eq!(drop_noise("Eins.\n[Musik]\n\nZwei."), "Eins.\n\nZwei.");
    }

    #[test]
    fn tidies_only_around_a_dropped_mark() {
        assert_eq!(drop_noise("Merci ! [Applaus]"), "Merci !");
        assert_eq!(drop_noise("[Musik] Und dann ..."), "Und dann ...");
        assert_eq!(
            drop_noise("Zwei  Leerzeichen (Lachen) hier."),
            "Zwei  Leerzeichen hier."
        );
    }

    #[test]
    fn keeps_an_unclosed_or_long_mark() {
        assert_eq!(drop_noise("Er sagte (Musik"), "Er sagte (Musik");
        assert_eq!(drop_noise("ä (ö) [ü] *"), "ä (ö) [ü] *");
    }

    #[test]
    fn keeps_text_about_noise() {
        for text in [
            "Ich höre Musik.",
            "Musik ist schön.",
            "Er lachte (laut).",
            "Die Musik (Jazz) war gut.",
            "2 * 3 = 6",
            "§ 433 (2) BGB",
        ] {
            assert_eq!(drop_noise(text), text);
        }
    }

    #[test]
    fn keeps_real_sentences() {
        for text in [
            "Ich komme morgen.",
            "Der Herr Präsident kommt morgen.",
            "Herr Präsident, ich bitte ums Wort.",
            "Danke, Herr Präsident Müller!",
        ] {
            assert_eq!(drop_phantom_ending(text), text);
        }
    }
}
