//! Diktat an die Schreibmarke anpassen: Mitten im Satz kein Großbuchstabe
//! am Anfang und kein Punkt am Ende, Leerzeichen zum Nachbartext.
//!
//! Cohere liefert jedes Diktat als eigenen Satz („Nach Hause.“). Steht links
//! der Schreibmarke „Ich gehe heute“, soll daraus „Ich gehe heute nach Hause.“
//! werden.
//!
//! Manche Apps wollen es anders ([`AppStyle`]): In Terminals und Chats fällt
//! der Schlusspunkt weg, in Mail-Programmen bleiben es vollständige Sätze.

use super::words;
use crate::language::Language;

/// Text links und rechts der Schreibmarke (oder der Auswahl).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Surroundings {
    /// Die letzten Zeichen vor der Schreibmarke.
    pub before: String,
    /// Die ersten Zeichen nach der Schreibmarke.
    pub after: String,
}

/// Mail-Programme: Dort bleiben Diktate vollständige Sätze.
const MAIL_APPS: [&str; 4] = [
    "com.apple.mail",
    "com.microsoft.Outlook",
    "com.readdle.smartemail-Mac",
    "com.superhuman.electron",
];

/// Terminals und Chats: Dort fällt der Schlusspunkt weg.
const CHAT_APPS: [&str; 18] = [
    // Terminals
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "dev.warp.Warp-Stable",
    "com.mitchellh.ghostty",
    "net.kovidgoyal.kitty",
    "org.alacritty",
    "io.alacritty",
    "com.github.wez.wezterm",
    // Chats
    "com.apple.MobileSMS",
    "net.whatsapp.WhatsApp",
    "desktop.WhatsApp",
    "com.tinyspeck.slackmacgap",
    "com.hnc.Discord",
    "ru.keepcoder.Telegram",
    "org.telegram.desktop",
    "org.whispersystems.signal-desktop",
    "com.microsoft.teams2",
    "com.facebook.archon",
];

/// Wie ein Diktat in eine bestimmte App eingefügt wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppStyle {
    /// An die Umgebung angepasst: mitten im Satz klein und ohne Punkt.
    Adaptive,
    /// Vollständige Sätze (Mail): großer Anfang, Schlusspunkt; nur das
    /// Leerzeichen zum Nachbartext wird ergänzt.
    Sentences,
    /// Ohne Schlusspunkt (Terminal, Chats), sonst angepasst.
    Chat,
}

impl AppStyle {
    /// Der Stil für die App mit dieser Bundle-ID.
    #[must_use]
    pub(crate) fn of(app: &str) -> Self {
        if MAIL_APPS.contains(&app) {
            Self::Sentences
        } else if CHAT_APPS.contains(&app) {
            Self::Chat
        } else {
            Self::Adaptive
        }
    }

    /// Name für das Protokoll.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Adaptive => "angepasst",
            Self::Sentences => "vollständige Sätze",
            Self::Chat => "ohne Schlusspunkt",
        }
    }
}

/// Zeichen links der Schreibmarke, nach denen das Diktat großgeschrieben
/// bleibt (ein neuer Satz, auch nach einem Doppelpunkt).
const CAPITAL_AFTER: [char; 4] = ['.', '!', '?', ':'];

/// Englische Wörter, die mitten im Satz klein stehen; alle anderen (Namen,
/// Orte) bleiben, wie das Modell sie schreibt.
const ENGLISH_LOWERCASE: &[&str] = &[
    "a", "about", "after", "also", "an", "and", "are", "as", "at", "because", "before", "but",
    "by", "do", "for", "from", "he", "her", "here", "his", "how", "if", "in", "is", "it", "its",
    "just", "maybe", "my", "no", "not", "now", "of", "on", "or", "our", "she", "so", "some",
    "that", "the", "their", "then", "there", "these", "they", "this", "those", "to", "very", "was",
    "we", "were", "what", "when", "where", "which", "who", "why", "with", "yes", "you", "your",
];

/// Leerraum innerhalb einer Zeile.
const BLANKS: [char; 3] = [' ', '\t', '\u{a0}'];

/// Satzzeichen am Diktatende, die wegfallen, wenn der Satz rechts weitergeht.
const CLOSING: [char; 4] = ['.', ',', ';', ':'];

/// Satzzeichen, mit denen der Text rechts der Schreibmarke beginnen kann
/// (dazu schließende Anführungszeichen, siehe [`closes_right`]).
const PUNCTUATION: [char; 8] = ['.', ',', ';', ':', '!', '?', ')', ']'];

