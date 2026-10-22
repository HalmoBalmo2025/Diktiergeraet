//! Tests der Normzitate.

use super::*;

fn roman(text: &str) -> String {
    format(text, CitationStyle::Roman, &Statutes::default())
}

fn abbreviated(text: &str) -> String {
    format(text, CitationStyle::Abbreviated, &Statutes::default())
}

/// Was Cohere aus gesprochenen Testsätzen gemacht hat.
#[test]
fn formats_model_output() {
    let cases = [
        (
            "Der Anspruch ergibt sich aus § 823 Absatz 1 BGB.",
            "Der Anspruch ergibt sich aus § 823 I BGB.",
        ),
        (
            "Nach 433 Absatz 2 Satz 1 BGB ist der Käufer verpflichtet.",
            "Nach § 433 II 1 BGB ist der Käufer verpflichtet.",
        ),
        (
            "Hier greift Artikel 2 Absatz 1 in Verbindung mit Artikel 1 Absatz 1 Grundgesetz.",
            "Hier greift Art. 2 I i.V.m. Art. 1 I GG.",
        ),
        (
            "Das folgt aus Paragraph 280 Absatz 1 Absatz 3 Paragraph 281 BGB.",
            "Das folgt aus § 280 I, III, § 281 BGB.",
        ),
        (
            "Gemäß 812 Absatz 1 Satz 1 Erste Alternative BGB besteht ein Anspruch.",
            "Gemäß § 812 I 1 Alt. 1 BGB besteht ein Anspruch.",
        ),
        (
            "Paragraph 242 St. G. B., Paragraph 263 St. G. B. und Paragraph 223 St. G. B. kommen in Betracht.",
            "§ 242 StGB, § 263 StGB und § 223 StGB kommen in Betracht.",
        ),
        (
            "Siehe 7 Absatz 1 Nummer 2 VWGO und 3-5 ZPO.",
            "Siehe § 7 I Nr. 2 VwGO und §§ 3-5 ZPO.",
        ),
        ("1004 Absatz 1 Satz 2 Analog BGB.", "§ 1004 I 2 analog BGB."),
    ];
    for (input, expected) in cases {
        assert_eq!(roman(input), expected, "{input}");
    }
}

/// Aus dem Transkript einer BGB-AT-Vorlesung.
#[test]
fn formats_lecture_transcript() {
    let cases = [
        ("aus 433 Absatz 2 hat", "aus § 433 II hat"),
        (
            "zitierst du auch hier 145 und 147 BGB, denn",
            "zitierst du auch hier §§ 145 und 147 BGB, denn",
        ),
        ("in 119 Absatz 1 Fall 1", "in § 119 I Fall 1"),
        (
            "Das steht dann in 179 Absatz 2 und 3 drin",
            "Das steht dann in § 179 II und III drin",
        ),
        (
            "die Gesellschafter 124 HGB oder",
            "die Gesellschafter § 124 HGB oder",
        ),
        ("Paragraf fünfzehn des Handelsgesetzbuches", "§ 15 HGB"),
        (
            "die Paragraphen 177 und 179 BGB auflösen",
            "die §§ 177 und 179 BGB auflösen",
        ),
        (
            "107 Fall 1, 107 Fall 2, 110 BGB und 108 BGB",
            "§§ 107 Fall 1, 107 Fall 2, 110 BGB und § 108 BGB",
        ),
        (
            "aus 433 Absatz 2 in Verbindung mit dem Kaufvertrag",
            "aus § 433 II in Verbindung mit dem Kaufvertrag",
        ),
        ("(Paragraf 130 Absatz 1 Satz 2).", "(§ 130 I 2)."),
    ];
    for (input, expected) in cases {
        assert_eq!(roman(input), expected, "{input}");
    }
}

#[test]
fn leaves_ordinary_numbers_alone() {
    for text in [
        "Wir werden jetzt in 60 bis 90 Minuten die Basics wiederholen.",
        "sofort 18 Punkte schreiben",
        "für 50.000 Euro kaufen",
        "Los geht's mit Thema 1, dem Vertragsschluss.",
        "Dritter Anfechtungsgrund in 119. Hier",
        "Das ist der 3. Fall dann.",
        "Die Art und Weise, der alte Weg.",
        "433 Absatz 2 und 50 Euro",
    ] {
        let expected = match text {
            "433 Absatz 2 und 50 Euro" => "§ 433 II und 50 Euro",
            other => other,
        };
        assert_eq!(roman(text), expected, "{text}");
    }
}

