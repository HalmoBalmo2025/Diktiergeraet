//! Nachbearbeitung von Text ohne Sprachmodell: Geräusch-Vermerke und
//! erfundene Sätze entfernen, E-Mail-Adressen (nur in Diktaten),
//! Wörterbuch, Jura-Regeln, Zahlen, Absätze, Wortzahl.
//!
//! Was in welcher Reihenfolge geschieht, steht an einer Stelle:
//! [`clean_dictation`] für Diktate, [`clean_transcript`] für Videos und
//! Dateien.

mod commands;
pub(crate) mod dictionary;
mod email;
mod figures;
mod fit;
mod legal;
mod numbers;
mod phantom;
mod statutes;
mod terms;
mod words;

use dictionary::Dictionary;
pub(crate) use fit::{AppStyle, Surroundings, fit};
#[cfg(test)]
pub(crate) use legal::CitationStyle;
pub(crate) use phantom::implausibly_long;
use phantom::{drop_noise, drop_phantom_ending};

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::language::Language;

/// Zielgröße eines Absatzes in langen Transkripten (Zeichen).
const PARAGRAPH_LEN: usize = 600;

/// So weit darf eine Kapitelüberschrift von ihrem geschätzten Platz an einen
/// Satzanfang rücken. Die Zeiten der Wörter sind nur geschätzt und liegen
/// einige Sekunden daneben; weiter rückt sie nicht, damit ein Text ohne
/// Satzzeichen seine Überschriften nicht weit verschoben bekommt.
const CHAPTER_SNAP: Duration = Duration::from_secs(30);

/// Ob vor jedem Absatz eines langen Transkripts steht, wann er beginnt.
/// In Einstellungsdatei und Panel `true` (an) oder `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub(crate) enum Timestamps {
    /// Nur Text.
    #[default]
    Off,
    /// `[12:30]` vor jedem Absatz.
    On,
}

impl From<bool> for Timestamps {
    fn from(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }
}

impl From<Timestamps> for bool {
    fn from(timestamps: Timestamps) -> Self {
        match timestamps {
            Timestamps::On => true,
            Timestamps::Off => false,
        }
    }
}

/// Ob ein Video-Transkript die Kapitel des Videos als Überschriften
/// bekommt. In Einstellungsdatei und Panel `true` (an) oder `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub(crate) enum ChapterHeadings {
    /// `## Titel` vor jedem Kapitel.
    On,
    /// Nur Text.
    Off,
}

impl From<bool> for ChapterHeadings {
    fn from(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }
}

impl From<ChapterHeadings> for bool {
    fn from(chapters: ChapterHeadings) -> Self {
        match chapters {
            ChapterHeadings::On => true,
            ChapterHeadings::Off => false,
        }
    }
}

/// Ein erkannter Abschnitt eines langen Transkripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Piece {
    /// Was das Modell erkannt hat.
    pub text: String,
    /// Wo der Abschnitt in der Aufnahme beginnt.
    pub start: Duration,
    /// Wie lang er ist.
    pub length: Duration,
}

/// Ein Kapitel eines Videos (von `YouTube`); wird im Transkript zur
/// Überschrift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chapter {
    /// Wo es beginnt.
    pub start: Duration,
    /// Sein Titel.
    pub title: String,
}

/// Ein Wort mit der geschätzten Zeit, zu der es gesprochen wurde.
struct TimedWord {
    word: String,
    at: Duration,
}

/// Ein erkannter Diktat-Teil, wie er eingefügt wird.
///
/// Ohne Geräusch-Vermerke und erfundenen Schlusssatz, Sprachbefehle
/// („neuer Absatz“) als Umbrüche, gesprochene E-Mail-Adressen
/// zusammengesetzt, dann die Regeln für jeden Text (Wörterbuch, bei Deutsch
/// Normzitate und Zahlen). Umbrüche am Anfang oder Ende bleiben stehen.
#[must_use]
pub(crate) fn clean_dictation(raw: &str, dictionary: &Dictionary, language: Language) -> String {
    let text = email::join(&commands::apply(&drop_phantom_ending(&drop_noise(
        raw.trim(),
    ))));
    rewrite(&text, dictionary, language)
        .trim_matches(' ')
        .to_owned()
}

