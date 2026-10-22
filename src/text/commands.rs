//! Sprachbefehle im Diktat: „neuer Absatz“ → Leerzeile, „neue Zeile“ →
//! Zeilenumbruch (auch auf Englisch: „new paragraph“, „new line“).
//!
//! Cohere schreibt den Befehl als Wörter, meist mit Satzzeichen drumherum
//! („… gut. Neuer Absatz. Morgen …“ oder „… gut, neue Zeile, Milch …“).
//! Der Befehl samt Satzzeichen und Leerraum wird ersetzt; nach einem neuen
//! Absatz beginnt der Text groß. Folgt eine Zahl („neuer Absatz 2“), ist es
//! kein Befehl, sondern ein Normzitat. Ein Befehl gilt nur an einer
//! Satzgrenze: am Anfang des Diktats oder nach `.`, `,`, `:`, `;`, `!`, `?`
//! (so entschieden); mitten im Satz („wurde ein neuer Absatz eingefügt“, „a
//! new line of products“) bleibt er Text.

use super::numbers;

/// Befehle (klein geschrieben) und was dafür eingesetzt wird.
const COMMANDS: [(&str, Break); 4] = [
    ("neuer absatz", Break::Paragraph),
    ("neue zeile", Break::Line),
    ("new paragraph", Break::Paragraph),
    ("new line", Break::Line),
];

/// Satzzeichen, die der Befehl schluckt (nach ihm) bzw. die vor ihm
/// wegfallen.
const SWALLOWED: [char; 6] = ['.', ',', ';', '!', '?', ':'];

/// Was ein Befehl einsetzt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Break {
    /// Leerzeile; danach beginnt der Text groß.
    Paragraph,
    /// Zeilenumbruch.
    Line,
}

impl Break {
    const fn text(self) -> &'static str {
        match self {
            Self::Paragraph => "\n\n",
            Self::Line => "\n",
        }
    }
}

/// Ersetzt die Sprachbefehle in `text`.
pub(crate) fn apply(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    let mut word_start = true;
    let mut capitalize = false;
    loop {
        if word_start
            && at_boundary(&result)
            && let Some((after, command)) = command_at(rest)
        {
            // Komma, Semikolon und Leerzeichen vor dem Befehl fallen weg; ein
            // Satzende bleibt stehen, ein Umbruch eines Befehls davor auch
            // („Neue Zeile. Neue Zeile.“ gibt eine Leerzeile).
            let kept = result.trim_end_matches([' ', '\t', ',', ';']).len();
            result.truncate(kept);
            result.push_str(command.text());
            rest = after.trim_start_matches(|c: char| c == ' ' || SWALLOWED.contains(&c));
            capitalize = command == Break::Paragraph;
            word_start = true;
            continue;
        }
        let mut chars = rest.chars();
        let Some(next) = chars.next() else {
            break;
        };
        if capitalize && next.is_alphabetic() {
            result.extend(next.to_uppercase());
        } else {
            result.push(next);
        }
        capitalize &= !next.is_alphanumeric();
        word_start = !next.is_alphanumeric();
        rest = chars.as_str();
    }
    result
}

/// Steht nach `before` eine Satzgrenze: nichts, ein Satzzeichen oder ein
/// Umbruch (eines Befehls davor), Leerzeichen dazwischen egal?
fn at_boundary(before: &str) -> bool {
    before
        .trim_end_matches([' ', '\t'])
        .chars()
        .next_back()
        .is_none_or(|last| SWALLOWED.contains(&last) || last == '\n')
}

/// Beginnt `text` mit einem Befehl? Dann der Text danach und was er
/// einsetzt.
fn command_at(text: &str) -> Option<(&str, Break)> {
    COMMANDS.iter().find_map(|(phrase, command)| {
        let after = strip_prefix_ignoring_case(text, phrase)?;
        ends_command(after).then_some((after, *command))
    })
}

