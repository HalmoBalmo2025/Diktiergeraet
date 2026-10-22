//! Stille abschneiden und lange Aufnahmen in Abschnitte teilen.
//!
//! Cohere Transcribe „transkribiert“ auch Rauschen, deshalb wird Stille am
//! Anfang und Ende entfernt und eine Aufnahme ohne Sprache gar nicht erst ans
//! Modell gegeben. Längeres wird an leisen Stellen in Abschnitte von
//! höchstens [`MAX_CHUNK_SAMPLES`] geteilt; warum 30 s, steht in
//! `docs/grenzen.md`.

use crate::note::Explain;
use crate::numeric;
use serde::{Deserialize, Serialize};

/// Länge eines Analyse-Rahmens: 20 ms bei 16 kHz.
const FRAME_LEN: usize = 320;

/// Untergrenze der Sprachschwelle (RMS), damit leises Rauschen nie als
/// Sprache gilt; gemessen in `docs/grenzen.md`. Stille mit Tastenklicks
/// bleibt darunter, gemessen an [`MIN_VOICED_FRAMES`] und
/// [`MIN_EDGE_RUN`].
const MIN_THRESHOLD: f32 = 0.0015;

/// Obergrenze der Sprachschwelle: normal laute Sprache zählt immer.
const MAX_THRESHOLD: f32 = 0.02;

/// Ein Rahmen ist Sprache, wenn er so viel lauter als das Grundrauschen ist.
const NOISE_FACTOR: f32 = 3.0;

/// Mindestens so viele Sprachrahmen (160 ms), sonst war es nur ein Geräusch.
const MIN_VOICED_FRAMES: usize = 8;

/// So viele Sprachrahmen am Stück (100 ms) braucht es, damit Anfang oder
/// Ende der Sprache dort liegt. Kürzeres am Rand ist ein Geräusch, meist der
/// Klick der fn-Taste beim Beenden; bliebe es drin, bliebe auch die Pause
/// davor, und in Stille erfindet das Modell Text („Herr Präsident!“).
const MIN_EDGE_RUN: usize = 5;

/// Rand, der vor und nach der Sprache stehen bleibt (300 ms).
const PAD_FRAMES: usize = 15;

/// Teile eines laufenden Diktats: Ab dem Doppelten der eingestellten
/// Sekunden ([`PartSeconds`]) wird schon an einer kurzen Sprechpause
/// geschnitten …
const PART_MIN_FACTOR: u32 = 2;

/// … ab dem Fünffachen am leisesten Rahmen, falls keine Pause kommt …
const PART_MAX_FACTOR: u32 = 5;

/// … und nie später als nach so vielen Sekunden: Längere Teile teilte
/// Diktator vor dem Modell noch einmal an einer beliebigen leisen Stelle
/// (höchstens [`MAX_CHUNK_SAMPLES`] am Stück, `docs/grenzen.md`). Der Abstand
/// zu 30 s fängt ab, dass nur jede halbe Sekunde nachgesehen wird.
const PART_LIMIT_SECONDS: u32 = 28;

/// Höchstwert für [`PartSeconds`]. Darüber bliebe bis zur Grenze von
/// 28 s kaum Zeit, auf ein Satzende zu warten.
pub(crate) const MAX_PART_SECONDS: u8 = 20;

/// So viele leise Rahmen am Stück (600 ms) gelten als lange Pause; Atempausen
/// mitten im Satz sind meist kürzer.
const LONG_PAUSE_FRAMES: usize = 30;

/// So viele leise Rahmen am Stück (300 ms) gelten als Sprechpause.
const PAUSE_FRAMES: usize = 15;

/// Höchstlänge eines Abschnitts: 30 s bei [`SAMPLE_RATE`](super::SAMPLE_RATE) (16 kHz).
pub(crate) const MAX_CHUNK_SAMPLES: usize = 30 * 16_000;