/// Deutsche Wörter, die mitten im Satz kleingeschrieben werden. Alle anderen
/// bleiben groß, weil sie Nomen sein könnten. „Sie“ und „Ihr“ fehlen
/// absichtlich (Höflichkeitsform).
const GERMAN_LOWERCASE: &[&str] = &[
    "aber",
    "alle",
    "alles",
    "als",
    "also",
    "am",
    "an",
    "auch",
    "auf",
    "aus",
    "außerdem",
    "bei",
    "beim",
    "bereits",
    "bin",
    "bis",
    "bist",
    "bitte",
    "bzw",
    "da",
    "dabei",
    "dafür",
    "dagegen",
    "daher",
    "damit",
    "dann",
    "darf",
    "darum",
    "das",
    "dass",
    "dem",
    "den",
    "denn",
    "der",
    "deren",
    "des",
    "deshalb",
    "dessen",
    "deswegen",
    "die",
    "dies",
    "diese",
    "diesem",
    "diesen",
    "dieser",
    "dieses",
    "doch",
    "dort",
    "du",
    "durch",
    "dürfen",
    "eben",
    "eigentlich",
    "ein",
    "eine",
    "einem",
    "einen",
    "einer",
    "eines",
    "einfach",
    "entweder",
    "er",
    "erst",
    "es",
    "etwa",
    "etwas",
    "euch",
    "für",
    "ganz",
    "gar",
    "gegen",
    "genau",
    "gerade",
    "gestern",
    "habe",
    "haben",
    "hat",
    "hatte",
    "hatten",
    "heute",
    "hier",
    "ich",
    "im",
    "immer",
    "in",
    "ins",
    "ist",
    "ja",
    "jede",
    "jedem",
    "jeden",
    "jeder",
    "jedes",
    "jetzt",
    "kann",
    "kein",
    "keine",
    "keinen",
    "können",
    "konnte",
    "mal",
    "man",
    "mehr",
    "mein",
    "meine",
    "mich",
    "mir",
    "mit",
    "morgen",
    "muss",
    "müssen",
    "musste",
    "nach",
    "nämlich",
    "nein",
    "nicht",
    "nichts",
    "nie",
    "noch",
    "nun",
    "nur",
    "ob",
    "obwohl",
    "oder",
    "ohne",
    "oft",
    "schon",
    "sehr",
    "sein",
    "seine",
    "seit",
    "sich",
    "sind",
    "so",
    "sodass",
    "sogar",
    "soll",
    "sollen",
    "sollte",
    "sondern",
    "sowie",
    "trotzdem",
    "um",
    "und",
    "uns",
    "unser",
    "unter",
    "viel",
    "viele",
    "vielleicht",
    "vom",
    "von",
    "vor",
    "während",
    "war",
    "waren",
    "warum",
    "was",
    "weder",
    "wegen",
    "weil",
    "welche",
    "welcher",
    "welches",
    "wenig",
    "wenn",
    "wer",
    "werden",
    "wie",
    "wieder",
    "will",
    "wir",
    "wird",
    "wo",
    "wohl",
    "wollen",
    "wollte",
    "wurde",
    "wurden",
    "würde",
    "würden",
    "zu",
    "zum",
    "zur",
    "zwar",
    "zwischen",
];

/// Passt ein Diktat an den Text um die Schreibmarke an.
///
/// Leerzeichen zum Nachbartext links und rechts kommen in jedem Stil dazu,
/// außer in Sprachen ohne Leerzeichen (Chinesisch, Japanisch).
#[must_use]
pub(crate) fn fit(
    text: &str,
    surroundings: &Surroundings,
    language: Language,
    style: AppStyle,
) -> String {
    // Nur Leerzeichen kappen: Ein Umbruch am Rand kommt aus einem
    // Sprachbefehl („neue Zeile“).
    let text = text.trim_matches(BLANKS);
    if text.is_empty() {
        return String::new();
    }
    let body = match style {
        AppStyle::Sentences => text.to_owned(),
        AppStyle::Adaptive => adapted(text, surroundings, language),
        AppStyle::Chat => without_final_period(&adapted(text, surroundings, language)).to_owned(),
    };
    let spaced = !language.without_spaces();
    let space_before =
        spaced && !body.starts_with('\n') && needs_space_before(&surroundings.before);
    let space_after = spaced && !body.ends_with('\n') && needs_space_after(&surroundings.after);
    let mut fitted = String::with_capacity(body.len().saturating_add(2));
    if space_before {
        fitted.push(' ');
    }
    fitted.push_str(&body);
    if space_after {
        fitted.push(' ');
    }
    fitted
}