/// `text` ohne den Anfang, der ohne Rücksicht auf Groß- und Kleinschreibung
/// `phrase` ist; `None`, wenn er nicht damit beginnt.
fn strip_prefix_ignoring_case<'t>(text: &'t str, phrase: &str) -> Option<&'t str> {
    let mut chars = text.chars();
    for expected in phrase.chars() {
        let actual = chars.next()?;
        if !actual.to_lowercase().eq(expected.to_lowercase()) {
            return None;
        }
    }
    Some(chars.as_str())
}

/// Endet der Befehl hier – kein Wort klebt an, und es folgt nicht direkt
/// eine Zahl, auch nicht als Wort („neuer Absatz 2“, „neuer Absatz zwei“
/// gehören zu einem Zitat; nach „Neuer Absatz.“ darf ein Satz mit einer
/// Zahl beginnen)?
fn ends_command(after: &str) -> bool {
    if after.chars().next().is_some_and(char::is_alphanumeric) {
        return false;
    }
    let following = after.trim_start_matches(' ');
    // Ohne Zeichen dahinter ist der ganze Rest das Wort.
    let next_word = following
        .split_once(|c: char| !c.is_alphanumeric())
        .map_or(following, |(word, _)| word);
    !next_word.starts_with(|c: char| c.is_ascii_digit()) && numbers::cardinal(next_word).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_counts_only_at_a_sentence_boundary() {
        assert_eq!(
            apply("Im Gesetz wurde ein neuer Absatz eingefügt."),
            "Im Gesetz wurde ein neuer Absatz eingefügt."
        );
        assert_eq!(
            apply("We added a new line of products."),
            "We added a new line of products."
        );
        assert_eq!(apply("Neue Zeile. Erst hier."), "\nErst hier.");
        assert_eq!(apply("Gut! Neuer Absatz. Weiter."), "Gut!\n\nWeiter.");
    }

    #[test]
    fn a_number_may_start_the_next_sentence() {
        assert_eq!(
            apply("Das war gut. Neuer Absatz. 2026 beginnt neu."),
            "Das war gut.\n\n2026 beginnt neu."
        );
    }

    #[test]
    fn inserts_paragraphs_and_lines() {
        assert_eq!(
            apply("Das war gut. Neuer Absatz. morgen geht es weiter."),
            "Das war gut.\n\nMorgen geht es weiter."
        );
        assert_eq!(
            apply("Einkaufsliste: neue Zeile, Milch, neue Zeile, Brot."),
            "Einkaufsliste:\nMilch\nBrot."
        );
        assert_eq!(
            apply("That's it. New paragraph. Next"),
            "That's it.\n\nNext"
        );
    }

    #[test]
    fn works_at_the_start_and_end() {
        assert_eq!(apply("Neuer Absatz. Hallo."), "\n\nHallo.");
        assert_eq!(apply("Erste Zeile. Neue Zeile."), "Erste Zeile.\n");
    }

    #[test]
    fn leaves_other_text_alone() {
        assert_eq!(
            apply("Nach § 433 neuer Absatz 2 BGB gilt das."),
            "Nach § 433 neuer Absatz 2 BGB gilt das."
        );
        assert_eq!(
            apply("Die neuen Zeilen sind da."),
            "Die neuen Zeilen sind da."
        );
        assert_eq!(apply("Ein erneuter Absatz."), "Ein erneuter Absatz.");
        assert_eq!(
            apply("Nach § 433 neuer Absatz zwei BGB gilt das."),
            "Nach § 433 neuer Absatz zwei BGB gilt das."
        );
    }

    #[test]
    fn two_commands_in_a_row_give_two_breaks() {
        assert_eq!(
            apply("Erste Zeile. Neue Zeile. Neue Zeile. Dritte Zeile."),
            "Erste Zeile.\n\nDritte Zeile."
        );
        assert_eq!(
            apply("Ende. Neuer Absatz. Neue Zeile. Weiter."),
            "Ende.\n\n\nWeiter."
        );
    }
}
