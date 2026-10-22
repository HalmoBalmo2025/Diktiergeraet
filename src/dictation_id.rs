//! Die Nummer eines Diktats: Steuerung, Modell-Thread und
//! Überarbeiten-Thread erkennen daran Antworten auf verworfene Diktate.

/// Fortlaufende Nummer eines Diktats seit dem Start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DictationId(u64);

impl DictationId {
    /// Das erste Diktat.
    pub(crate) const FIRST: Self = Self(0);

    /// Das Diktat mit der Nummer `number` (für Tests).
    #[cfg(test)]
    pub(crate) const fn new(number: u64) -> Self {
        Self(number)
    }

    /// Das Diktat danach. Nach `u64::MAX` Diktaten beginnt die Zählung von
    /// vorn; so lange läuft kein Diktator.
    #[must_use]
    pub(crate) const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

impl std::fmt::Display for DictationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
