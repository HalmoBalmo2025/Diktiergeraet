//! Die Swift-Hilfsprogramme (Quelltext in `helpers/`), die neben dem
//! Programm in `Diktator.app/Contents/MacOS/` liegen.
//!
//! Was es nur für Swift gibt (`SwiftUI`, Foundation Models, Bedienungshilfen,
//! Rechtschreibprüfung), erledigt je ein kleines Hilfsprogramm; so braucht
//! Diktator selbst kein `unsafe`. Gebaut werden sie von `scripts/bundle.sh`;
//! bei `cargo run` fehlen sie.
//!
//! Die kurzen Aufrufe ([`run`], über [`crate::process`]) bekommen ihre
//! Eingabe über die Standardeingabe und antworten über die
//! Standardausgabe; scheitern sie, steht der Grund in der
//! Standardfehlerausgabe.

use crate::note::{Explain, Note};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::process::{self, Limit, ProcessError, Reason};

/// Ein Hilfsprogramm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Helper {
    /// Menüleisten-Symbol und Panel (`helpers/ui/`).
    Ui,
    /// Text um die Schreibmarke lesen (`helpers/context.swift`).
    Context,
    /// Text mit Apple Intelligence überarbeiten (`helpers/llm.swift`).
    Llm,
    /// Rechtschreibprüfung von macOS (`helpers/spell.swift`).
    Spell,
}

impl Helper {
    /// Dateiname neben dem Programm.
    #[must_use]
    pub(crate) const fn file_name(self) -> &'static str {
        match self {
            Self::Ui => "diktator-ui",
            Self::Context => "diktator-context",
            Self::Llm => "diktator-llm",
            Self::Spell => "diktator-spell",
        }
    }

    /// Name in Meldungen.
    const fn name(self) -> &'static str {
        match self {
            Self::Ui => "Oberfläche",
            Self::Context => "Hilfsprogramm diktator-context",
            Self::Llm => "Apple Intelligence",
            Self::Spell => "Rechtschreibprüfung",
        }
    }

    /// Englischer Name in Meldungen.
    const fn english_name(self) -> &'static str {
        match self {
            Self::Ui => "Interface",
            Self::Context => "Helper diktator-context",
            Self::Llm => "Apple Intelligence",
            Self::Spell => "Spell checking",
        }
    }

    /// Pfad neben dem laufenden Programm, falls das Hilfsprogramm dort liegt.
    #[must_use]
    pub(crate) fn path(self) -> Option<PathBuf> {
        let executable = std::env::current_exe().ok()?;
        let helper = executable.parent()?.join(self.file_name());
        helper.is_file().then_some(helper)
    }
}

/// Warum ein Hilfsprogramm keine Antwort lieferte.
#[derive(Debug, thiserror::Error)]
pub(crate) enum HelperError {
    /// Das Hilfsprogramm liegt nicht neben dem Programm.
    #[error("Hilfsprogramm {} fehlt", .0.file_name())]
    Missing(Helper),
    /// Das Hilfsprogramm ließ sich nicht starten.
    #[error("Hilfsprogramm {} nicht startbar: {source}", .helper.file_name())]
    Spawn {
        /// Welches.
        helper: Helper,
        /// Ursache.
        #[source]
        source: io::Error,
    },
    /// Ein- oder Ausgabe des Hilfsprogramms schlug fehl.
    #[error("Hilfsprogramm {}: {source}", .helper.file_name())]
    Io {
        /// Welches.
        helper: Helper,
        /// Ursache.
        #[source]
        source: io::Error,
    },
    /// Das Hilfsprogramm hat abgelehnt; der Grund kommt von ihm selbst
    /// (kein Textfeld im Fokus, Sprache unbekannt, Modell nicht verfügbar …).
    #[error("{reason}")]
    Failed {
        /// Welches.
        helper: Helper,
        /// Seine Fehlermeldung, in beiden Sprachen (als JSON-Zeile auf stderr).
        reason: Note,
    },
    /// Keine Antwort in der Zeitgrenze.
    #[error("{} antwortet nicht", .0.name())]
    TimedOut(Helper),
    /// Abgebrochen, bevor es antwortete.
    #[error("{}: abgebrochen", .0.name())]
    Cancelled(Helper),
    /// Ein Thread, der seine Ein- oder Ausgabe bediente, ist abgestürzt.
    #[error("Hilfsprogramm {}: Ein- oder Ausgabe-Thread abgestürzt", .0.file_name())]
    Crashed(Helper),
}

impl Explain for HelperError {
    fn english(&self) -> String {
        match self {
            Self::Missing(helper) => format!("helper {} is missing", helper.file_name()),
            Self::Spawn { helper, source } => {
                format!("cannot start helper {}: {source}", helper.file_name())
            }
            Self::Io { helper, source } => format!("helper {}: {source}", helper.file_name()),
            Self::Failed { reason, helper: _ } => reason.en().to_owned(),
            Self::TimedOut(helper) => format!("{} is not responding", helper.english_name()),
            Self::Cancelled(helper) => format!("{}: cancelled", helper.english_name()),
            Self::Crashed(helper) => format!(
                "helper {}: an input or output thread crashed",
                helper.file_name()
            ),
        }
    }
}

/// Ruft `helper` mit `args` auf, schickt `input` über die Standardeingabe
/// und liefert die Standardausgabe. Antwortet es nicht in `timeout`, wird
/// es beendet.
///
/// Beide Ausgaben werden schon während des Laufs gelesen, damit ein
/// Hilfsprogramm mit langer Antwort nicht an einem vollen Puffer hängt.
///
/// # Errors
/// [`HelperError`], wenn das Hilfsprogramm fehlt, nicht startet, scheitert
/// oder nicht rechtzeitig antwortet.
pub(crate) fn run(
    helper: Helper,
    args: &[&str],
    input: &str,
    timeout: Duration,
) -> Result<String, HelperError> {
    run_within(helper, args, input, Limit::After(timeout))
}