/// Ein langes Transkript aus seinen Abschnitten.
///
/// Jeder Abschnitt ohne Geräusch-Vermerke, dann die Regeln für jeden Text,
/// aber ohne E-Mail-Adressen: In Videos ist „at example.com“ meist eine
/// Webadresse („Visit our website at example.com“), keine diktierte Mail.
/// dann alles in Absätze geteilt, auf Wunsch mit Zeitmarken; jedes Kapitel
/// beginnt mit seinem Titel als Überschrift (`## Titel`) am Satzanfang, der
/// seinem Beginn am nächsten liegt ([`heading_positions`]). Erfundene
/// Schlusssätze werden hier nicht gesucht – in einer Parlamentsrede sind
/// sie echt.
#[must_use]
pub(crate) fn clean_transcript(
    pieces: &[Piece],
    dictionary: &Dictionary,
    language: Language,
    timestamps: Timestamps,
    chapters: &[Chapter],
) -> String {
    let words: Vec<TimedWord> = pieces
        .iter()
        .flat_map(|piece| {
            let text = rewrite(&drop_noise(piece.text.trim()), dictionary, language);
            timed_words(&text, piece.start, piece.length, language)
        })
        .collect();
    paragraphs(&words, PARAGRAPH_LEN, timestamps, chapters, language.gap())
}

/// Satzzeichen am Satzende in Sprachen ohne Leerzeichen.
const FULL_WIDTH_ENDS: [char; 3] = ['。', '！', '？'];

/// Die Wörter eines Abschnitts, gleichmäßig über seine Länge verteilt. In
/// Sprachen ohne Leerzeichen (Chinesisch, Japanisch) zählt jeder Satz als
/// ein „Wort“, damit Absätze an Satzenden umbrechen können.
fn timed_words(
    text: &str,
    start: Duration,
    length: Duration,
    language: Language,
) -> Vec<TimedWord> {
    let words: Vec<&str> = if language.without_spaces() {
        text.split_inclusive(FULL_WIDTH_ENDS)
            .map(str::trim)
            .filter(|sentence| !sentence.is_empty())
            .collect()
    } else {
        text.split_whitespace().collect()
    };
    let count = u32::try_from(words.len()).unwrap_or(u32::MAX).max(1);
    words
        .into_iter()
        .zip(0_u32..)
        .map(|(word, index)| TimedWord {
            word: word.to_owned(),
            at: start.saturating_add(
                length
                    .checked_mul(index)
                    .and_then(|offset| offset.checked_div(count))
                    .unwrap_or_default(),
            ),
        })
        .collect()
}

/// `[2:05]`, ab einer Stunde `[1:02:05]`.
fn stamp(at: Duration) -> String {
    format!("[{}]", clock(at))
}

/// `2:05`, ab einer Stunde `1:02:05`.
#[must_use]
pub(crate) fn clock(at: Duration) -> String {
    let total = at.as_secs();
    let (hours, minutes, seconds) = (total / 3_600, total % 3_600 / 60, total % 60);
    match hours {
        0 => format!("{minutes}:{seconds:02}"),
        hours => format!("{hours}:{minutes:02}:{seconds:02}"),
    }
}

/// Die Regeln für jeden Text: eigene Ersetzungen, Fachbegriffe; nur bei Deutsch danach Normzitate und Zahlen als
/// Ziffern (nach den Zitaten, damit „Absatz 2“ nicht vorher umgeschrieben
/// wird).
fn rewrite(text: &str, dictionary: &Dictionary, language: Language) -> String {
    let replaced = dictionary.fix_terms(&dictionary.replace(text));
    match language {
        Language::German => figures::to_figures(&dictionary.cite(&replaced)),
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
        | Language::Korean => replaced,
    }
}

/// Teilt langen Text in Absätze von etwa `target_len` Zeichen.
///
/// Umbrochen wird nur nach einem Satzende (`.`, `?`, `!`, `。` …), damit
/// kein Satz zerrissen wird. Absätze werden durch eine Leerzeile getrennt,
/// Wörter durch `gap` (in Sprachen ohne Leerzeichen leer).
fn paragraphs(
    words: &[TimedWord],
    target_len: usize,
    timestamps: Timestamps,
    chapters: &[Chapter],
    gap: &str,
) -> String {
    let mut result = String::new();
    let mut paragraph_len = 0_usize;
    let mut headings = heading_positions(words, chapters).into_iter().peekable();
    let mut words = words.iter().enumerate().peekable();
    while let Some((index, TimedWord { word, at })) = words.next() {
        // Beginnt vor diesem Wort ein Kapitel: Überschrift, neuer Absatz.
        while let Some((_, chapter)) = headings.next_if(|(position, _)| *position == index) {
            if !result.is_empty() {
                result.push_str("\n\n");
            }
            result.push_str("## ");
            result.push_str(chapter.title.trim());
            paragraph_len = 0;
        }
        match (paragraph_len, timestamps) {
            (0, Timestamps::On) => {
                if !result.is_empty() {
                    result.push_str("\n\n");
                }
                result.push_str(&stamp(*at));
                result.push(' ');
            }
            (0, Timestamps::Off) if !result.is_empty() => result.push_str("\n\n"),
            (0, Timestamps::Off) => {}
            (_, Timestamps::On | Timestamps::Off) => result.push_str(gap),
        }
        result.push_str(word);
        paragraph_len = paragraph_len
            .saturating_add(word.chars().count())
            .saturating_add(1);
        let next = words.peek().map(|(_, next)| next.word.as_str());
        if paragraph_len >= target_len && ends_sentence(word, next) {
            paragraph_len = 0;
        }
    }
    result
}