/// Wann ein laufendes Diktat schon in Teilen transkribiert wird.
///
/// Früh geschnitten erscheint der Text schon beim Sprechen; dafür liegt der
/// Schnitt eher mitten im Satz, und dort setzt das Modell oft einen Punkt
/// oder lässt das Komma weg. Erst am Ende geschnitten, sieht es jeden Satz
/// ganz (bis 30 s; Längeres teilt Diktator an leisen Stellen).
///
/// In der Datei und vom Panel kommt es als Zahl: 0 = erst am Ende, sonst
/// die Sekunden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub(crate) enum PartTiming {
    /// Nichts während der Aufnahme; alles beim Beenden (lange Aufnahmen
    /// dann an leisen Stellen in Abschnitte, siehe [`split`]).
    AtEnd,
    /// Ab so vielen Sekunden an der nächsten langen Pause (meist ein
    /// Satzende), ab dem Doppelten an jeder Sprechpause, ab dem Fünffachen
    /// am leisesten Rahmen, spätestens nach 28 s.
    After(PartSeconds),
}

impl TryFrom<u8> for PartTiming {
    type Error = PartSecondsError;

    /// 0 = erst am Ende, 1 bis [`MAX_PART_SECONDS`] die Sekunden.
    fn try_from(seconds: u8) -> Result<Self, Self::Error> {
        match seconds {
            0 => Ok(Self::AtEnd),
            seconds => PartSeconds::try_from(seconds).map(Self::After),
        }
    }
}

impl From<PartTiming> for u8 {
    fn from(timing: PartTiming) -> Self {
        match timing {
            PartTiming::AtEnd => 0,
            PartTiming::After(seconds) => seconds.get(),
        }
    }
}

/// Sekunden außerhalb von 0 bis [`MAX_PART_SECONDS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{0} s bis zum ersten Teil, erlaubt sind 0 (erst am Ende) bis {MAX_PART_SECONDS}")]
pub(crate) struct PartSecondsError(pub(crate) u16);

impl Explain for PartSecondsError {
    fn english(&self) -> String {
        format!(
            "{} s until the first part, allowed are 0 (only at the end) to {MAX_PART_SECONDS}",
            self.0
        )
    }
}

impl Default for PartTiming {
    fn default() -> Self {
        Self::After(PartSeconds::DEFAULT)
    }
}

/// Sekunden bis zum ersten möglichen Schnitt, 1 bis [`MAX_PART_SECONDS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PartSeconds(u8);

impl PartSeconds {
    /// Standard: nach 5 s an einer langen Pause.
    pub(crate) const DEFAULT: Self = Self(5);

    /// Die Sekunden.
    #[must_use]
    pub(crate) const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for PartSeconds {
    type Error = PartSecondsError;

    /// Nur 1 bis [`MAX_PART_SECONDS`].
    fn try_from(seconds: u8) -> Result<Self, Self::Error> {
        match seconds {
            1..=MAX_PART_SECONDS => Ok(Self(seconds)),
            other => Err(PartSecondsError(u16::from(other))),
        }
    }
}

/// Pegel einer Aufnahme, für die Entscheidung „Sprache oder nicht“ und für
/// das Protokoll.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Levels {
    /// Lautester Rahmen (RMS).
    pub peak: f32,
    /// Grundrauschen: das 20. Perzentil der Rahmenpegel.
    pub noise_floor: f32,
    /// Ab diesem Pegel zählt ein Rahmen als Sprache.
    pub threshold: f32,
    /// Anzahl der Sprachrahmen (je 20 ms).
    pub voiced_frames: usize,
}

impl std::fmt::Display for Levels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Spitze {:.3}, Grundrauschen {:.4}, Schwelle {:.4}, Sprachrahmen {}",
            self.peak, self.noise_floor, self.threshold, self.voiced_frames
        )
    }
}

