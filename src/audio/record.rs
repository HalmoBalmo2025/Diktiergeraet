//! Mikrofonaufnahme mit cpal.
//!
//! Der Mikrofon-Stream läuft nur zwischen [`start`] und
//! [`Recording::into_captured`] – der orange Punkt in der Menüleiste ist also nur
//! während einer Aufnahme zu sehen. Mit [`Recording::take_part`] lassen sich
//! fertige Teile schon während der Aufnahme abholen.
//!
//! Aufgenommen wird immer mit dem eingebauten Mikrofon des Macs, auch wenn
//! macOS gerade `AirPods` oder das iPhone als Eingang gewählt hat; nur ohne
//! eingebautes Mikrofon (Mac mini, Mac Studio) gilt der Standard-Eingang.

use crate::note::Explain;
use crate::numeric::count_as_f64;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, SampleFormat, Stream, StreamConfig};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

use super::chunk::{self, PartTiming};
use super::{Pcm16k, SAMPLE_RATE};

/// Geräte-UID des eingebauten Mikrofons (Apple Silicon), wie cpal sie
/// nach `coreaudio:` meldet.
const BUILT_IN_UID: &str = "BuiltInMicrophoneDevice";

/// Namensteile eingebauter Mikrofone, falls die UID anders lautet (ältere
/// Macs): `MacBook Pro-Mikrofon`, `iMac Microphone`.
const BUILT_IN_NAMES: [&str; 2] = ["MacBook", "iMac"];

/// Blockgröße des Resamplers in Frames.
const RESAMPLER_CHUNK: usize = 1024;

/// Fehler bei der Aufnahme.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RecordError {
    /// Kein Mikrofon gefunden.
    #[error("kein Mikrofon gefunden")]
    NoInputDevice,
    /// cpal meldet einen Fehler (Gerät, Format, Stream).
    #[error("Mikrofon: {0}")]
    Device(#[from] cpal::Error),
    /// Das Mikrofon liefert ein Sample-Format, das hier nicht verarbeitet wird.
    #[error("nicht unterstütztes Sample-Format: {0:?}")]
    UnsupportedFormat(SampleFormat),
    /// Der Resampler ließ sich nicht anlegen.
    #[error("Resampler: {0}")]
    ResamplerSetup(#[from] rubato::ResamplerConstructionError),
    /// Beim Umrechnen auf 16 kHz ging etwas schief.
    #[error("Umrechnen auf 16 kHz: {0}")]
    Resample(#[from] rubato::ResampleError),
    /// Der Puffer passt nicht zur angegebenen Länge.
    #[error("Audiopuffer: {0}")]
    Buffer(#[from] rubato::audioadapter_buffers::SizeError),
}

impl Explain for RecordError {
    fn english(&self) -> String {
        match self {
            Self::NoInputDevice => "no microphone found".to_owned(),
            Self::Device(source) => format!("microphone: {source}"),
            Self::UnsupportedFormat(format) => format!("unsupported sample format: {format:?}"),
            Self::ResamplerSetup(source) => format!("resampler: {source}"),
            Self::Resample(source) => format!("converting to 16 kHz: {source}"),
            Self::Buffer(source) => format!("audio buffer: {source}"),
        }
    }
}

/// Welches Mikrofon aufnimmt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Input {
    /// Das eingebaute.
    BuiltIn,
    /// Der Standard-Eingang (ein anderes Mikrofon).
    Default,
}

impl Input {
    /// Name fürs Protokoll.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::BuiltIn => "eingebautes Mikrofon",
            Self::Default => "Standard-Eingang",
        }
    }
}

/// Eine beendete Aufnahme.
#[derive(Debug)]
pub(crate) struct Captured {
    /// Die Aufnahme in 16 kHz mono.
    pub pcm: Pcm16k,
    /// Welches Mikrofon (für das Protokoll; sein Name kann den des Nutzers
    /// enthalten, „iPhone von Anna“).
    pub device: Input,
    /// Wie oft macOS einen kurzen Aussetzer gemeldet hat (Puffer über- oder
    /// unterlaufen, Gerät gewechselt). Dabei fehlen höchstens Millisekunden;
    /// die Aufnahme bleibt brauchbar.
    pub glitches: usize,
}