/// Wie [`run`], beendet nach `limit` (Zeitgrenze, auf Wunsch auch ein
/// Abbruch-Flag).
///
/// # Errors
/// Wie [`run`]; [`HelperError::Cancelled`], wenn abgebrochen wurde.
pub(crate) fn run_within(
    helper: Helper,
    args: &[&str],
    input: &str,
    limit: Limit<'_>,
) -> Result<String, HelperError> {
    let path = helper.path().ok_or(HelperError::Missing(helper))?;
    run_program(helper, &path, args, input, limit)
}

/// Wie [`run`], mit dem Programm unter `program`.
fn run_program(
    helper: Helper,
    program: &Path,
    args: &[&str],
    input: &str,
    limit: Limit<'_>,
) -> Result<String, HelperError> {
    let finished = process::run(
        process::command(program).args(args),
        input.as_bytes(),
        limit,
    )
    .map_err(|error| match error {
        ProcessError::Spawn(source) => HelperError::Spawn { helper, source },
        ProcessError::Io(source) => HelperError::Io { helper, source },
        ProcessError::TimedOut => HelperError::TimedOut(helper),
        ProcessError::Cancelled => HelperError::Cancelled(helper),
        ProcessError::Crashed => HelperError::Crashed(helper),
    })?;
    if finished.status.success() {
        return Ok(String::from_utf8_lossy(&finished.stdout).into_owned());
    }
    let reason = match finished.reason() {
        Reason::Said(line) => reason_from(&line),
        Reason::Exit(exit) => exit.note(),
    };
    Err(HelperError::Failed { helper, reason })
}

/// Der Grund aus der letzten Zeile der Fehlerausgabe eines Hilfsprogramms:
/// Die eigenen melden ihn als JSON-Zeile in beiden Sprachen (`{"de": …,
/// "en": …}`, siehe `fail` in den Swift-Quellen), auch wenn ein Framework
/// davor Warnungen schrieb; was sonst kommt (etwa die letzte Zeile eines
/// Absturzes), steht in beiden Fassungen gleich.
fn reason_from(line: &str) -> Note {
    serde_json::from_str(line).unwrap_or_else(|_| Note::same(line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_helper_is_reported() {
        // Neben dem Test-Programm liegt kein Hilfsprogramm.
        let error = run(Helper::Spell, &[], "", Duration::from_secs(1)).unwrap_err();
        assert!(
            matches!(error, HelperError::Missing(Helper::Spell)),
            "{error:?}"
        );
    }

    /// Ein Shell-Befehl als Hilfsprogramm.
    fn shell(script: &str, input: &str, timeout: Duration) -> Result<String, HelperError> {
        run_program(
            Helper::Spell,
            Path::new("/bin/sh"),
            &["-c", script],
            input,
            Limit::After(timeout),
        )
    }

    #[test]
    fn a_cancelled_helper_is_not_reported_as_timed_out() {
        let cancel = std::sync::atomic::AtomicBool::new(true);
        let error = run_program(
            Helper::Llm,
            Path::new("/bin/sh"),
            &["-c", "sleep 10"],
            "",
            Limit::AfterOrCancel(Duration::from_secs(60), &cancel),
        )
        .unwrap_err();
        assert!(
            matches!(error, HelperError::Cancelled(Helper::Llm)),
            "{error:?}"
        );
    }

    #[test]
    fn passes_long_input_and_output_through() {
        // Mehr als ein Pipe-Puffer (64 KB) in beide Richtungen.
        let input = "Wort\n".repeat(50_000);
        let output = shell("cat", &input, Duration::from_secs(10)).unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn reports_the_helpers_reason() {
        let error = shell(
            "echo 'Sprache unbekannt' >&2; exit 3",
            "",
            Duration::from_secs(10),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "Sprache unbekannt");
        let both = shell(
            r#"echo '{"de":"kein Text","en":"no text"}' >&2; exit 1"#,
            "",
            Duration::from_secs(10),
        )
        .unwrap_err();
        assert_eq!(both.to_string(), "kein Text");
        assert_eq!(both.english(), "no text");
        let silent = shell("exit 3", "", Duration::from_secs(10)).unwrap_err();
        assert!(
            matches!(
                silent,
                HelperError::Failed {
                    helper: _,
                    reason: _
                }
            ),
            "{silent:?}"
        );
        assert_eq!(silent.to_string(), "endete mit Code 3");
        assert_eq!(silent.english(), "exited with code 3");
        let warned_first = shell(
            r#"echo 'Warnung eines Frameworks' >&2; echo '{"de":"kein Text","en":"no text"}' >&2; exit 1"#,
            "",
            Duration::from_secs(10),
        )
        .unwrap_err();
        assert_eq!(warned_first.to_string(), "kein Text");
        assert_eq!(warned_first.english(), "no text");
    }

    #[test]
    fn stops_a_helper_that_does_not_answer() {
        use crate::clock::Clock as _;
        let started = crate::clock::SystemClock.now();
        let error = shell("sleep 10", "", Duration::from_millis(100)).unwrap_err();
        assert!(
            matches!(error, HelperError::TimedOut(Helper::Spell)),
            "{error:?}"
        );
        assert_eq!(error.to_string(), "Rechtschreibprüfung antwortet nicht");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "der hängende Helfer wird nach der Frist beendet, nicht erst nach 10 s"
        );
    }
}