/// Misst die Pegel einer Aufnahme.
///
/// Die Schwelle richtet sich nach dem Grundrauschen, nicht nach dem
/// lautesten Rahmen: Ein einzelnes lautes Geräusch (Tastenklick, Startklang)
/// darf leise Sprache nicht unter die Schwelle drücken.
#[must_use]
pub(crate) fn measure(samples: &[f32]) -> Levels {
    let levels = frame_levels(samples);
    let peak = levels.iter().copied().fold(0.0_f32, f32::max);
    let noise_floor = percentile_20(&levels);
    let threshold = (noise_floor * NOISE_FACTOR).clamp(MIN_THRESHOLD, MAX_THRESHOLD);
    let voiced_frames = levels.iter().filter(|level| **level >= threshold).count();
    Levels {
        peak,
        noise_floor,
        threshold,
        voiced_frames,
    }
}

/// Schneidet Stille vorne und hinten ab.
///
/// Gibt `None` zurück, wenn die Aufnahme keine Sprache enthält.
#[must_use]
pub(crate) fn trim_silence(samples: &[f32]) -> Option<&[f32]> {
    samples.get(speech_range(samples)?)
}

/// Wo in `samples` die Sprache liegt (Samples, mit Rand); `None` ohne
/// Sprache. Der Anfang sagt, wie weit sie hinter dem Anfang der Aufnahme
/// beginnt (für Zeitmarken).
#[must_use]
pub(crate) fn speech_range(samples: &[f32]) -> Option<std::ops::Range<usize>> {
    let measured = measure(samples);
    if measured.voiced_frames < MIN_VOICED_FRAMES {
        return None;
    }
    let levels = frame_levels(samples);
    let voiced: Vec<bool> = levels
        .iter()
        .map(|level| *level >= measured.threshold)
        .collect();
    let first = first_run_start(&voiced, MIN_EDGE_RUN)?;
    let reversed: Vec<bool> = voiced.iter().rev().copied().collect();
    let last = voiced
        .len()
        .checked_sub(1)?
        .checked_sub(first_run_start(&reversed, MIN_EDGE_RUN)?)?;

    let start_frame = first.saturating_sub(PAD_FRAMES);
    let end_frame = last.saturating_add(1).saturating_add(PAD_FRAMES);
    let start = start_frame.saturating_mul(FRAME_LEN);
    let end = end_frame.saturating_mul(FRAME_LEN).min(samples.len());
    (start < end).then_some(start..end)
}

/// Erster Rahmen einer Folge von mindestens `run` Sprachrahmen.
fn first_run_start(voiced: &[bool], run: usize) -> Option<usize> {
    let mut length = 0_usize;
    for (index, is_voiced) in voiced.iter().enumerate() {
        length = if *is_voiced {
            length.saturating_add(1)
        } else {
            0
        };
        if length >= run {
            return index.checked_add(1)?.checked_sub(run);
        }
    }
    None
}

/// 20. Perzentil der Pegel (0 bei leerer Eingabe).
fn percentile_20(levels: &[f32]) -> f32 {
    let mut sorted = levels.to_vec();
    sorted.sort_by(f32::total_cmp);
    let index = sorted.len().checked_div(5).unwrap_or(0);
    sorted.get(index).copied().unwrap_or(0.0)
}