/// Das Mikrofon fiel aus; was bis dahin aufgenommen war, geht nicht
/// verloren.
#[derive(Debug)]
pub(crate) struct Interrupted {
    /// Warum die Aufnahme abbrach.
    pub error: RecordError,
    /// Die Aufnahme bis dahin; leer, wenn sie sich nicht umrechnen ließ.
    pub captured: Captured,
}

/// Was der Audio-Thread an die Aufnahme schickt.
enum Message {
    /// Ein Block verschachtelter Samples.
    Samples(Vec<f32>),
    /// Der Stream meldet einen Fehler.
    Failed(cpal::Error),
}

/// Eine laufende Aufnahme. Wird sie ohne [`Recording::into_captured`] fallen
/// gelassen, ist die Aufnahme verworfen (Esc).
pub(crate) struct Recording {
    stream: Stream,
    /// Welches Mikrofon.
    device: Input,
    /// Pegel (RMS) des zuletzt gelieferten Blocks als `f32`-Bits; schreibt
    /// der Audio-Thread.
    level: Arc<AtomicU32>,
    receiver: Receiver<Message>,
    config: StreamConfig,
    /// Noch nicht abgeholte Aufnahme, mono bei der Abtastrate des Geräts.
    buffer: Vec<f32>,
    /// Gemeldete kurze Aussetzer.
    glitches: usize,
}

impl std::fmt::Debug for Recording {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recording")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Startet eine Aufnahme vom eingebauten Mikrofon (sonst vom
/// Standard-Eingang).
///
/// # Errors
/// [`RecordError`], wenn kein Mikrofon da ist, die Berechtigung fehlt oder
/// der Stream nicht startet.
pub(crate) fn start() -> Result<Recording, RecordError> {
    let (device, input) = microphone().ok_or(RecordError::NoInputDevice)?;
    let supported = device.default_input_config()?;
    let config = supported.config();
    let (sender, receiver) = mpsc::channel();
    let level = Arc::new(AtomicU32::new(0));
    let meter = Arc::clone(&level);

    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "SampleFormat ist non_exhaustive; alle übrigen Formate werden gleich abgelehnt"
    )]
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_stream::<f32>(&device, &config, sender, meter, |sample| sample)?,
        SampleFormat::I16 => build_stream::<i16>(&device, &config, sender, meter, |sample| {
            f32::from(sample) / 32_768.0
        })?,
        other => return Err(RecordError::UnsupportedFormat(other)),
    };
    stream.play()?;
    Ok(Recording {
        stream,
        device: input,
        level,
        receiver,
        config,
        buffer: Vec::new(),
        glitches: 0,
    })
}

/// Das eingebaute Mikrofon, sonst der Standard-Eingang.
fn microphone() -> Option<(cpal::Device, Input)> {
    let host = cpal::default_host();
    let built_in = host.input_devices().ok().and_then(|mut devices| {
        devices.find(|device| {
            let uid = device.id().map(|id| id.to_string()).unwrap_or_default();
            let name = device
                .description()
                .map(|description| description.name().to_owned())
                .unwrap_or_default();
            is_built_in(&uid, &name)
        })
    });
    built_in.map(|device| (device, Input::BuiltIn)).or_else(|| {
        host.default_input_device()
            .map(|device| (device, Input::Default))
    })
}

/// Ist das Gerät mit dieser UID (`coreaudio:…`) und diesem Namen das
/// eingebaute Mikrofon?
fn is_built_in(uid: &str, name: &str) -> bool {
    uid.rsplit(':').next() == Some(BUILT_IN_UID)
        || BUILT_IN_NAMES.iter().any(|part| name.contains(part))
}