/// Mitten im Satz klein, am Ende an den Text rechts angepasst.
fn adapted(text: &str, surroundings: &Surroundings, language: Language) -> String {
    let body = if continues_sentence(&surroundings.before) {
        lowercase_first_word(text, language)
    } else {
        text.to_owned()
    };
    fit_end(&body, &surroundings.after)
}

/// Klebt rechts direkt ein Wort an der Schreibmarke?
fn needs_space_after(after: &str) -> bool {
    after.chars().next().is_some_and(char::is_alphanumeric)
}

/// Ohne einen einzelnen Schlusspunkt; „…“, „...“, „?“, „!“ und der Punkt
/// einer Abkürzung („usw.“) bleiben.
fn without_final_period(text: &str) -> &str {
    match text.strip_suffix('.') {
        Some(rest) if !rest.ends_with('.') && !words::ends_with_abbreviation(text) => rest,
        Some(_) | None => text,
    }
}

/// Steht links Text ohne Satzende (also mitten in einem Satz)? Nach einem
/// öffnenden Anführungszeichen beginnt wörtliche Rede, also nicht; nach
/// einem schließenden („Hallo“) zählt, was davor steht.
fn continues_sentence(before: &str) -> bool {
    // Ohne Umbruch ist alles die letzte Zeile.
    let line = before.rsplit_once('\n').map_or(before, |(_, line)| line);
    let trimmed = line.trim_end_matches(BLANKS);
    if ends_with_opening_quote(trimmed) {
        return false;
    }
    trimmed
        .trim_end_matches(|c: char| {
            words::is_quote(c) || words::is_opening_bracket(c) || matches!(c, ')' | ']')
        })
        .trim_end_matches(BLANKS)
        .chars()
        .next_back()
        .is_some_and(|last| !CAPITAL_AFTER.contains(&last))
}

/// Endet `text` mit einem öffnenden Anführungszeichen („Er sagte „“)?
fn ends_with_opening_quote(text: &str) -> bool {
    let mut chars = text.chars().rev();
    chars
        .next()
        .is_some_and(|last| words::is_quote(last) && words::quote_opens(last, chars.next()))
}

/// Klebt der Text links direkt an der Schreibmarke? Nach einer öffnenden
/// Klammer oder einem öffnenden Anführungszeichen kommt kein Leerzeichen,
/// nach einem schließenden („Hallo“) schon.
fn needs_space_before(before: &str) -> bool {
    let Some(last) = before.chars().next_back() else {
        return false;
    };
    !last.is_whitespace() && !words::is_opening_bracket(last) && !ends_with_opening_quote(before)
}

/// Schließt `next`, das rechts direkt am Diktat steht, etwas ab (ein
/// Satzzeichen oder ein schließendes Anführungszeichen)?
fn closes_right(next: char) -> bool {
    PUNCTUATION.contains(&next) || (words::is_quote(next) && !words::quote_opens(next, Some('a')))
}

/// Schreibt das erste Wort klein, wo das unbedenklich ist.
fn lowercase_first_word(text: &str, language: Language) -> String {
    let word_end = text
        .find(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’')
        .unwrap_or(text.len());
    let (word, rest) = text.split_at(word_end);
    // Deutsch und Englisch: nur Funktionswörter (Nomen und Namen bleiben
    // groß). Für andere Sprachen gibt es keine solche Liste; dort bleibt
    // das erste Wort, wie das Modell es schreibt, damit Namen groß bleiben
    // („Je vais à Paris.“).
    let lower = word.to_lowercase();
    let allowed = match language {
        Language::German => GERMAN_LOWERCASE.contains(&lower.as_str()),
        Language::English => ENGLISH_LOWERCASE.contains(&lower.as_str()),
        Language::French
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
        | Language::Korean => false,
    };
    if !allowed {
        return text.to_owned();
    }
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return text.to_owned();
    };
    first
        .to_lowercase()
        .chain(chars)
        .chain(rest.chars())
        .collect()
}

/// Passt das Ende an den Text rechts an.
fn fit_end(text: &str, after: &str) -> String {
    let Some(next) = after.chars().next() else {
        return text.to_owned();
    };
    if next == '\n' {
        return text.to_owned();
    }
    if closes_right(next) {
        // Das Satzzeichen steht rechts schon.
        return without_closing(text).to_owned();
    }
    if next.is_whitespace() {
        let following = after.trim_start_matches(BLANKS).chars().next();
        return match following {
            Some(letter) if letter.is_lowercase() => without_closing(text).to_owned(),
            Some(_) | None => text.to_owned(),
        };
    }
    if !next.is_alphanumeric() {
        // Ein Zeichen wie „─“ oder „•“: kein Wort, das den Satz fortsetzt.
        return text.to_owned();
    }
    // Rechts klebt ein Wort direkt an (das Leerzeichen davor setzt `fit`):
    // Ist es großgeschrieben, beginnt dort ein neuer Satz wie nach einem
    // Leerzeichen, sonst geht der Satz weiter.
    if next.is_uppercase() {
        text.to_owned()
    } else {
        without_closing(text).to_owned()
    }
}

