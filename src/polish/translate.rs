//! Lange Transkripte übersetzen, Absatz für Absatz.
//!
//! Zeitmarken (`[12:30]`) und Kapitelüberschriften (`## …`) bekommt das
//! Sprachmodell nicht zu sehen: Jeder Absatz wird ohne seine Zeitmarke
//! übersetzt und danach wieder hinter dieselbe gesetzt. So bleibt jede Marke
//! vor dem Absatz, zu dem sie gehört, und das Modell kann sie weder
//! verschieben noch verändern. Kleine Stücke passen außerdem in das
//! begrenzte Kontextfenster von Apples Modell.

/// Ein Stück des Transkripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Unit {
    /// Kapitelüberschrift ohne `## `.
    Heading(String),
    /// Ein Absatz, mit der Zeitmarke davor (falls eingeschaltet).
    Paragraph {
        /// `[12:30]`, falls vorhanden.
        stamp: Option<String>,
        /// Der Text dahinter.
        body: String,
    },
}

impl Unit {
    /// Der Text, der übersetzt wird.
    pub(super) fn text(&self) -> &str {
        match self {
            Self::Heading(text)
            | Self::Paragraph {
                body: text,
                stamp: _,
            } => text,
        }
    }

    /// Dasselbe Stück mit anderem Text, Zeitmarke bzw. `## ` bleiben.
    pub(super) fn with_text(&self, text: String) -> Self {
        match self {
            Self::Heading(_) => Self::Heading(text),
            Self::Paragraph { stamp, body: _ } => Self::Paragraph {
                stamp: stamp.clone(),
                body: text,
            },
        }
    }

    /// Das Stück, wie es im Transkript steht.
    fn render(&self) -> String {
        match self {
            Self::Heading(text) => format!("## {text}"),
            Self::Paragraph {
                stamp: Some(stamp),
                body,
            } => format!("{stamp} {body}"),
            Self::Paragraph { stamp: None, body } => body.clone(),
        }
    }
}

/// Zerlegt ein Transkript in seine Absätze (getrennt durch Leerzeilen).
pub(super) fn split(transcript: &str) -> Vec<Unit> {
    transcript
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(|paragraph| {
            if let Some(title) = paragraph.strip_prefix("## ") {
                return Unit::Heading(title.trim().to_owned());
            }
            match stamp_of(paragraph) {
                Some((stamp, body)) => Unit::Paragraph {
                    stamp: Some(stamp.to_owned()),
                    body: body.trim().to_owned(),
                },
                None => Unit::Paragraph {
                    stamp: None,
                    body: paragraph.to_owned(),
                },
            }
        })
        .collect()
}

/// Setzt die Stücke wieder zu einem Transkript zusammen.
pub(super) fn join(units: &[Unit]) -> String {
    units
        .iter()
        .map(Unit::render)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Trennt eine Zeitmarke (`[12:30]`, `[1:02:05]`) vom Rest des Absatzes.
fn stamp_of(paragraph: &str) -> Option<(&str, &str)> {
    let end = paragraph.find(']')?;
    let (stamp, rest) = paragraph.split_at(end.checked_add(1)?);
    let inner = stamp.strip_prefix('[')?.strip_suffix(']')?;
    let is_time = !inner.is_empty()
        && inner.contains(':')
        && inner
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b':');
    is_time.then_some((stamp, rest))
}

/// Anweisung zum Übersetzen von `from` nach `to` (englische Namen).
///
/// Auf Englisch wie beim Überarbeiten: Apples kleines Modell folgt so am
/// zuverlässigsten.
pub(super) fn instructions(from: &str, to: &str) -> String {
    format!(
        "You translate a passage of a transcribed video from {from} into {to}. Translate \
faithfully and completely, keep the meaning and the tone, and do not summarize, shorten, \
explain or add anything. Keep names, numbers and citations like \"§ 433 II BGB\" unchanged. \
Reply with the {to} translation only."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_stamps_and_headings_out_of_the_text() {
        let transcript =
            "## Einführung\n\n[0:05] Hola a todos.\n\n[1:02:05] Seguimos.\n\nSin marca.";
        let units = split(transcript);
        assert_eq!(
            units,
            [
                Unit::Heading("Einführung".to_owned()),
                Unit::Paragraph {
                    stamp: Some("[0:05]".to_owned()),
                    body: "Hola a todos.".to_owned()
                },
                Unit::Paragraph {
                    stamp: Some("[1:02:05]".to_owned()),
                    body: "Seguimos.".to_owned()
                },
                Unit::Paragraph {
                    stamp: None,
                    body: "Sin marca.".to_owned()
                },
            ]
        );
        assert_eq!(join(&units), transcript);
    }

    #[test]
    fn translated_text_lands_behind_the_same_stamp() {
        let units = split("## Introducción\n\n[12:30] Hola.");
        let translated: Vec<Unit> = units
            .iter()
            .zip(["Einführung", "Hallo."])
            .map(|(unit, text)| unit.with_text(text.to_owned()))
            .collect();
        assert_eq!(join(&translated), "## Einführung\n\n[12:30] Hallo.");
    }

    #[test]
    fn brackets_that_are_no_time_stay_in_the_text() {
        for paragraph in ["[Musik] Hola.", "[] Hola.", "[12] Hola.", "Hola [0:05]."] {
            assert_eq!(
                split(paragraph),
                [Unit::Paragraph {
                    stamp: None,
                    body: paragraph.to_owned()
                }],
                "{paragraph}"
            );
        }
    }

    #[test]
    fn instructions_name_both_languages() {
        let text = instructions("Spanish", "German");
        assert!(
            text.contains("from Spanish into German"),
            "die Anweisung nennt Ausgangs- und Zielsprache"
        );
        assert!(
            text.ends_with("Reply with the German translation only."),
            "die Anweisung endet mit der Bitte um nur die Übersetzung"
        );
    }
}