/// Baut einen Eingabe-Stream, der jeden Block als `f32` weiterreicht.
fn build_stream<T: cpal::SizedSample + Copy + 'static>(
    device: &cpal::Device,
    config: &StreamConfig,
    sender: Sender<Message>,
    level: Arc<AtomicU32>,
    to_f32: fn(T) -> f32,
) -> Result<Stream, RecordError> {
    let error_sender = sender.clone();
    let stream = device.build_input_stream(
        *config,
        move |data: &[T], _info| {
            let block: Vec<f32> = data.iter().copied().map(to_f32).collect();
            level.store(chunk::rms(&block).to_bits(), Ordering::Relaxed);
            // Schlägt fehl, wenn die Aufnahme schon beendet oder verworfen
            // ist; dann wird der Block zu Recht nicht mehr gebraucht.
            match sender.send(Message::Samples(block)) {
                Ok(()) | Err(mpsc::SendError(_)) => {}
            }
        },
        move |error| match error_sender.send(Message::Failed(error)) {
            Ok(()) | Err(mpsc::SendError(_)) => {}
        },
        None,
    )?;
    Ok(stream)
}

impl Recording {
    /// Beendet die Aufnahme und liefert sie als 16 kHz mono.
    ///
    /// # Errors
    /// [`Interrupted`], wenn der Stream unterwegs einen ernsten Fehler
    /// meldete (kurze Aussetzer zählen nicht) oder das Umrechnen scheitert;
    /// darin steht, was bis dahin aufgenommen war.
    pub(crate) fn into_captured(mut self) -> Result<Captured, Interrupted> {
        // Erst die letzten Blöcke abholen, dann den Stream schließen.
        let mut failure = self.drain().err();
        let Self {
            stream,
            device,
            receiver,
            config,
            mut buffer,
            mut glitches,
            ..
        } = self;
        drop(stream);
        for message in receiver.try_iter() {
            match message {
                Message::Samples(block) => buffer.extend(downmix(&block, config.channels)),
                Message::Failed(error) if is_glitch(&error) => {
                    glitches = glitches.saturating_add(1);
                }
                // Der erste ernste Fehler zählt.
                Message::Failed(error) => failure = failure.or(Some(RecordError::Device(error))),
            }
        }
        let (pcm, failure) = match resample_to_16k(&buffer, config.sample_rate) {
            Ok(samples) => (Pcm16k::from_samples(samples), failure),
            Err(error) => (Pcm16k::from_samples(Vec::new()), failure.or(Some(error))),
        };
        let captured = Captured {
            pcm,
            device,
            glitches,
        };
        match failure {
            None => Ok(captured),
            Some(error) => Err(Interrupted { error, captured }),
        }
    }

    /// Ein fertiger Teil der laufenden Aufnahme (bis zu einer Sprechpause,
    /// siehe [`chunk::part_end`]), oder `None`, wenn es noch zu früh ist
    /// oder `timing` erst am Ende schneidet. Der Rest bleibt für den
    /// nächsten Teil bzw. [`Recording::into_captured`].
    ///
    /// # Errors
    /// [`RecordError`], wenn der Stream einen ernsten Fehler meldete oder
    /// das Umrechnen scheitert. Was bis dahin aufgenommen war, bleibt im
    /// Puffer; [`Recording::into_captured`] liefert es.
    pub(crate) fn take_part(&mut self, timing: PartTiming) -> Result<Option<Pcm16k>, RecordError> {
        self.drain()?;
        let Some(cut) = chunk::part_end(&self.buffer, self.config.sample_rate, timing) else {
            return Ok(None);
        };
        let rest = self.buffer.split_off(cut.min(self.buffer.len()));
        let part = std::mem::replace(&mut self.buffer, rest);
        Ok(Some(Pcm16k::from_samples(resample_to_16k(
            &part,
            self.config.sample_rate,
        )?)))
    }