/// Vor welchem Wort jedes Kapitel seine Überschrift bekommt, in der
/// Reihenfolge der Kapitel.
///
/// Geschätzt beginnt ein Kapitel vor dem ersten Wort ab seinem Beginn. Weil
/// die Zeiten der Wörter nur gleichmäßig über ihren Abschnitt verteilt sind,
/// läge die Überschrift dort oft mitten im Satz; sie rückt deshalb an den
/// Satzanfang, dessen Zeit dem Beginn am nächsten liegt (bei Gleichstand an
/// den früheren), höchstens [`CHAPTER_SNAP`] weit und nie vor die Überschrift
/// des Kapitels davor. Ein Kapitel nach dem letzten Wort bekommt keine.
fn heading_positions<'chapters>(
    words: &[TimedWord],
    chapters: &'chapters [Chapter],
) -> Vec<(usize, &'chapters Chapter)> {
    let sentence_starts: Vec<(usize, Duration)> = words
        .first()
        .map(|first| (0, first.at))
        .into_iter()
        .chain(
            words
                .array_windows::<2>()
                .zip(1..)
                .filter(|([before, word], _)| ends_sentence(&before.word, Some(&word.word)))
                .map(|([_, word], index)| (index, word.at)),
        )
        .collect();
    let mut earliest = 0;
    chapters
        .iter()
        .filter_map(|chapter| {
            let estimated = words.iter().position(|word| word.at >= chapter.start)?;
            let nearest = sentence_starts
                .iter()
                .filter(|(index, at)| {
                    *index >= earliest && at.abs_diff(chapter.start) <= CHAPTER_SNAP
                })
                .min_by_key(|(_, at)| at.abs_diff(chapter.start))
                .map_or(estimated, |(index, _)| *index);
            earliest = nearest.max(earliest);
            Some((earliest, chapter))
        })
        .collect()
}

/// Zeichen, nach denen ein Absatz umbrechen darf.
const PARAGRAPH_BREAKS_AFTER: [char; 6] = ['.', '?', '!', '。', '！', '？'];

/// Endet das Wort einen Satz, sodass danach (vor `next`) ein Absatz
/// beginnen darf? Nicht der Punkt einer Abkürzung („Abs.“, „z. B.“) oder
/// einer Ordnungszahl („am 3. Mai“), und nicht vor einem
/// kleingeschriebenen Wort.
fn ends_sentence(word: &str, next: Option<&str>) -> bool {
    let bare =
        word.trim_end_matches(|c: char| words::is_quote(c) || matches!(c, ')' | ']' | '」' | '』'));
    if !bare.ends_with(PARAGRAPH_BREAKS_AFTER) || next.is_some_and(starts_lowercase) {
        return false;
    }
    let Some(before_dot) = bare.strip_suffix('.') else {
        return true;
    };
    let ordinal = !before_dot.is_empty() && before_dot.chars().all(|c| c.is_ascii_digit());
    !ordinal && !words::ends_with_abbreviation(bare)
}

/// Beginnt der Text mit einem Kleinbuchstaben?
fn starts_lowercase(text: &str) -> bool {
    text.chars().next().is_some_and(char::is_lowercase)
}