/// Wo ein laufendes Diktat (Mono-Samples bei `sample_rate`) geschnitten
/// werden kann.
///
/// Der Teil wird schon transkribiert, während weiter aufgenommen wird.
/// Mit [`PartTiming::After`] (Standard 5 s) wird mitten in der ersten
/// langen Pause nach diesen Sekunden geschnitten oder in der ersten kurzen
/// nach dem Doppelten (10 s), sonst ab dem Fünffachen (25 s) am leisesten
/// Rahmen – beides höchstens nach 28 s. `None`: noch
/// nicht schneiden (mit [`PartTiming::AtEnd`] nie).
#[must_use]
pub(crate) fn part_end(samples: &[f32], sample_rate: u32, timing: PartTiming) -> Option<usize> {
    let PartTiming::After(early) = timing else {
        return None;
    };
    let early = u32::from(early.get());
    let frame_len = usize::try_from(sample_rate.checked_div(50)?).ok()?.max(1);
    let seconds =
        |count: u32| usize::try_from(sample_rate.saturating_mul(count)).unwrap_or(usize::MAX);
    let max_seconds = early
        .saturating_mul(PART_MAX_FACTOR)
        .min(PART_LIMIT_SECONDS);
    let min_seconds = early.saturating_mul(PART_MIN_FACTOR).min(max_seconds);
    let (min_len, max_len) = (seconds(min_seconds), seconds(max_seconds));
    if samples.len() < seconds(early) {
        return None;
    }
    let levels: Vec<f32> = samples.chunks(frame_len).map(rms).collect();
    let noise_floor = percentile_20(&levels);
    let threshold = (noise_floor * NOISE_FACTOR).clamp(MIN_THRESHOLD, MAX_THRESHOLD);
    let early_frame = seconds(early).checked_div(frame_len).unwrap_or(0);
    let min_frame = min_len.checked_div(frame_len).unwrap_or(0);
    let mut quiet = 0_usize;
    for (index, level) in levels.iter().enumerate().skip(early_frame) {
        quiet = if *level < threshold {
            quiet.saturating_add(1)
        } else {
            0
        };
        // Die Pause muss ganz in der Aufnahme liegen (nicht das Ende sein).
        let inside = index.saturating_add(1) < levels.len();
        let needed = if index >= min_frame {
            PAUSE_FRAMES
        } else {
            LONG_PAUSE_FRAMES
        };
        if quiet >= needed && inside {
            let middle = index.saturating_sub(needed.checked_div(2).unwrap_or(0));
            return Some(middle.saturating_mul(frame_len));
        }
    }
    if samples.len() < max_len {
        return None;
    }
    // Gesucht wird ab dem Doppelten; fällt das schon auf die Grenze (ab
    // 14 s), ab den eingestellten Sekunden – sonst bliebe nichts zum Suchen.
    let quietest_from = if min_seconds < max_seconds {
        min_frame
    } else {
        early_frame
    };
    // Ohne den letzten, meist unvollständigen Rahmen: Mit wenigen Samples
    // ist er oft der leiseste, und der Schnitt fiele ans Pufferende.
    let quietest = levels
        .iter()
        .enumerate()
        .take(levels.len().saturating_sub(1))
        .skip(quietest_from)
        .min_by(|(_, left), (_, right)| left.total_cmp(right))
        .map(|(index, _)| index)?;
    Some(quietest.saturating_mul(frame_len).max(frame_len))
}

/// Teilt Audio in Abschnitte von höchstens `max_len` Samples.
///
/// Geschnitten wird im letzten Drittel jedes Abschnitts am leisesten
/// Rahmen, damit kein Wort zerteilt wird.
#[must_use]
pub(crate) fn split(samples: &[f32], max_len: usize) -> Vec<&[f32]> {
    let max_len = max_len.max(FRAME_LEN);
    let mut chunks = Vec::new();
    let mut rest = samples;
    while rest.len() > max_len {
        let cut = quietest_cut(rest, max_len);
        let (head, tail) = rest.split_at(cut.min(rest.len()));
        chunks.push(head);
        rest = tail;
    }
    if !rest.is_empty() {
        chunks.push(rest);
    }
    chunks
}

/// Schnittstelle (Sample-Index) im Fenster von zwei Dritteln bis zur vollen
/// Höchstlänge: der Anfang des leisesten Rahmens dort.
fn quietest_cut(samples: &[f32], max_len: usize) -> usize {
    let window_start = max_len.saturating_mul(2).checked_div(3).unwrap_or(0);
    let window = samples.get(window_start..max_len).unwrap_or_default();
    let quietest_frame = frame_levels(window)
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| left.total_cmp(right))
        .map_or(0, |(index, _)| index);
    let cut = window_start.saturating_add(quietest_frame.saturating_mul(FRAME_LEN));
    // Ein Schnitt bei 0 würde nie vorankommen.
    if cut == 0 { max_len } else { cut }
}