    /// Pegel (RMS) des zuletzt gelieferten Blocks, für die Anzeige.
    #[must_use]
    pub(crate) fn current_level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Wie lange die Aufnahme ist, die noch in keinem Teil steckt: bei
    /// „erst am Ende“ die ganze bisherige. Zählt die Blöcke, die
    /// [`Recording::take_part`] zuletzt abgeholt hat.
    #[must_use]
    pub(crate) fn buffered(&self) -> Duration {
        let rate = f64::from(self.config.sample_rate);
        debug_assert!(rate > 0.0, "invariant: Geräte liefern eine Abtastrate");
        if rate <= 0.0 {
            // Ohne Abtastrate keine Dauer; dann greift auch keine Grenze.
            return Duration::ZERO;
        }
        Duration::from_secs_f64(count_as_f64(self.buffer.len()) / rate)
    }

    /// Holt die bisher gelieferten Blöcke in den Puffer.
    fn drain(&mut self) -> Result<(), RecordError> {
        for message in self.receiver.try_iter() {
            match message {
                Message::Samples(block) => {
                    self.buffer.extend(downmix(&block, self.config.channels));
                }
                Message::Failed(error) if is_glitch(&error) => {
                    self.glitches = self.glitches.saturating_add(1);
                }
                Message::Failed(error) => return Err(RecordError::Device(error)),
            }
        }
        Ok(())
    }
}

/// Nur ein kurzer Aussetzer, nach dem der Stream weiterläuft?
fn is_glitch(error: &cpal::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied
    )
}

/// Mittelt die Kanäle zu einem.
fn downmix(interleaved: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let channel_count = f32::from(channels);
    interleaved
        .chunks_exact(usize::from(channels))
        .map(|frame| frame.iter().sum::<f32>() / channel_count)
        .collect()
}

/// Rechnet mono-Audio von `source_rate` auf 16 kHz um.
fn resample_to_16k(mono: &[f32], source_rate: u32) -> Result<Vec<f32>, RecordError> {
    if source_rate == SAMPLE_RATE || mono.is_empty() {
        return Ok(mono.to_vec());
    }
    let (Ok(source), Ok(target)) = (usize::try_from(source_rate), usize::try_from(SAMPLE_RATE))
    else {
        return Ok(mono.to_vec());
    };
    let mut resampler = Fft::<f32>::new(source, target, RESAMPLER_CHUNK, 1, FixedSync::Input)?;
    let input = InterleavedSlice::new(mono, 1, mono.len())?;
    let output = resampler.process_all(&input, mono.len(), None)?;
    Ok(output.take_data())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_built_in_microphone() {
        assert!(
            is_built_in("coreaudio:BuiltInMicrophoneDevice", "MacBook Pro-Mikrofon"),
            "das MacBook-Mikrofon gilt als eingebaut"
        );
        assert!(
            is_built_in(
                "coreaudio:AppleHDAEngineInput:1B,0,1,0:1",
                "iMac Microphone"
            ),
            "das iMac-Mikrofon gilt als eingebaut"
        );
        assert!(
            !is_built_in(
                "coreaudio:B9187B5F-A9BD-434B-BAD2-4C0600000003",
                "Mikrofon von „iPhone von Anna“"
            ),
            "das iPhone-Mikrofon gilt nicht als eingebaut"
        );
        assert!(
            !is_built_in("coreaudio:5C-52-30-AA-BB-CC:input", "AirPods Pro"),
            "AirPods gelten nicht als eingebaut"
        );
    }

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(downmix(&[0.1, 0.2], 1), vec![0.1, 0.2]);
    }

    #[test]
    fn resample_48k_to_16k_keeps_duration() {
        let one_second = vec![0.0_f32; 48_000];
        let output = resample_to_16k(&one_second, 48_000).unwrap();
        assert!(
            (15_900..=16_100).contains(&output.len()),
            "{}",
            output.len()
        );
    }

    #[test]
    fn resample_44k1_to_16k_keeps_duration() {
        let one_second = vec![0.0_f32; 44_100];
        let output = resample_to_16k(&one_second, 44_100).unwrap();
        assert!(
            (15_900..=16_100).contains(&output.len()),
            "{}",
            output.len()
        );
    }
}
