//! Beliebige Audio- und Videodateien mit `ffmpeg` in [`Pcm16k`] wandeln.

use crate::note::Explain;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::range::TimeRange;
use super::{Pcm16k, SAMPLE_RATE};
use crate::paths;
use crate::process::{self, Limit, ProcessError};

/// Bytes pro `f32`-Sample im Rohformat `f32le`.
const BYTES_PER_SAMPLE: usize = 4;

/// Länger darf das Dekodieren nicht dauern (gemessen: `docs/grenzen.md`).
const DECODE_TIMEOUT: Duration = Duration::from_mins(15);

/// Fehler beim Dekodieren.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DecodeError {
    /// `ffmpeg` ist nicht installiert.
    #[error("ffmpeg fehlt (im Terminal: brew install ffmpeg)")]
    MissingFfmpeg,
    /// `ffmpeg` ließ sich nicht starten oder seine Ausgabe nicht lesen.
    #[error("ffmpeg: {0}")]
    Process(#[source] ProcessError),
    /// Abgebrochen.
    #[error("abgebrochen")]
    Cancelled,
    /// `ffmpeg` lief, meldete aber einen Fehler.
    #[error("ffmpeg ist fehlgeschlagen: {}", .message.as_deref().unwrap_or("ohne Meldung"))]
    Failed {
        /// Letzte Zeile der Fehlermeldung von ffmpeg, der Pfad der Datei
        /// darin als „…“ (er käme sonst ins Protokoll); `None` ohne
        /// Meldung.
        message: Option<String>,
    },
}

impl Explain for DecodeError {
    fn english(&self) -> String {
        match self {
            Self::MissingFfmpeg => {
                "ffmpeg is missing (in Terminal: brew install ffmpeg)".to_owned()
            }
            Self::Process(source) => format!("ffmpeg: {}", source.english()),
            Self::Cancelled => "cancelled".to_owned(),
            Self::Failed { message } => format!(
                "ffmpeg failed: {}",
                message.as_deref().unwrap_or("without a message")
            ),
        }
    }
}

/// Dekodiert eine Datei (jedes Format, das ffmpeg kennt) zu 16 kHz mono;
/// mit `range` nur diesen Bereich. Wird `cancel` gesetzt, endet ffmpeg.
///
/// # Errors
/// [`DecodeError`], wenn ffmpeg fehlt, die Datei nicht lesen kann oder
/// abgebrochen wird.
pub(crate) fn decode_file(
    path: &Path,
    range: Option<&TimeRange>,
    cancel: &AtomicBool,
) -> Result<Pcm16k, DecodeError> {
    let mut command = process::command(paths::tool("ffmpeg").ok_or(DecodeError::MissingFfmpeg)?);
    command
        .arg("-nostdin")
        .args(["-v", "error"])
        .args(range.map(TimeRange::ffmpeg_args).unwrap_or_default())
        .arg("-i")
        .arg(path)
        .args(["-f", "f32le", "-ac", "1", "-ar"])
        .arg(SAMPLE_RATE.to_string())
        .arg("-");
    let output = process::run(
        &mut command,
        b"",
        Limit::AfterOrCancel(DECODE_TIMEOUT, cancel),
    )
    .map_err(|error| match error {
        ProcessError::Cancelled => DecodeError::Cancelled,
        other @ (ProcessError::Spawn(_)
        | ProcessError::Io(_)
        | ProcessError::TimedOut
        | ProcessError::Crashed) => DecodeError::Process(other),
    })?;
    if !output.status.success() {
        let shown_path = path.to_string_lossy();
        return Err(DecodeError::Failed {
            message: process::last_line(&output.stderr)
                .map(|line| line.replace(shown_path.as_ref(), "…")),
        });
    }
    Ok(Pcm16k::from_samples(samples_from_f32le(&output.stdout)))
}

/// Liest `f32`-Samples (little endian). Ein unvollständiges letztes Sample
/// wird verworfen; ffmpeg schreibt aber immer ganze Samples.
pub(crate) fn samples_from_f32le(bytes: &[u8]) -> Vec<f32> {
    let (samples, _rest) = bytes.as_chunks::<BYTES_PER_SAMPLE>();
    samples.iter().copied().map(f32::from_le_bytes).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_file_is_an_ffmpeg_failure_without_its_path() {
        if paths::tool("ffmpeg").is_none() {
            return;
        }
        let dir = crate::testing::TempDir::new("Vorlesungen Anna");
        let path = dir.file("BGB AT 3.m4a", "kein Ton\n".repeat(1_000));
        let error = decode_file(&path, None, &AtomicBool::new(false)).unwrap_err();
        let DecodeError::Failed { message } = &error else {
            panic!("ffmpeg ist gescheitert, gemeldet: {error:?}");
        };
        let message = message.as_deref().unwrap_or_default();
        assert!(
            !message.contains("Vorlesungen Anna") && !message.contains("BGB AT 3"),
            "Meldung (angezeigt und protokolliert): {message:?}"
        );
        assert!(!message.contains('\n'), "eine Zeile: {message:?}");
    }

    #[test]
    fn reads_little_endian_floats() {
        let mut bytes = Vec::new();
        for value in [0.5_f32, -1.0, 0.25] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.push(0xFF); // abgeschnittenes Sample
        assert_eq!(samples_from_f32le(&bytes), vec![0.5, -1.0, 0.25]);
    }
}
