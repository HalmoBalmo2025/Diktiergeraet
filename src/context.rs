//! Die App vorn und der Text um ihre Schreibmarke – über das Hilfsprogramm
//! `diktator-context` (Quelltext in `helpers/context.swift`).

use std::time::Duration;

use crate::helper::{self, Helper, HelperError};
use crate::text::Surroundings;

/// Länger wird nicht gewartet; dann wird eingefügt wie ohne Hilfsprogramm.
const TIMEOUT: Duration = Duration::from_millis(800);

/// Was das Hilfsprogramm über die Stelle weiß, an der eingefügt wird.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cursor {
    /// Bundle-ID der App vorn; `None`, wenn macOS sie nicht nennt.
    pub app: Option<String>,
    /// Der Text um die Schreibmarke, soweit lesbar.
    pub text: CursorText,
}

/// Der Text um die Schreibmarke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CursorText {
    /// Gelesen.
    Known(Surroundings),
    /// Nicht lesbar (kein Textfeld, Terminal, die App verrät ihn nicht);
    /// der Grund kommt vom Hilfsprogramm.
    Unknown {
        /// Warum nicht.
        reason: String,
    },
}

/// Liest die App vorn und den Text links und rechts ihrer Schreibmarke.
///
/// # Errors
/// [`HelperError`], wenn das Hilfsprogramm fehlt, nicht rechtzeitig
/// antwortet oder nicht einmal die App kennt.
pub(crate) fn read() -> Result<Cursor, HelperError> {
    let output = helper::run(Helper::Context, &[], "", TIMEOUT)?;
    Ok(parse(&output))
}

/// „app\0links\0rechts“ oder „app\0Grund“ → [`Cursor`]. Eine leere App
/// heißt: macOS nennt sie nicht.
fn parse(output: &str) -> Cursor {
    let (app, rest) = output.split_once('\0').unwrap_or((output, ""));
    let app = (!app.is_empty()).then(|| app.to_owned());
    let text = match rest.split_once('\0') {
        Some((before, after)) => CursorText::Known(Surroundings {
            before: before.to_owned(),
            after: after.to_owned(),
        }),
        None if output.contains('\0') => CursorText::Unknown {
            reason: rest.to_owned(),
        },
        None => CursorText::Unknown {
            reason: "keine Antwort".to_owned(),
        },
    };
    Cursor { app, text }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_helper_output() {
        assert_eq!(
            parse("com.apple.Notes\0Ich gehe \0 und dann"),
            Cursor {
                app: Some("com.apple.Notes".to_owned()),
                text: CursorText::Known(Surroundings {
                    before: "Ich gehe ".to_owned(),
                    after: " und dann".to_owned(),
                }),
            }
        );
        assert_eq!(
            parse("com.apple.Terminal\0Terminal: Umgebung wird nicht gelesen").text,
            CursorText::Unknown {
                reason: "Terminal: Umgebung wird nicht gelesen".to_owned()
            }
        );
        assert_eq!(
            parse("com.apple.Notes\0links\0"),
            Cursor {
                app: Some("com.apple.Notes".to_owned()),
                text: CursorText::Known(Surroundings {
                    before: "links".to_owned(),
                    after: String::new(),
                }),
            }
        );
        assert_eq!(
            parse("\0kein Textfeld"),
            Cursor {
                app: None,
                text: CursorText::Unknown {
                    reason: "kein Textfeld".to_owned()
                },
            },
            "macOS nennt die App nicht"
        );
        assert_eq!(
            parse("com.apple.Notes").text,
            CursorText::Unknown {
                reason: "keine Antwort".to_owned()
            }
        );
    }
}