/// Ohne Satzzeichen am Ende, außer dem Punkt einer Abkürzung.
fn without_closing(text: &str) -> &str {
    if words::ends_with_abbreviation(text) {
        text
    } else {
        text.trim_end_matches(CLOSING)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn around(before: &str, after: &str) -> Surroundings {
        Surroundings {
            before: before.to_owned(),
            after: after.to_owned(),
        }
    }

    fn german(text: &str, before: &str, after: &str) -> String {
        fit(
            text,
            &around(before, after),
            Language::German,
            AppStyle::Adaptive,
        )
    }

    fn english(text: &str, before: &str, after: &str) -> String {
        fit(
            text,
            &around(before, after),
            Language::English,
            AppStyle::Adaptive,
        )
    }

    #[test]
    fn empty_field_keeps_sentence() {
        assert_eq!(german("Nach Hause.", "", ""), "Nach Hause.");
    }

    #[test]
    fn new_sentence_after_sentence_end() {
        assert_eq!(german("Das ist gut.", "Hallo.", ""), " Das ist gut.");
        assert_eq!(german("Das ist gut.", "Hallo. ", ""), "Das ist gut.");
        assert_eq!(
            german("Das ist gut.", "Erster Absatz.\n", ""),
            "Das ist gut."
        );
        assert_eq!(german("Das ist gut.", "Frage? ", ""), "Das ist gut.");
    }

    #[test]
    fn continues_sentence_in_lowercase() {
        assert_eq!(german("Nach Hause.", "Ich gehe heute ", ""), "nach Hause.");
        assert_eq!(german("Nach Hause.", "Ich gehe heute", ""), " nach Hause.");
        assert_eq!(
            german("Weil es regnet.", "Ich bleibe, ", ""),
            "weil es regnet."
        );
        assert_eq!(
            english("The car is red.", "I think ", ""),
            "the car is red."
        );
    }

    #[test]
    fn keeps_nouns_and_names_capitalized() {
        assert_eq!(
            german("Vertrag ist wirksam.", "Der ", ""),
            "Vertrag ist wirksam."
        );
        assert_eq!(
            german("Sie haben recht.", "Ich glaube, ", ""),
            "Sie haben recht."
        );
        assert_eq!(english("I agree.", "Well, ", ""), "I agree.");
        assert_eq!(english("NATO said so.", "And ", ""), "NATO said so.");
        assert_eq!(english("McDonald won.", "And ", ""), "McDonald won.");
        assert_eq!(english("John yesterday.", "I met ", ""), "John yesterday.");
    }

    #[test]
    fn other_languages_keep_the_first_word() {
        for (text, before, language) in [
            ("Paris.", "Je vais à ", Language::French),
            ("Marie demain.", "Je vais voir ", Language::French),
            ("I ragazzi giocano.", "Oggi ", Language::Italian),
        ] {
            assert_eq!(
                fit(text, &around(before, ""), language, AppStyle::Adaptive),
                text
            );
        }
    }

    #[test]
    fn keeps_the_period_of_an_abbreviation() {
        assert_eq!(
            german("Äpfel, Birnen usw.", "", " und Kiwis"),
            "Äpfel, Birnen usw."
        );
        assert_eq!(
            german("Zum Beispiel z. B.", "", " hier"),
            "Zum Beispiel z. B."
        );
        assert_eq!(german("Das ist gut.", "", " und"), "Das ist gut");
    }

    #[test]
    fn a_digit_is_no_abbreviation() {
        assert_eq!(german("Um 10.", "Ich komme ", " und bleibe"), "um 10");
        assert_eq!(german("Um 8.", "Ich komme ", " und bleibe"), "um 8");
        assert_eq!(german("Aus § 823 I.", "Das folgt ", " und"), "aus § 823 I");
    }

    #[test]
    fn chat_keeps_the_period_of_an_abbreviation() {
        let chat = |text: &str| fit(text, &around("", ""), Language::German, AppStyle::Chat);
        assert_eq!(chat("Bring Äpfel, Birnen usw."), "Bring Äpfel, Birnen usw.");
        assert_eq!(chat("Bring Äpfel, Birnen."), "Bring Äpfel, Birnen");
    }

    #[test]
    fn no_space_after_an_opening_quote() {
        assert_eq!(german("Der Vertrag.", "Er sagte »", ""), "Der Vertrag.");
        assert_eq!(german("Der Vertrag.", "Er sagte ‚", ""), "Der Vertrag.");
    }

    #[test]
    fn drops_period_when_sentence_goes_on() {
        assert_eq!(
            german("Nach Hause.", "Ich gehe ", " und dann"),
            "nach Hause"
        );
        assert_eq!(
            german("Nach Hause.", "Ich gehe ", ", und dann"),
            "nach Hause"
        );
        assert_eq!(
            german("Nach Hause.", "Ich gehe ", "und dann"),
            "nach Hause "
        );
        assert_eq!(german("Heute.", "Ich gehe ", "."), "heute");
        assert_eq!(german("Warum?", "Ich frage ", " und"), "warum?");
    }

    #[test]
    fn keeps_period_before_new_sentence_or_line() {
        assert_eq!(
            german("Nach Hause.", "Ich gehe ", " Dann essen"),
            "nach Hause."
        );
        assert_eq!(
            german("Nach Hause.", "Ich gehe ", "\nZweite Zeile"),
            "nach Hause."
        );
        assert_eq!(german("Nach Hause.", "Ich gehe ", "────"), "nach Hause.");
    }

    #[test]
    fn no_space_after_opening_bracket_or_quote() {
        assert_eq!(german("Siehe oben.", "Text (", ")"), "Siehe oben");
        assert_eq!(german("Der Vertrag.", "Er sagte „", "“"), "Der Vertrag");
        assert_eq!(english("The deal.", "He said “", "”"), "The deal");
    }

    #[test]
    fn a_german_closing_quote_continues_the_sentence() {
        assert_eq!(german("Und ging.", "Er sagte „Hallo“ ", ""), "und ging.");
        assert_eq!(german("Und ging.", "Er sagte „Hallo“", ""), " und ging.");
        assert_eq!(
            german("Dann ging er.", "Er sagte „Hallo.“ ", ""),
            "Dann ging er."
        );
        assert_eq!(english("And left.", "He said “Hello” ", ""), "and left.");
    }

    #[test]
    fn a_sentence_before_a_glued_capitalised_word_keeps_its_period() {
        assert_eq!(german("Danke.", "", " Ich komme morgen."), "Danke.");
        assert_eq!(german("Danke.", "", "Ich komme morgen."), "Danke. ");
        assert_eq!(
            fit(
                "Danke.",
                &around("", "Ich komme morgen."),
                Language::German,
                AppStyle::Sentences
            ),
            "Danke. "
        );
    }

    #[test]
    fn no_spaces_between_chinese_or_japanese_texts() {
        for (text, before, after, language) in [
            ("你好。", "", "世界", Language::Chinese),
            ("你好。", "世界", "", Language::Chinese),
            ("こんにちは。", "", "世界", Language::Japanese),
        ] {
            for style in [AppStyle::Adaptive, AppStyle::Sentences, AppStyle::Chat] {
                let fitted = fit(text, &around(before, after), language, style);
                assert!(!fitted.contains(' '), "kein Leerzeichen: {fitted:?}");
            }
        }
    }

    #[test]
    fn follows_the_style_of_the_app() {
        assert_eq!(AppStyle::of("com.apple.mail"), AppStyle::Sentences);
        assert_eq!(AppStyle::of("com.apple.MobileSMS"), AppStyle::Chat);
        assert_eq!(AppStyle::of("com.apple.Notes"), AppStyle::Adaptive);
        let chat = |text: &str, before: &str| {
            fit(text, &around(before, ""), Language::German, AppStyle::Chat)
        };
        assert_eq!(chat("Bin gleich da.", ""), "Bin gleich da");
        assert_eq!(chat("Wirklich?", ""), "Wirklich?");
        assert_eq!(chat("Na ja...", ""), "Na ja...");
        assert_eq!(chat("Und dann los.", "Ich komme"), " und dann los");
        assert_eq!(
            fit(
                "Und dann los.",
                &around("Ich komme. ", "Heute"),
                Language::German,
                AppStyle::Sentences
            ),
            "Und dann los. "
        );
        // Mail: vollständiger Satz, auch mitten in einem Satz.
        assert_eq!(
            fit(
                "Und dann los.",
                &around("Ich komme", " heute"),
                Language::German,
                AppStyle::Sentences
            ),
            " Und dann los."
        );
    }
}