/// RMS-Pegel jedes 20-ms-Rahmens.
fn frame_levels(samples: &[f32]) -> Vec<f32> {
    samples.chunks(FRAME_LEN).map(rms).collect()
}

/// Effektivwert eines Rahmens.
pub(crate) fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let energy: f64 = frame
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum();
    numeric::f64_to_f32((energy / numeric::count_as_f64(frame.len())).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_a_frame_longer_than_u16_max() {
        let level = rms(&vec![0.5_f32; 70_000]);
        assert!(
            (level - 0.5).abs() < 1e-6,
            "Effektivwert von lauter 0,5: {level}"
        );
    }
    use crate::audio::SAMPLE_RATE;

    const SECOND: usize = 16_000;

    #[test]
    fn constants_match_the_sample_rate() {
        assert_eq!(u32::try_from(SECOND).unwrap(), SAMPLE_RATE);
        assert_eq!(MAX_CHUNK_SAMPLES, 30 * SECOND);
    }

    fn silence(len: usize) -> Vec<f32> {
        vec![0.0; len]
    }

    fn tone(len: usize, amplitude: f32) -> Vec<f32> {
        (0..len)
            .map(|index| {
                let phase = f32::from(u16::try_from(index % 100).unwrap()) / 100.0;
                amplitude * (phase * std::f32::consts::TAU).sin()
            })
            .collect()
    }

    #[test]
    fn silence_is_not_speech() {
        assert!(
            trim_silence(&silence(SECOND)).is_none(),
            "Stille ist keine Sprache"
        );
        assert!(
            trim_silence(&tone(SECOND, 0.002)).is_none(),
            "ein kaum hörbarer Ton ist keine Sprache"
        );
    }

    #[test]
    fn quiet_speech_is_speech() {
        // Leise gesprochen (Effektivwert um 0,0028) in einem ruhigen Raum.
        let mut audio = tone(SECOND, 0.0003);
        audio.extend(tone(2 * SECOND, 0.004));
        audio.extend(tone(SECOND, 0.0003));
        let trimmed = trim_silence(&audio).unwrap();
        assert!(
            trimmed.len() >= 2 * SECOND,
            "leise Sprache bleibt vollständig erhalten"
        );
    }

    #[test]
    fn short_click_is_not_speech() {
        let mut audio = silence(SECOND);
        audio.extend(tone(FRAME_LEN * 3, 0.5));
        audio.extend(silence(SECOND));
        assert!(
            trim_silence(&audio).is_none(),
            "ein kurzer Klick zwischen Stille ist keine Sprache"
        );
    }

    #[test]
    fn trims_leading_and_trailing_silence_with_padding() {
        let mut audio = silence(2 * SECOND);
        audio.extend(tone(SECOND, 0.3));
        audio.extend(silence(2 * SECOND));
        let trimmed = trim_silence(&audio).unwrap();
        let pad = PAD_FRAMES * FRAME_LEN;
        assert!(
            trimmed.len() >= SECOND,
            "die Sprache bleibt vollständig erhalten"
        );
        assert!(
            trimmed.len() <= SECOND + 2 * pad + 2 * FRAME_LEN,
            "vorn und hinten bleibt nur die Polsterung von der Stille"
        );
    }

    #[test]
    fn loud_click_does_not_hide_quiet_speech() {
        // Lauter Klick (Startklang), dann leise Sprache: muss erkannt werden.
        let mut audio = silence(SECOND);
        audio.extend(tone(FRAME_LEN * 3, 0.9));
        audio.extend(silence(SECOND / 2));
        audio.extend(tone(3 * SECOND, 0.03));
        audio.extend(silence(SECOND));
        let trimmed = trim_silence(&audio).unwrap();
        assert!(
            trimmed.len() >= 3 * SECOND,
            "die leise Sprache nach dem Klick bleibt erhalten"
        );
    }

    #[test]
    fn key_click_after_a_pause_is_cut_off() {
        // Sprache, 1,5 s Pause, dann der Klick der fn-Taste.
        let mut audio = silence(SECOND);
        audio.extend(tone(2 * SECOND, 0.2));
        audio.extend(silence(3 * SECOND / 2));
        audio.extend(tone(FRAME_LEN * 2, 0.6));
        audio.extend(silence(FRAME_LEN * 5));
        let trimmed = trim_silence(&audio).unwrap();
        let pad = PAD_FRAMES * FRAME_LEN;
        assert!(
            trimmed.len() <= 2 * SECOND + 2 * pad + 2 * FRAME_LEN,
            "der Tastenklick nach der Pause wird abgeschnitten"
        );
    }

    #[test]
    fn parts_end_in_the_first_pause_after_ten_seconds() {
        let mut audio = tone(5 * SECOND, 0.2);
        audio.extend(silence(SECOND / 2)); // zu früh
        audio.extend(tone(6 * SECOND, 0.2));
        audio.extend(silence(SECOND / 2)); // hier
        audio.extend(tone(2 * SECOND, 0.2));
        let rate = SAMPLE_RATE;
        let cut = part_end(&audio, rate, PartTiming::default()).unwrap();
        let pause_start = 5 * SECOND + SECOND / 2 + 6 * SECOND;
        assert!(
            (pause_start..pause_start + SECOND / 2).contains(&cut),
            "{cut}"
        );
        assert_eq!(
            part_end(&audio[..9 * SECOND], rate, PartTiming::default()),
            None
        );
    }

    #[test]
    fn long_pause_after_five_seconds_ends_a_part_early() {
        let mut audio = tone(3 * SECOND, 0.2);
        audio.extend(silence(SECOND)); // lang, aber vor 5 s
        audio.extend(tone(3 * SECOND, 0.2));
        audio.extend(silence(SECOND / 2)); // kurz: vor 10 s nicht genug
        audio.extend(tone(SECOND, 0.2));
        audio.extend(silence(SECOND)); // lang, nach 5 s: hier
        audio.extend(tone(SECOND, 0.2));
        let cut = part_end(&audio, SAMPLE_RATE, PartTiming::default()).unwrap();
        let long_pause = 3 * SECOND + SECOND + 3 * SECOND + SECOND / 2 + SECOND;
        assert!((long_pause..long_pause + SECOND).contains(&cut), "{cut}");
    }

    #[test]
    fn parts_wait_for_a_pause_until_the_maximum() {
        let speech = tone(20 * SECOND, 0.2);
        assert_eq!(part_end(&speech, SAMPLE_RATE, PartTiming::default()), None);
        let mut long = tone(26 * SECOND, 0.2);
        // Eine Pause ganz am Ende zählt nicht (sie könnte weitergehen).
        long.extend(silence(SECOND / 10));
        let cut = part_end(&long, SAMPLE_RATE, PartTiming::default()).unwrap();
        assert!(
            cut >= 10 * SECOND && cut <= long.len(),
            "ohne Pause endet der Teil spätestens am Ende, nicht vor 10 s"
        );
    }

    #[test]
    fn later_parts_wait_for_their_seconds() {
        let after = |seconds| PartTiming::After(PartSeconds::try_from(seconds).unwrap());
        // Sprache, lange Pause nach 6 s, Sprache bis 20 s, lange Pause, Sprache.
        let mut audio = tone(6 * SECOND, 0.2);
        audio.extend(silence(SECOND));
        audio.extend(tone(13 * SECOND, 0.2));
        audio.extend(silence(SECOND));
        audio.extend(tone(2 * SECOND, 0.2));
        let early = part_end(&audio, SAMPLE_RATE, after(5)).unwrap();
        assert!((6 * SECOND..7 * SECOND).contains(&early), "{early}");
        let late = part_end(&audio, SAMPLE_RATE, after(15)).unwrap();
        let second_pause = 20 * SECOND;
        assert!(
            (second_pause..second_pause + SECOND).contains(&late),
            "{late}"
        );
        assert_eq!(
            part_end(&audio[..19 * SECOND], SAMPLE_RATE, after(20)),
            None
        );
    }

    #[test]
    fn late_parts_end_before_the_model_limit() {
        // 20 s eingestellt, aber keine Pause: Schnitt spätestens bei 28 s.
        let after = PartTiming::After(PartSeconds::try_from(20).unwrap());
        let speech = tone(27 * SECOND, 0.2);
        assert_eq!(part_end(&speech, SAMPLE_RATE, after), None);
        let mut long = tone(29 * SECOND, 0.2);
        long.extend(silence(SECOND / 10));
        let cut = part_end(&long, SAMPLE_RATE, after).unwrap();
        assert!(
            (20 * SECOND..long.len()).contains(&cut),
            "der Zwangsschnitt liegt nach den eingestellten 20 s: {cut}"
        );
        assert!(
            cut < MAX_CHUNK_SAMPLES,
            "der Teil passt am Stück ins Modell: {cut}"
        );
    }

    #[test]
    fn at_end_never_cuts() {
        let mut audio = tone(40 * SECOND, 0.2);
        audio.extend(silence(SECOND));
        audio.extend(tone(SECOND, 0.2));
        assert_eq!(part_end(&audio, SAMPLE_RATE, PartTiming::AtEnd), None);
    }

    #[test]
    fn part_seconds_are_checked() {
        assert_eq!(PartSeconds::try_from(0), Err(PartSecondsError(0)));
        assert_eq!(PartSeconds::try_from(21), Err(PartSecondsError(21)));
        assert_eq!(PartSeconds::try_from(20).map(PartSeconds::get), Ok(20));
        assert_eq!(PartTiming::try_from(0), Ok(PartTiming::AtEnd));
        assert_eq!(u8::from(PartTiming::default()), 5);
        serde_json::from_str::<PartTiming>("21").unwrap_err();
        assert_eq!(
            serde_json::from_str::<PartTiming>("0").unwrap(),
            PartTiming::AtEnd
        );
    }

    #[test]
    fn continuous_speech_is_kept() {
        let audio = tone(10 * SECOND, 0.2);
        let trimmed = trim_silence(&audio).unwrap();
        assert_eq!(trimmed.len(), audio.len());
    }

    #[test]
    fn short_audio_stays_one_chunk() {
        let audio = tone(5 * SECOND, 0.3);
        let chunks = split(&audio, MAX_CHUNK_SAMPLES);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), audio.len());
    }

    #[test]
    fn long_audio_is_cut_at_the_pause() {
        // 25 s Sprache, 1 s Pause, 25 s Sprache: der Schnitt muss in die Pause.
        let mut audio = tone(25 * SECOND, 0.3);
        audio.extend(silence(SECOND));
        audio.extend(tone(25 * SECOND, 0.3));
        let chunks = split(&audio, MAX_CHUNK_SAMPLES);
        assert_eq!(chunks.len(), 2);
        let first_len = chunks[0].len();
        assert!(
            (25 * SECOND..=26 * SECOND).contains(&first_len),
            "der Schnitt liegt in der Pause"
        );
        assert_eq!(
            chunks.iter().map(|chunk| chunk.len()).sum::<usize>(),
            audio.len()
        );
    }

    #[test]
    fn every_chunk_respects_the_maximum() {
        let audio = tone(95 * SECOND, 0.3);
        let chunks = split(&audio, MAX_CHUNK_SAMPLES);
        assert!(
            chunks.iter().all(|chunk| chunk.len() <= MAX_CHUNK_SAMPLES),
            "kein Abschnitt ist länger als das Maximum"
        );
        assert_eq!(
            chunks.iter().map(|chunk| chunk.len()).sum::<usize>(),
            audio.len()
        );
    }
}