#[test]
fn formats_more_parts() {
    let cases = [
        (
            "Paragraf 3 Nummer 2 Buchstabe b UWG",
            "§ 3 Nr. 2 lit. b UWG",
        ),
        (
            "Paragraf 32 Absatz 2 zweiter Halbsatz StGB",
            "§ 32 II Hs. 2 StGB",
        ),
        ("Paragraf 194 folgende BGB", "§ 194 ff. BGB"),
        ("§ 42 Absatz 1 Variante 2 VwGO", "§ 42 I Var. 2 VwGO"),
        (
            "Paragraf 812 Absatz 1 Satz 1 1. Alternative",
            "§ 812 I 1 Alt. 1",
        ),
        ("Paragraf 123 Satz 2 BGB", "§ 123 S. 2 BGB"),
        (
            "Paragraf 433 BGB in Verbindung mit Paragraf 434 BGB",
            "§ 433 BGB i.V.m. § 434 BGB",
        ),
        (
            "Artikel 2 Absatz 1 i.V.m. 1 Absatz 1 GG",
            "Art. 2 I i.V.m. 1 I GG",
        ),
        ("5 Absatz 1 GG", "Art. 5 I GG"),
        ("§ 823 Absatz eins Satz zwei BGB", "§ 823 I 2 BGB"),
        ("Paragraf 90a BGB", "§ 90a BGB"),
    ];
    for (input, expected) in cases {
        assert_eq!(roman(input), expected, "{input}");
    }
}

#[test]
fn writes_abbreviated_style() {
    let cases = [
        ("Nach 433 Absatz 2 Satz 1 BGB", "Nach § 433 Abs. 2 S. 1 BGB"),
        (
            "Artikel 2 Absatz 1 in Verbindung mit Artikel 1 Absatz 1 Grundgesetz",
            "Art. 2 Abs. 1 i.V.m. Art. 1 Abs. 1 GG",
        ),
        ("179 Absatz 2 und 3 BGB", "§ 179 Abs. 2 und 3 BGB"),
    ];
    for (input, expected) in cases {
        assert_eq!(abbreviated(input), expected, "{input}");
    }
}

#[test]
fn uses_extra_statutes() {
    let statutes = Statutes::new(&["BayPAG".to_owned()]);
    assert_eq!(
        format(
            "Artikel 11 Absatz 1 BayPAG",
            CitationStyle::Roman,
            &statutes
        ),
        "Art. 11 I BayPAG"
    );
    assert_eq!(
        format("11 Absatz 1 baypag", CitationStyle::Roman, &statutes),
        "Art. 11 I BayPAG"
    );
}

#[test]
fn a_bare_number_needs_a_real_statute_or_norm_part() {
    let statutes = Statutes::default();
    let cite = |text: &str| format(text, CitationStyle::Roman, &statutes);
    assert_eq!(cite("Ich muss um 5 weg."), "Ich muss um 5 weg.");
    assert_eq!(cite("Die ersten 10 weg, dann"), "Die ersten 10 weg, dann");
    assert_eq!(cite("nach 10 WEG"), "nach § 10 WEG");
    assert_eq!(cite("nach 10 Absatz 1 weg"), "nach § 10 I WEG");
    assert_eq!(cite("Paragraf 10 weg"), "§ 10 WEG");
    assert_eq!(
        cite("Lies Seite 5 Absatz 3 im Skript."),
        "Lies Seite 5 Absatz 3 im Skript."
    );
    assert_eq!(cite("Siehe Folie 12 Nummer 3."), "Siehe Folie 12 Nummer 3.");
    assert_eq!(cite("Siehe 12 Nummer 3 BGB."), "Siehe § 12 Nr. 3 BGB.");
}

#[test]
fn spelled_statutes_are_capitalised() {
    assert_eq!(roman("Sie ist a. o. Mitglied."), "Sie ist a. o. Mitglied.");
    assert_eq!(
        roman("Vgl. Müller, a. a. O., S. 12."),
        "Vgl. Müller, a. a. O., S. 12."
    );
    assert_eq!(roman("Paragraf 242 St. G. B."), "§ 242 StGB.");
}

#[test]
fn reads_an_abbreviation_after_an_article() {
    assert_eq!(roman("Paragraf 15 des HGB"), "§ 15 HGB");
    assert_eq!(roman("nach 433 des BGB gilt"), "nach § 433 BGB gilt");
    assert_eq!(roman("um 5 der weg"), "um 5 der weg");
}

