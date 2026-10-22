//! Audio: Aufnahme, Dekodieren von Dateien, Stille und Abschnitte.
//!
//! Alles, was das Modell bekommt, ist [`Pcm16k`]: 16 kHz, mono, `f32` in
//! `[-1, 1]` (übersteuerte Werte gekappt, siehe [`Pcm16k::from_samples`]).

pub(crate) mod chunk;
pub(crate) mod decode;
pub(crate) mod range;
pub(crate) mod record;
pub(crate) mod wav;

use std::time::Duration;

use crate::numeric::count_as_f64;
use serde::Serialize;

/// Abtastrate, die Cohere Transcribe erwartet.
pub(crate) const SAMPLE_RATE: u32 = 16_000;

/// Audio in dem Format, das das Modell erwartet: 16 kHz, mono, `f32`.
/// Jedes Sample ist endlich und liegt in `[-1, 1]`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Pcm16k {
    samples: Vec<f32>,
}

impl Pcm16k {
    /// Übernimmt Samples, die bereits 16 kHz mono sind. Übersteuerte Werte
    /// (ffmpeg und der Resampler kappen nicht, `docs/grenzen.md`) werden auf
    /// `[-1, 1]` gekappt, wie ein Wandler es täte; nicht endliche Werte
    /// werden Stille.
    #[must_use]
    pub(crate) fn from_samples(mut samples: Vec<f32>) -> Self {
        for sample in &mut samples {
            *sample = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
        Self { samples }
    }

    /// Die Samples.
    #[must_use]
    pub(crate) fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// Spieldauer.
    #[must_use]
    pub(crate) fn duration(&self) -> Duration {
        duration_of(self.samples.len())
    }

    /// `true`, wenn kein einziges Sample vorhanden ist.
    #[must_use]
    pub(crate) const fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

/// Schwellen in dBFS für die Pegelanzeige: leises Sprechen erreicht die
/// ersten Stufen, lautes die letzten.
const METER_STEPS: [f32; 10] = [
    -52.0, -47.0, -42.0, -38.0, -34.0, -30.0, -26.0, -22.0, -18.0, -14.0,
];

/// Pegelstufe für die Anzeige, 0 (still) bis 10 (so viele wie
/// [`METER_STEPS`]). Zum Panel geht sie als Zahl.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct Level(u8);

impl Level {
    /// Still.
    pub(crate) const SILENT: Self = Self(0);

    /// Die Stufe, 0 bis 10.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn steps(self) -> u8 {
        self.0
    }
}

/// Pegel als Stufe für die Anzeige.
#[must_use]
pub(crate) fn meter(rms: f32) -> Level {
    if rms <= 0.0 || !rms.is_finite() {
        return Level::SILENT;
    }
    let decibels = 20.0 * rms.log10();
    let steps = METER_STEPS.iter().filter(|step| decibels > **step).count();
    // Höchstens so viele wie Schwellen, also 10.
    Level(u8::try_from(steps).unwrap_or(u8::MAX))
}

/// Spieldauer einer Anzahl von 16-kHz-Samples.
#[must_use]
pub(crate) fn duration_of(sample_count: usize) -> Duration {
    Duration::from_secs_f64(count_as_f64(sample_count) / f64::from(SAMPLE_RATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_stay_finite_and_within_one() {
        let pcm = Pcm16k::from_samples(vec![1.5, -2.0, f32::NAN, f32::INFINITY, 0.25]);
        assert_eq!(pcm.samples(), [1.0, -1.0, 0.0, 0.0, 0.25]);
    }

    #[test]
    fn meter_grows_with_the_level() {
        assert_eq!(meter(0.0).steps(), 0);
        assert_eq!(meter(0.000_1).steps(), 0);
        // −40 dB bzw. −20 dB: leises bzw. kräftiges Sprechen.
        assert_eq!(meter(0.01).steps(), 3);
        assert_eq!(meter(0.1).steps(), 8);
        assert_eq!(meter(0.5).steps(), 10);
        assert_eq!(meter(f32::NAN).steps(), 0);
    }
}