/// Anzahl der Wörter. Chinesisch und Japanisch stehen ohne Leerzeichen; dort
/// zählt jedes Schriftzeichen (Han, Kana) als ein Wort, was der üblichen
/// Wortlänge dort näher kommt als ein ganzer Absatz als ein Wort.
#[must_use]
pub(crate) fn word_count(text: &str) -> usize {
    text.split_whitespace()
        .map(|chunk| {
            let signs = chunk.chars().filter(|c| is_cjk(*c)).count();
            match signs {
                0 => 1,
                signs => {
                    let others = chunk
                        .split(is_cjk)
                        .filter(|part| part.chars().any(char::is_alphanumeric))
                        .count();
                    signs.saturating_add(others)
                }
            }
        })
        .sum()
}

/// Ein Schriftzeichen ohne Leerzeichen dazwischen: Kana und Han.
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{30ff}'
        | '\u{3400}'..='\u{4dbf}'
        | '\u{4e00}'..='\u{9fff}'
        | '\u{f900}'..='\u{faff}'
        | '\u{20000}'..='\u{2fa1f}')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wörter ohne Zeit.
    fn untimed(text: &str) -> Vec<TimedWord> {
        timed_words(text, Duration::ZERO, Duration::ZERO, Language::German)
    }

    /// Ein Abschnitt ab `start` Sekunden, `length` Sekunden lang.
    fn piece(text: &str, start: u64, length: u64) -> Piece {
        Piece {
            text: text.to_owned(),
            start: Duration::from_secs(start),
            length: Duration::from_secs(length),
        }
    }

    #[test]
    fn stamps_each_paragraph_with_its_start() {
        let dictionary = Dictionary::default();
        let long = "Wort ".repeat(130);
        let pieces = [
            piece(&format!("{long}Ende eins."), 0, 30),
            piece("[Musik]", 30, 30),
            piece(&format!("Zweiter Absatz {long}Ende zwei."), 750, 30),
        ];
        let transcript =
            clean_transcript(&pieces, &dictionary, Language::German, Timestamps::On, &[]);
        let paragraphs: Vec<&str> = transcript.split("\n\n").collect();
        assert_eq!(paragraphs.len(), 2);
        assert!(
            paragraphs[0].starts_with("[0:00] Wort"),
            "der erste Absatz trägt den Zeitstempel 0:00"
        );
        assert!(
            paragraphs[1].starts_with("[12:30] Zweiter Absatz"),
            "der zweite Absatz trägt den Zeitstempel seines Anfangs"
        );
        let plain = clean_transcript(&pieces, &dictionary, Language::German, Timestamps::Off, &[]);
        assert!(
            plain.starts_with("Wort"),
            "ohne Zeitstempel beginnt der Text direkt"
        );
        assert!(
            !plain.contains('['),
            "ohne Zeitstempel steht keine Marke im Text"
        );
    }

    #[test]
    fn starts_each_chapter_with_a_heading() {
        let dictionary = Dictionary::default();
        let pieces = [
            piece(
                "Willkommen zur Vorlesung. Heute geht es um Verträge.",
                0,
                20,
            ),
            piece(
                "Der Vertrag kommt durch Angebot und Annahme zustande.",
                20,
                20,
            ),
        ];
        let chapters = [
            Chapter {
                start: Duration::ZERO,
                title: "Einleitung".to_owned(),
            },
            Chapter {
                start: Duration::from_secs(20),
                title: " Vertragsschluss ".to_owned(),
            },
            // Nach dem letzten Wort: keine leere Überschrift.
            Chapter {
                start: Duration::from_secs(90),
                title: "Ende".to_owned(),
            },
        ];
        assert_eq!(
            clean_transcript(
                &pieces,
                &dictionary,
                Language::German,
                Timestamps::On,
                &chapters
            ),
            "## Einleitung\n\n[0:00] Willkommen zur Vorlesung. Heute geht es um Verträge.\n\n\
             ## Vertragsschluss\n\n[0:20] Der Vertrag kommt durch Angebot und Annahme zustande."
        );
    }

    /// Ein Wort je Sekunde ab 0, Überschriften ab den Kapiteln
    /// `(Sekunde, Titel)`, ohne Zeitmarken.
    fn with_chapters(text: &str, chapters: &[(u64, &str)]) -> String {
        let words = timed_words(
            text,
            Duration::ZERO,
            Duration::from_secs(text.split_whitespace().count().try_into().unwrap()),
            Language::German,
        );
        let chapters: Vec<Chapter> = chapters
            .iter()
            .map(|(start, title)| Chapter {
                start: Duration::from_secs(*start),
                title: (*title).to_owned(),
            })
            .collect();
        paragraphs(&words, 600, Timestamps::Off, &chapters, " ")
    }

    #[test]
    fn a_heading_moves_to_the_nearest_sentence_start() {
        // Wörter: Das 0, ist 1, der 2, erste 3, Satz. 4, Hier 5, beginnt 6 …
        let text = "Das ist der erste Satz. Hier beginnt das zweite Kapitel.";
        let expected =
            "## Eins\n\nDas ist der erste Satz.\n\n## Zwei\n\nHier beginnt das zweite Kapitel.";
        // Geschätzt vor „das“: zurück an den Satzanfang.
        assert_eq!(with_chapters(text, &[(0, "Eins"), (7, "Zwei")]), expected);
        // Geschätzt vor „Satz.“: vor an den nächsten Satzanfang.
        assert_eq!(with_chapters(text, &[(0, "Eins"), (4, "Zwei")]), expected);
    }

    #[test]
    fn close_chapters_keep_their_order() {
        let text = "Das ist der erste Satz. Hier beginnt das zweite Kapitel.";
        assert_eq!(
            with_chapters(text, &[(6, "Zwei"), (7, "Drei")]),
            "Das ist der erste Satz.\n\n## Zwei\n\n## Drei\n\nHier beginnt das zweite Kapitel."
        );
    }

    #[test]
    fn without_a_sentence_start_nearby_the_heading_stays() {
        let text = "wort ".repeat(40);
        let placed = with_chapters(&text, &[(35, "Spät")]);
        let before = placed.split("\n\n## Spät\n\n").next().unwrap();
        assert_eq!(before.split_whitespace().count(), 35, "{placed}");
    }

    #[test]
    fn spreads_words_over_their_piece() {
        let words = timed_words(
            "eins zwei drei vier",
            Duration::from_mins(1),
            Duration::from_secs(20),
            Language::German,
        );
        let times: Vec<u64> = words.iter().map(|word| word.at.as_secs()).collect();
        assert_eq!(times, [60, 65, 70, 75]);
        assert_eq!(stamp(Duration::from_secs(3_725)), "[1:02:05]");
        assert_eq!(stamp(Duration::from_secs(65)), "[1:05]");
    }

    #[test]
    fn applies_legal_rules_to_german_only() {
        let dictionary = Dictionary::default();
        assert_eq!(
            clean_dictation("Paragraf 823 Absatz 1 BGB", &dictionary, Language::German),
            "§ 823 I BGB"
        );
        assert_eq!(
            clean_dictation("Paragraf 823 Absatz 1 BGB", &dictionary, Language::English),
            "Paragraf 823 Absatz 1 BGB"
        );
    }

    #[test]
    fn turns_voice_commands_into_breaks() {
        let dictionary = Dictionary::default();
        assert_eq!(
            clean_dictation(
                "Erster Punkt nach Paragraf 433 BGB. Neuer Absatz. Zweiter Punkt kostet 15 Euro. Neue Zeile.",
                &dictionary,
                Language::German
            ),
            "Erster Punkt nach § 433 BGB.\n\nZweiter Punkt kostet 15 Euro.\n"
        );
    }

    #[test]
    fn cleans_dictation_parts() {
        let dictionary = Dictionary::default();
        assert_eq!(
            clean_dictation(
                " [Musik] Schreib an peter punkt hase at icloud punkt com. Herr Präsident!",
                &dictionary,
                Language::German
            ),
            "Schreib an peter.hase@icloud.com."
        );
    }

    #[test]
    fn joins_e_mail_addresses_in_dictations_only() {
        let dictionary = Dictionary::default();
        let texts = [
            "Visit our website at example.com.",
            "Learn more at example.com.",
            "Sign up at example.com today.",
        ];
        for text in texts {
            let pieces = [piece(text, 0, 10)];
            assert_eq!(
                clean_transcript(
                    &pieces,
                    &dictionary,
                    Language::English,
                    Timestamps::Off,
                    &[]
                ),
                text
            );
        }
        assert_eq!(
            clean_dictation("Mail an peter at web.de", &dictionary, Language::German),
            "Mail an peter@web.de"
        );
    }

    #[test]
    fn splits_chinese_transcripts_at_sentence_ends() {
        let dictionary = Dictionary::default();
        let sentence = "这是一个很长的句子，用来测试段落。";
        let pieces: Vec<Piece> = (0..40)
            .map(|index| piece(sentence, index * 30, 30))
            .collect();
        let transcript = clean_transcript(
            &pieces,
            &dictionary,
            Language::Chinese,
            Timestamps::Off,
            &[],
        );
        assert!(
            transcript.contains("\n\n"),
            "ein langes chinesisches Transkript hat Absätze"
        );
        assert!(
            !transcript.contains(' '),
            "zwischen chinesischen Sätzen steht kein Leerzeichen"
        );
        assert!(
            transcript
                .split("\n\n")
                .all(|paragraph| paragraph.ends_with('。')),
            "Absätze enden an Satzenden"
        );
    }

    #[test]
    fn cleans_transcripts_without_dropping_speeches() {
        let dictionary = Dictionary::default();
        let pieces = [
            piece("[Musik]", 0, 30),
            piece(" Herr Präsident! Wir beraten Paragraf 433 BGB. ", 30, 30),
            piece("(Applaus) Danke.", 60, 30),
        ];
        assert_eq!(
            clean_transcript(&pieces, &dictionary, Language::German, Timestamps::Off, &[]),
            "Herr Präsident! Wir beraten § 433 BGB. Danke."
        );
    }

    #[test]
    fn short_text_stays_one_paragraph() {
        assert_eq!(
            paragraphs(
                &untimed("Ein Satz. Noch einer."),
                600,
                Timestamps::Off,
                &[],
                " "
            ),
            "Ein Satz. Noch einer."
        );
    }

    #[test]
    fn breaks_only_after_sentence_end() {
        let text = "Erster langer Satz ohne Ende und weiter so. Zweiter Satz.";
        assert_eq!(
            paragraphs(&untimed(text), 20, Timestamps::Off, &[], " "),
            "Erster langer Satz ohne Ende und weiter so.\n\nZweiter Satz."
        );
    }

    #[test]
    fn recognises_quoted_sentence_end() {
        assert!(
            ends_sentence("gesagt.“", Some("Dann")),
            "Punkt vor schließendem Anführungszeichen endet den Satz"
        );
        assert!(
            ends_sentence("wirklich?", None),
            "Fragezeichen endet den Satz"
        );
        assert!(
            !ends_sentence("Abs", Some("Dann")),
            "ein Wort ohne Satzzeichen endet keinen Satz"
        );
        for (word, next) in [
            ("3.", "Mai"),
            ("Abs.", "1"),
            ("B.", "Hallo"),
            ("Ende.", "und"),
        ] {
            assert!(!ends_sentence(word, Some(next)), "{word} {next}");
        }
    }

    /// Ein langes Transkript aus einem Abschnitt.
    fn long_transcript(text: &str, dictionary: &Dictionary) -> String {
        let pieces = [piece(text, 0, 60)];
        clean_transcript(&pieces, dictionary, Language::German, Timestamps::Off, &[])
    }

    #[test]
    fn no_paragraph_break_after_an_ordinal_or_abbreviation() {
        let ordinal = format!("{}Am 3. Mai kam er. Dann ging er.", "Wort ".repeat(119));
        let plain = long_transcript(&ordinal, &Dictionary::default());
        assert!(
            plain.ends_with("Am 3. Mai kam er.\n\nDann ging er."),
            "{plain}"
        );
        let dictionary = Dictionary::parse("[jura]\nabsatz = \"abgekürzt\"\n").unwrap();
        let citation = format!(
            "{}Das folgt aus Paragraf 823 Absatz 1 BGB. Dann weiter.",
            "Wort ".repeat(115)
        );
        let cited = long_transcript(&citation, &dictionary);
        assert!(
            cited.ends_with("§ 823 Abs. 1 BGB.\n\nDann weiter."),
            "{cited}"
        );
    }

    #[test]
    fn counts_words_of_chinese_and_japanese() {
        let sentence = "这是一个很长的句子，用来测试段落。";
        let pieces: Vec<Piece> = (0..40)
            .map(|index| piece(sentence, index * 30, 30))
            .collect();
        let transcript = clean_transcript(
            &pieces,
            &Dictionary::default(),
            Language::Chinese,
            Timestamps::Off,
            &[],
        );
        assert!(word_count(&transcript) >= 40, "40 Sätze sind mehr Wörter");
        assert_eq!(word_count("こんにちは"), 5);
        assert_eq!(word_count("iPhone 15 买了"), 4);
        assert_eq!(word_count("§ 823 BGB – gut"), 5);
    }

    #[test]
    fn normalises_whitespace() {
        assert_eq!(
            paragraphs(&untimed("  A.\n B. "), 600, Timestamps::Off, &[], " "),
            "A. B."
        );
        assert_eq!(word_count("eins zwei  drei"), 3);
    }
}
