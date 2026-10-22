//! Überarbeiten mit Apple Intelligence (Foundation Models, auf dem Gerät)
//! über das Hilfsprogramm `diktator-llm` (Quelltext in `helpers/llm.swift`).
//! Die Anweisung geht als Argument, der Text über die Standardeingabe, das
//! Ergebnis kommt über die Standardausgabe.

use super::PolishError;
use crate::helper::{self, Helper};
use crate::process::Limit;

/// Überarbeitet `text` nach `instructions`; höchstens bis `limit`.
pub(super) fn ask(text: &str, instructions: &str, limit: Limit<'_>) -> Result<String, PolishError> {
    let output = helper::run_within(Helper::Llm, &[instructions], text, limit)?;
    let polished = output.trim();
    if polished.is_empty() {
        return Err(PolishError::Empty);
    }
    Ok(polished.to_owned())
}