#[test]
fn a_sentence_end_ends_the_citation() {
    let cases = [
        (
            "Maßgeblich ist § 433 Absatz 1. Absatz 2 regelt die Pflichten des Käufers.",
            "Maßgeblich ist § 433 I. Absatz 2 regelt die Pflichten des Käufers.",
        ),
        (
            "Paragraf 433 Absatz 1.\n\nAbsatz 2 regelt die Pflichten des Käufers.",
            "§ 433 I.\n\nAbsatz 2 regelt die Pflichten des Käufers.",
        ),
        (
            "Das geht nur auf diese Art. Drei Dinge sind wichtig.",
            "Das geht nur auf diese Art. Drei Dinge sind wichtig.",
        ),
        (
            "Nach § 179 Absatz 2. Und 3 Leute kamen.",
            "Nach § 179 II. Und 3 Leute kamen.",
        ),
        (
            "Nach Paragraf 433. Absatz 2 regelt das.",
            "Nach § 433. Absatz 2 regelt das.",
        ),
        (
            "Paragraf 433 BGB. In Verbindung mit Artikel 1 GG gilt.",
            "§ 433 BGB. In Verbindung mit Art. 1 GG gilt.",
        ),
        ("Paragraf 194 ff. BGB", "§ 194 ff. BGB"),
        (
            "Das folgt aus § 433 Absatz 2. Satz 2 gilt entsprechend.",
            "Das folgt aus § 433 II. Satz 2 gilt entsprechend.",
        ),
        (
            "Das ist so eine Art. Drei Leute kamen.",
            "Das ist so eine Art. Drei Leute kamen.",
        ),
        ("Paragraf 433\nParagraf 434 BGB", "§ 433\n§ 434 BGB"),
    ];
    for (input, expected) in cases {
        assert_eq!(roman(input), expected, "{input}");
    }
}

#[test]
fn bare_number_after_ivm_needs_a_norm_part_or_statute() {
    assert_eq!(
        roman("Paragraf 433 BGB in Verbindung mit 3 Zeugen"),
        "§ 433 BGB in Verbindung mit 3 Zeugen"
    );
    assert_eq!(
        roman("Paragraf 433 BGB in Verbindung mit 10 weg"),
        "§ 433 BGB in Verbindung mit 10 weg"
    );
}

