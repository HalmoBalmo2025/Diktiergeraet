//! Aufnahmen als WAV speichern (zur Fehlersuche).

use crate::note::Explain;
use std::fs;
use std::path::Path;

use super::{Pcm16k, SAMPLE_RATE};

/// Bytes pro Sample (32-Bit-Gleitkomma).
const BYTES_PER_SAMPLE: u32 = 4;

/// Fehler beim Schreiben.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WavError {
    /// Die Aufnahme ist für eine WAV-Datei zu lang (über 4 GB).
    #[error("Aufnahme zu lang für WAV")]
    TooLong,
    /// Die Sample-Anzahl passt nicht in das 32-Bit-Feld des WAV-Kopfs.
    #[error("Aufnahme zu lang für WAV: {0}")]
    TooManySamples(#[from] std::num::TryFromIntError),
    /// Die Datei ließ sich nicht schreiben.
    #[error("WAV schreiben: {0}")]
    Io(#[from] std::io::Error),
}

impl Explain for WavError {
    fn english(&self) -> String {
        match self {
            Self::TooLong => "recording too long for WAV".to_owned(),
            Self::TooManySamples(source) => format!("recording too long for WAV: {source}"),
            Self::Io(source) => format!("writing WAV: {source}"),
        }
    }
}

/// Schreibt 16 kHz mono `f32` als WAV (Format 3, IEEE float).
///
/// # Errors
/// [`WavError`], wenn die Aufnahme zu lang ist oder das Schreiben scheitert.
#[expect(
    clippy::disallowed_methods,
    reason = "überschreibt nur die eigene Prüfaufnahme im App-Ordner, nie eine Datei des Nutzers"
)]
pub(crate) fn write(path: &Path, pcm: &Pcm16k) -> Result<(), WavError> {
    fs::write(path, encode(pcm)?)?;
    Ok(())
}

/// Kodiert die Aufnahme als WAV-Bytes.
fn encode(pcm: &Pcm16k) -> Result<Vec<u8>, WavError> {
    let sample_count = u32::try_from(pcm.samples().len())?;
    let data_len = sample_count
        .checked_mul(BYTES_PER_SAMPLE)
        .ok_or(WavError::TooLong)?;
    let riff_len = data_len.checked_add(36).ok_or(WavError::TooLong)?;
    let byte_rate = SAMPLE_RATE.saturating_mul(BYTES_PER_SAMPLE);

    let mut bytes = Vec::with_capacity(usize::try_from(riff_len).unwrap_or(0).saturating_add(8));
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&riff_len.to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&3_u16.to_le_bytes()); // IEEE float
    bytes.extend_from_slice(&1_u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&byte_rate.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes()); // Blockgröße
    bytes.extend_from_slice(&32_u16.to_le_bytes()); // Bits pro Sample
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in pcm.samples() {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_length_are_consistent() {
        let pcm = Pcm16k::from_samples(vec![0.5; 10]);
        let bytes = encode(&pcm).unwrap();
        assert_eq!(bytes.len(), 44 + 40);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 40);
    }

    #[test]
    fn header_fields_and_samples_round_trip() {
        let samples = vec![0.5, -0.25, 1.0, 0.0];
        let bytes = encode(&Pcm16k::from_samples(samples.clone())).unwrap();
        let u16_at = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(u32_at(4), 36 + 16);
        assert_eq!(u16_at(20), 3, "IEEE float");
        assert_eq!(u16_at(22), 1, "mono");
        assert_eq!(u32_at(24), SAMPLE_RATE);
        assert_eq!(u32_at(28), SAMPLE_RATE * 4, "Bytes pro Sekunde");
        assert_eq!(u16_at(32), 4, "Blockgröße");
        assert_eq!(u16_at(34), 32, "Bits pro Sample");
        let decoded: Vec<f32> = bytes[44..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect();
        assert_eq!(decoded, samples);
    }
}
