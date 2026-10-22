//! Zahlenumwandlungen, die Genauigkeit verlieren können: die eine Stelle
//! für `as` (Kernregel 3). Jede sagt, wo sie ungenau wird.

/// Eine Anzahl als `f64`. Exakt bis 2^53 (weit mehr Samples, als je
/// vorkommen); darüber auf die nächste darstellbare Zahl gerundet, nie
/// gekappt.
#[must_use]
pub(crate) fn count_as_f64(count: usize) -> f64 {
    // `usize` hat höchstens 64 Bit; in zwei 32-Bit-Hälften verlustfrei
    // über `f64::from`.
    let count = u64::try_from(count).unwrap_or(u64::MAX);
    let high = u32::try_from(count >> 32).unwrap_or(u32::MAX);
    let low = u32::try_from(count & u64::from(u32::MAX)).unwrap_or(u32::MAX);
    f64::from(high) * 4_294_967_296.0 + f64::from(low)
}

/// `value` als `f32`, auf die nächste darstellbare Zahl gerundet; zu
/// große Beträge werden unendlich.
#[must_use]
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "die eine Umwandlung f64 → f32; das Runden ist hier gewollt"
)]
pub(crate) fn f64_to_f32(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_beyond_u16_and_u32_stay_exact() {
        // Exakt verglichen: über die Bits, ohne Spielraum.
        let exact = |count: usize, expected: f64| {
            assert_eq!(count_as_f64(count).to_bits(), expected.to_bits(), "{count}");
        };
        exact(0, 0.0);
        exact(70_000, 70_000.0);
        exact(5_000_000_000, 5_000_000_000.0);
        exact(1 << 53, 9_007_199_254_740_992.0);
        assert_eq!(f64_to_f32(0.5).to_bits(), 0.5_f32.to_bits());
    }
}