#[test]
fn each_norm_after_ivm_keeps_its_own_sign() {
    let cases = [
        (
            "aus 823 Absatz 1 BGB in Verbindung mit Artikel 2 Absatz 1 GG",
            "aus § 823 I BGB i.V.m. Art. 2 I GG",
        ),
        (
            "§ 433 BGB in Verbindung mit Paragraf 434 und 435 BGB",
            "§ 433 BGB i.V.m. §§ 434 und 435 BGB",
        ),
        (
            "2 Absatz 1 in Verbindung mit 1 Absatz 1 GG",
            "Art. 2 I i.V.m. 1 I GG",
        ),
        (
            "145 und 147 BGB in Verbindung mit Artikel 1 GG",
            "§§ 145 und 147 BGB i.V.m. Art. 1 GG",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(roman(input), expected, "{input}");
    }
}

#[test]
fn a_bare_number_after_a_counting_word_is_no_norm() {
    let cases = [
        (
            "Nach Absatz 2 Satz 1 muss der Käufer zahlen.",
            "Nach Absatz 2 Satz 1 muss der Käufer zahlen.",
        ),
        (
            "Lies Abschnitt 3 Satz 2 im Skript.",
            "Lies Abschnitt 3 Satz 2 im Skript.",
        ),
        ("Siehe Fußnote 3 Satz 2.", "Siehe Fußnote 3 Satz 2."),
        (
            "die Gesellschafter 124 HGB oder",
            "die Gesellschafter § 124 HGB oder",
        ),
        ("Prüfen Sie 433 Absatz 2.", "Prüfen Sie § 433 II."),
        (
            "die Reform 2002 des Bürgerlichen Gesetzbuchs",
            "die Reform 2002 des Bürgerlichen Gesetzbuchs",
        ),
        ("Im Jahr 2002 BGB-Reform.", "Im Jahr 2002 BGB-Reform."),
        ("nach 1922 Absatz 1 BGB", "nach § 1922 I BGB"),
        ("nach 1004 BGB", "nach § 1004 BGB"),
    ];
    for (input, expected) in cases {
        assert_eq!(roman(input), expected, "{input}");
    }
}

#[test]
fn a_second_part_after_und_continues_the_citation() {
    assert_eq!(
        roman("§ 433 Absatz 2 Satz 1 und Absatz 3 Satz 2 BGB"),
        "§ 433 II 1 und III 2 BGB"
    );
    assert_eq!(
        abbreviated("§ 433 Absatz 2 und Absatz 3 BGB"),
        "§ 433 Abs. 2 und Abs. 3 BGB"
    );
}

#[test]
fn a_number_before_a_noun_is_no_further_part() {
    let cases = [
        (
            "nach § 199 Absatz 1, 3 Jahre ab Kenntnis",
            "nach § 199 I, 3 Jahre ab Kenntnis",
        ),
        (
            "nach § 199 Absatz 1 und 3 Jahre später",
            "nach § 199 I und 3 Jahre später",
        ),
        (
            "Das steht dann in 179 Absatz 2 und 3 BGB",
            "Das steht dann in § 179 II und III BGB",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(roman(input), expected, "{input}");
    }
}

#[test]
fn formatted_citations_stay_as_they_are() {
    for text in [
        "Siehe § 7 I Nr. 2 VwGO.",
        "Gemäß § 812 I 1 Alt. 1 BGB.",
        "Hier greift Art. 2 I i.V.m. Art. 1 I GG.",
        "Das folgt aus § 280 I, III, § 281 BGB.",
        "Das steht dann in § 179 II und III drin",
        "Art. 2 I i.V.m. 1 I GG",
        "§§ 107 Fall 1, 107 Fall 2, 110 BGB und § 108 BGB",
    ] {
        assert_eq!(roman(text), text);
    }
    for text in [
        "Die Batterie hat 9 V.",
        "Sie kam um 5 I.",
        "nach § 433 II 3 Tage",
    ] {
        assert_eq!(roman(text), text);
    }
    assert_eq!(
        abbreviated("Nach § 433 II 1 BGB"),
        "Nach § 433 Abs. 2 S. 1 BGB"
    );
}

#[test]
fn bavarian_laws_count_articles() {
    let statutes = Statutes::default();
    assert_eq!(
        format("Nach 55 Absatz 1 BayBO", CitationStyle::Roman, &statutes),
        "Nach Art. 55 I BayBO"
    );
    assert_eq!(
        format("Paragraf 11 PAG", CitationStyle::Roman, &statutes),
        "§ 11 PAG"
    );
}

/// Eigenschaftstests: zufällige Texte aus Wörtern, wie sie in Zitaten
/// und drumherum vorkommen.
mod properties {
    use super::*;
    use proptest::prelude::*;

    /// Wörter, aus denen die Texte bestehen.
    const TOKENS: &[&str] = &[
        "Paragraf",
        "§",
        "§§",
        "Artikel",
        "Art.",
        "Art",
        "Absatz",
        "Abs.",
        "Satz",
        "S.",
        "Nummer",
        "Nr.",
        "Buchstabe",
        "lit.",
        "a",
        "b",
        "erste",
        "Alternative",
        "Alt.",
        "Halbsatz",
        "Fall",
        "analog",
        "ff.",
        "folgende",
        "und",
        "oder",
        "bis",
        "in",
        "Verbindung",
        "mit",
        "i.V.m.",
        "BGB",
        "bgb",
        "GG",
        "StGB",
        "St.",
        "G.",
        "B.",
        "des",
        "Handelsgesetzbuches",
        "HGB",
        "BayBO",
        "weg",
        "WEG",
        "1",
        "2",
        "3",
        "12",
        "25",
        "433",
        "823",
        "3-5",
        "2002",
        "fünf",
        "zwei",
        "I",
        "II",
        "IV",
        "Seite",
        "Euro",
        "Die",
        "der",
        "Käufer",
        "Jahre",
        "Sie",
        "nach",
        "Nach",
        "(",
        "Abschnitt",
    ];

    /// Was zwischen zwei Wörtern steht.
    const GAPS: &[&str] = &[" ", " ", " ", ", ", ". ", "\n", "! ", ") "];

    /// Ein Text aus bis zu 14 Wörtern.
    fn text() -> impl Strategy<Value = String> {
        prop::collection::vec(
            (prop::sample::select(TOKENS), prop::sample::select(GAPS)),
            0..14,
        )
        .prop_map(|parts| {
            parts
                .into_iter()
                .flat_map(|(word, gap)| [word, gap])
                .collect()
        })
    }

    proptest! {
        /// Was die Regeln geschrieben haben, bleiben sie beim zweiten
        /// Mal so, wie es ist.
        #[test]
        fn format_is_idempotent(text in text()) {
            for style in [CitationStyle::Roman, CitationStyle::Abbreviated] {
                let statutes = Statutes::default();
                let once = format(&text, style, &statutes);
                prop_assert_eq!(format(&once, style, &statutes), once);
            }
        }
    }
}
