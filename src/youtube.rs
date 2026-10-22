//! `YouTube`: Link prüfen, Titel und Sprache abfragen, Tonspur laden.
//!
//! Die Tonspur fließt ohne Zwischendatei von `yt-dlp` direkt in `ffmpeg`,
//! das sie in 16 kHz mono `f32` wandelt. Beide Prozesse lassen sich über
//! ein gemeinsames Abbruch-Flag beenden.
//!
//! yt-dlp bekommt die Pfade zu `ffmpeg` und `deno` ausdrücklich: Eine per
//! Doppelklick oder beim Anmelden gestartete App hat nur den `PATH` von
//! macOS, ohne Homebrew. Ohne ffmpeg scheitern Bereiche, ohne
//! JavaScript-Laufzeit (deno) löst yt-dlp die Prüfungen von `YouTube` nicht,
//! und `YouTube` verweigert viele Formate mit HTTP 403.

use crate::note::Explain;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use serde::Deserialize;

use crate::audio::decode::samples_from_f32le;
use crate::audio::range::TimeRange;
use crate::audio::{Pcm16k, SAMPLE_RATE};
use crate::clock::{Clock, SystemClock};
use crate::language::Language;
use crate::paths;
use crate::process::{self, Limit, ProcessError, Running, last_line};
use crate::text::Chapter;

/// Wie oft während des Ladens auf Abbruch und Ende geprüft wird.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Länger darf die Abfrage von Titel und Sprache nicht dauern.
const INFO_TIMEOUT: Duration = Duration::from_secs(60);

/// Kommt so lange keine Tonspur an, gilt der Download als hängend und wird
/// beendet. Keine Grenze für die ganze Dauer: Ein langes Video darf lange
/// laden, solange etwas ankommt.
const STALL_LIMIT: Duration = Duration::from_mins(2);

/// So lange darf yt-dlp nach dem Ende von ffmpeg noch brauchen; danach
/// liest niemand mehr, was es liefert.
const DOWNLOADER_GRACE: Duration = Duration::from_secs(5);

/// Hosts, deren Links als YouTube-Video gelten.
const HOSTS: [&str; 5] = [
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "music.youtube.com",
    "youtu.be",
];

/// Ein geprüfter YouTube-Link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoUrl(String);

impl VideoUrl {
    /// Der Link als Text.
    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Text, der kein YouTube-Video-Link ist.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("kein YouTube-Link in der Zwischenablage")]
pub(crate) struct NotAVideoLink;

impl Explain for NotAVideoLink {
    fn english(&self) -> String {
        "no YouTube link on the clipboard".to_owned()
    }
}

impl FromStr for VideoUrl {
    type Err = NotAVideoLink;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let link = text.trim();
        let without_scheme = link
            .strip_prefix("https://")
            .or_else(|| link.strip_prefix("http://"))
            .unwrap_or(link);
        let (host, path) = without_scheme.split_once('/').ok_or(NotAVideoLink)?;
        if !HOSTS.contains(&host) || link.contains(char::is_whitespace) {
            return Err(NotAVideoLink);
        }
        let is_video = match host {
            "youtu.be" => !path.is_empty(),
            _ => {
                path.starts_with("watch?")
                    || path.starts_with("shorts/")
                    || path.starts_with("live/")
            }
        };
        if !is_video {
            return Err(NotAVideoLink);
        }
        Ok(Self(format!("https://{host}/{path}")))
    }
}

/// Fehler beim Laden von `YouTube`.
#[derive(Debug, thiserror::Error)]
pub(crate) enum YoutubeError {
    /// Ein Werkzeug ist nicht installiert.
    #[error("{0} fehlt (im Terminal: brew install {0})")]
    MissingTool(&'static str),
    /// Ein Werkzeug ließ sich nicht starten.
    #[error("{tool} konnte nicht gestartet werden (installiert? `brew install {tool}`)")]
    Spawn {
        /// `yt-dlp` oder `ffmpeg`.
        tool: &'static str,
        /// Ursache.
        #[source]
        source: io::Error,
    },
    /// `YouTube` verweigert den Download (HTTP 403, Bot-Prüfung, zu viele
    /// Anfragen).
    #[error("YouTube verweigert den Download ({}) – {}", .reason.german(), .hint.german())]
    Refused {
        /// Was `YouTube` gemeldet hat.
        reason: Refusal,
        /// Was meist hilft.
        hint: RefusalHint,
    },
    /// Ein Werkzeug lief, meldete aber einen Fehler.
    #[error("{tool}: {}", .message.as_deref().unwrap_or("unbekannter Fehler"))]
    Failed {
        /// `yt-dlp` oder `ffmpeg`.
        tool: &'static str,
        /// Fehlermeldung des Werkzeugs; `None`, wenn es nichts ausgab.
        message: Option<String>,
    },
    /// Ein Werkzeug antwortete nicht in der Zeitgrenze.
    #[error("{0} antwortet nicht")]
    TimedOut(&'static str),
    /// Warten auf einen Prozess oder Lesen seiner Ausgabe schlug fehl.
    #[error("Prozess: {0}")]
    Process(#[source] io::Error),
    /// Ein Thread, der die Ausgaben las, ist abgestürzt.
    #[error("Lese-Thread abgestürzt")]
    Crashed,
    /// Vom Nutzer abgebrochen.
    #[error("abgebrochen")]
    Cancelled,
    /// Zwei Minuten kam keine Tonspur an; der Download wurde beendet.
    #[error("yt-dlp liefert seit 2 Minuten nichts mehr – abgebrochen")]
    Stalled,
    /// yt-dlp lieferte zum Video keinen Titel (leere Antwort).
    #[error("yt-dlp lieferte keinen Titel")]
    NoTitle,
}

impl Explain for YoutubeError {
    fn english(&self) -> String {
        match self {
            Self::MissingTool(tool) => {
                format!("{tool} is missing (in Terminal: brew install {tool})")
            }
            Self::Spawn { tool, source: _ } => {
                format!("{tool} could not be started (installed? `brew install {tool}`)")
            }
            Self::Refused { reason, hint } => format!(
                "YouTube refuses the download ({}) – {}",
                reason.english(),
                hint.english()
            ),
            Self::Failed { tool, message } => {
                format!("{tool}: {}", message.as_deref().unwrap_or("unknown error"))
            }
            Self::TimedOut(tool) => format!("{tool} is not responding"),
            Self::Process(source) => format!("process: {source}"),
            Self::Crashed => "reading thread crashed".to_owned(),
            Self::Cancelled => "cancelled".to_owned(),
            Self::NoTitle => "yt-dlp returned no title".to_owned(),
            Self::Stalled => "yt-dlp has delivered nothing for 2 minutes – stopped".to_owned(),
        }
    }
}

/// Womit `YouTube` einen Download verweigert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// HTTP 403.
    Forbidden,
    /// HTTP 429: zu viele Anfragen.
    TooManyRequests,
    /// „Sign in to confirm you’re not a bot“.
    BotCheck,
}

impl Refusal {
    /// Deutsch für die Meldung.
    #[must_use]
    pub(crate) const fn german(self) -> &'static str {
        match self {
            Self::Forbidden => "HTTP 403",
            Self::TooManyRequests => "zu viele Anfragen",
            Self::BotCheck => "Bot-Prüfung",
        }
    }

    /// Englisch für die Meldung.
    #[must_use]
    pub(crate) const fn english(self) -> &'static str {
        match self {
            Self::Forbidden => "HTTP 403",
            Self::TooManyRequests => "too many requests",
            Self::BotCheck => "bot check",
        }
    }
}

/// Was nach einer Verweigerung meist hilft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefusalHint {
    /// yt-dlp aktualisieren (Deno ist da).
    UpgradeYtDlp,
    /// Deno fehlt; yt-dlp braucht es für die Prüfung von `YouTube`.
    InstallDeno,
}

impl RefusalHint {
    /// Deutsch für die Meldung.
    #[must_use]
    pub(crate) const fn german(self) -> &'static str {
        match self {
            Self::UpgradeYtDlp => "oft hilft „brew upgrade yt-dlp“, sonst später erneut versuchen",
            Self::InstallDeno => "deno fehlt: „brew install deno“",
        }
    }

    /// Englisch für die Meldung.
    #[must_use]
    pub(crate) const fn english(self) -> &'static str {
        match self {
            Self::UpgradeYtDlp => "“brew upgrade yt-dlp” often helps, otherwise try again later",
            Self::InstallDeno => "deno is missing: “brew install deno”",
        }
    }
}

/// Titel, Sprache, Länge und Kapitel eines Videos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoInfo {
    /// Titel des Videos.
    pub title: String,
    /// Sprache laut `YouTube` (ISO-Code), falls bekannt.
    pub language: Option<String>,
    /// Länge, falls bekannt (bei Livestreams nicht).
    pub duration: Option<Duration>,
    /// Kapitel, falls das Video welche hat.
    pub chapters: Vec<Chapter>,
}

/// Ein Kapitel, wie yt-dlp es als JSON meldet. Beide Felder können fehlen
/// oder `null` sein; ein solches Kapitel fällt allein weg, nicht alle.
#[derive(Debug, Deserialize)]
struct RawChapter {
    #[serde(default)]
    start_time: Option<f64>,
    #[serde(default)]
    title: Option<String>,
}

impl VideoInfo {
    /// Sprache fürs Modell: die von `YouTube` gemeldete, wenn das Modell sie
    /// kennt, sonst Englisch.
    #[must_use]
    pub(crate) fn model_language(&self) -> Language {
        self.language
            .as_deref()
            .and_then(Language::from_code)
            .unwrap_or(Language::English)
    }
}

/// Fragt Titel, Sprache, Länge und Kapitel ab, ohne etwas herunterzuladen.
/// Wird `cancel` gesetzt, endet die Abfrage mit [`YoutubeError::Cancelled`].
///
/// # Errors
/// [`YoutubeError`], wenn yt-dlp fehlt, das Video nicht abrufbar ist, nicht
/// rechtzeitig antwortet oder abgebrochen wird.
pub(crate) fn fetch_info(url: &VideoUrl, cancel: &AtomicBool) -> Result<VideoInfo, YoutubeError> {
    let mut command = yt_dlp()?;
    command
        .args([
            "--no-playlist",
            "--skip-download",
            "--no-warnings",
            "--print",
            "%(title)s",
            "--print",
            "%(language)s",
            "--print",
            "%(duration)s",
            "--print",
            "%(chapters)j",
        ])
        .arg(url.as_str());
    let output = process::run(
        &mut command,
        b"",
        Limit::AfterOrCancel(INFO_TIMEOUT, cancel),
    )
    .map_err(|error| match error {
        ProcessError::Spawn(source) => YoutubeError::Spawn {
            tool: "yt-dlp",
            source,
        },
        ProcessError::Io(source) => YoutubeError::Process(source),
        ProcessError::TimedOut => YoutubeError::TimedOut("yt-dlp"),
        ProcessError::Cancelled => YoutubeError::Cancelled,
        ProcessError::Crashed => YoutubeError::Crashed,
    })?;
    if !output.status.success() {
        return Err(downloader_failure(&output.stderr));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let title = lines
        .next()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or(YoutubeError::NoTitle)?
        .to_owned();
    let language = lines
        .next()
        .map(str::trim)
        .filter(|code| !code.is_empty() && *code != "NA")
        .map(str::to_owned);
    let duration = lines
        .next()
        .and_then(|seconds| seconds.trim().parse::<f64>().ok())
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok());
    let chapters = lines.next().map(parse_chapters).unwrap_or_default();
    Ok(VideoInfo {
        title,
        language,
        duration,
        chapters,
    })
}

/// Kapitel aus der JSON-Zeile von yt-dlp; ohne Kapitel („NA“, `null`)
/// oder bei Unlesbarem keine.
fn parse_chapters(line: &str) -> Vec<Chapter> {
    serde_json::from_str::<Option<Vec<RawChapter>>>(line.trim())
        .ok()
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|raw| {
            Some(Chapter {
                start: Duration::try_from_secs_f64(raw.start_time?).ok()?,
                title: raw.title?,
            })
        })
        .filter(|chapter| !chapter.title.trim().is_empty())
        .collect()
}

/// Lädt die Tonspur und wandelt sie in 16 kHz mono; mit `range` nur diesen
/// Bereich.
///
/// Wird `cancel` gesetzt, werden beide Prozesse beendet und
/// [`YoutubeError::Cancelled`] zurückgegeben. Beide laufen in einer eigenen
/// Prozessgruppe, die am Ende auf jedem Weg beendet wird: Für einen Bereich
/// startet yt-dlp selbst ein ffmpeg, das seine Ausgaben erbt.
///
/// # Errors
/// [`YoutubeError`], wenn ein Werkzeug fehlt, scheitert oder abgebrochen wird.
pub(crate) fn fetch_audio(
    url: &VideoUrl,
    range: Option<&TimeRange>,
    cancel: &AtomicBool,
) -> Result<Pcm16k, YoutubeError> {
    let decoder_path = tool("ffmpeg")?;
    let mut command = yt_dlp()?;
    let mut downloader = process::spawn(
        command
            .args([
                "--no-playlist",
                "--quiet",
                "--no-warnings",
                "--no-progress",
                "-f",
                "bestaudio/best",
                "-o",
                "-",
            ])
            .args(range.map(section_args).unwrap_or_default())
            .arg(url.as_str())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .map_err(|source| YoutubeError::Spawn {
        tool: "yt-dlp",
        source,
    })?;
    let audio_stream = downloader
        .take_stdout()
        .map_or_else(Stdio::null, Stdio::from);
    // Scheitert der Start, beendet `downloader` beim Wegfallen yt-dlp.
    let decoder = process::spawn(
        process::command(decoder_path)
            .args([
                "-v", "error", "-i", "pipe:0", "-f", "f32le", "-ac", "1", "-ar",
            ])
            .arg(SAMPLE_RATE.to_string())
            .arg("-")
            .stdin(audio_stream)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .map_err(|source| YoutubeError::Spawn {
        tool: "ffmpeg",
        source,
    })?;
    collect(downloader, decoder, cancel)
}

/// Liest die Tonspur aus der laufenden Kette `downloader` → `decoder`, bis
/// ffmpeg fertig ist oder `cancel` gesetzt wird. Beendet danach auf jedem
/// Weg beide Prozessgruppen und sammelt beide Prozesse ein.
fn collect(
    mut downloader: Running,
    mut decoder: Running,
    cancel: &AtomicBool,
) -> Result<Pcm16k, YoutubeError> {
    let received = AtomicUsize::new(0);
    let watch = Watch {
        cancel,
        received: &received,
        clock: &SystemClock,
    };
    let streams = Streams::take(&mut downloader, &mut decoder);
    let Streams {
        pcm,
        downloader_errors: downloader_stderr,
        decoder_errors: decoder_stderr,
    } = streams;
    thread::scope(|scope| {
        let pcm = pcm.map(|stream| Counting {
            stream,
            received: &received,
        });
        let readers = process::reader(scope, "youtube-pcm", pcm).and_then(|pcm| {
            Ok((
                pcm,
                process::reader(scope, "youtube-yt-dlp", downloader_stderr)?,
                process::reader(scope, "youtube-ffmpeg", decoder_stderr)?,
            ))
        });
        let (pcm_bytes, downloader_errors, decoder_errors) = match readers {
            Ok(readers) => readers,
            Err(error) => {
                // Schon gestartete Lese-Threads enden mit den Prozessen.
                downloader.stop();
                decoder.stop();
                return Err(YoutubeError::Process(error));
            }
        };

        let finished = wait_for(&mut downloader, &mut decoder, &watch);
        // Auf jedem Weg, auch nach einem Ende von selbst: Sonst warten die
        // Lese-Threads auf Enkel (das ffmpeg von yt-dlp), die die Ausgaben
        // geerbt haben.
        downloader.stop();
        decoder.stop();

        let joined = |reader| process::joined(reader).map_err(|_crashed| YoutubeError::Crashed);
        let pcm_bytes = joined(pcm_bytes);
        // Die Fehlerausgaben sind nur für die Meldung: Fehlen sie, steht
        // dort „unbekannter Fehler“.
        let downloader_errors = joined(downloader_errors)?.unwrap_or_default();
        let decoder_errors = joined(decoder_errors)?.unwrap_or_default();
        // Ein abgebrochenes Lesen der Tonspur hieße ein still gekürztes
        // Transkript.
        let pcm_bytes = pcm_bytes?.map_err(YoutubeError::Process)?;
        match finished? {
            Outcome::Cancelled => Err(YoutubeError::Cancelled),
            Outcome::Stalled => Err(YoutubeError::Stalled),
            Outcome::DownloaderFailed => Err(downloader_failure(&downloader_errors)),
            Outcome::DecoderFailed => Err(YoutubeError::Failed {
                tool: "ffmpeg",
                message: last_line(&decoder_errors),
            }),
            Outcome::Success => Ok(Pcm16k::from_samples(samples_from_f32le(&pcm_bytes))),
        }
    })
}

/// Nur einen Bereich laden, mit `--force-keyframes-at-cuts`, damit er
/// genau beginnt (gemessen: `docs/grenzen.md`).
fn section_args(range: &TimeRange) -> Vec<String> {
    vec![
        "--download-sections".to_owned(),
        range.download_section(),
        "--force-keyframes-at-cuts".to_owned(),
    ]
}

/// Die Ausgabekanäle der beiden Prozesse.
struct Streams {
    pcm: Option<std::process::ChildStdout>,
    downloader_errors: Option<std::process::ChildStderr>,
    decoder_errors: Option<std::process::ChildStderr>,
}

impl Streams {
    /// Übernimmt die Kanäle, damit sie auf eigenen Threads gelesen werden
    /// können (sonst laufen die Puffer voll und die Prozesse blockieren).
    const fn take(downloader: &mut Running, decoder: &mut Running) -> Self {
        Self {
            pcm: decoder.take_stdout(),
            downloader_errors: downloader.take_stderr(),
            decoder_errors: decoder.take_stderr(),
        }
    }
}

/// yt-dlp mit den Pfaden seiner Helfer (siehe oben).
fn yt_dlp() -> Result<Command, YoutubeError> {
    let mut command = process::command(tool("yt-dlp")?);
    // Ein Ordner: Dort findet yt-dlp auch ffprobe (für genaue Bereiche).
    if let Some(dir) = paths::tool("ffmpeg").as_deref().and_then(Path::parent) {
        command.arg("--ffmpeg-location").arg(dir);
    }
    if let Some(deno) = paths::tool("deno") {
        command
            .arg("--js-runtimes")
            .arg(format!("deno:{}", deno.display()));
    }
    Ok(command)
}

/// Fehler aus der Ausgabe von yt-dlp: kurz und mit Hinweis, wenn `YouTube`
/// den Download verweigert; sonst die letzte Zeile ohne Vorspann.
fn downloader_failure(stderr: &[u8]) -> YoutubeError {
    let line = last_line(stderr);
    let text = line.as_deref().unwrap_or_default();
    let reason = if text.contains("HTTP Error 403") {
        Some(Refusal::Forbidden)
    } else if text.contains("HTTP Error 429") {
        Some(Refusal::TooManyRequests)
    } else if text.contains("not a bot") {
        Some(Refusal::BotCheck)
    } else {
        None
    };
    reason.map_or_else(
        || YoutubeError::Failed {
            tool: "yt-dlp",
            message: line
                .as_deref()
                .map(|line| without_preamble(line).to_owned()),
        },
        |reason| YoutubeError::Refused {
            reason,
            hint: if paths::tool("deno").is_some() {
                RefusalHint::UpgradeYtDlp
            } else {
                RefusalHint::InstallDeno
            },
        },
    )
}

/// Eine Fehlerzeile von yt-dlp ohne „`ERROR:`“ und „`[youtube] abc123:`“
/// davor.
fn without_preamble(line: &str) -> &str {
    let line = line.strip_prefix("ERROR:").unwrap_or(line).trim_start();
    match line.strip_prefix('[').and_then(|rest| rest.split_once(']')) {
        Some((_, rest)) => rest
            .split_once(": ")
            .map_or(rest, |(_, message)| message)
            .trim_start(),
        None => line,
    }
}

/// Pfad eines Werkzeugs, oder [`YoutubeError::MissingTool`].
fn tool(name: &'static str) -> Result<PathBuf, YoutubeError> {
    paths::tool(name).ok_or(YoutubeError::MissingTool(name))
}

/// Wie das Laden endete.
enum Outcome {
    /// Beide Prozesse erfolgreich.
    Success,
    /// yt-dlp meldete einen Fehler.
    DownloaderFailed,
    /// ffmpeg meldete einen Fehler.
    DecoderFailed,
    /// Abgebrochen, beide Prozesse beendet.
    Cancelled,
    /// Zu lange kam keine Tonspur an; beide Prozesse beendet.
    Stalled,
}

/// Wartet, bis ffmpeg fertig ist, oder bricht beide Prozesse ab.
///
/// Wer zuerst scheiterte, ist schuld: Endet ffmpeg mit einem Fehler,
/// während yt-dlp noch liefert, scheitert yt-dlp danach nur am
/// geschlossenen Rohr („Broken pipe“); gemeldet wird dann ffmpeg.
///
/// Kommt [`STALL_LIMIT`] lang keine Tonspur an, endet beides als
/// [`Outcome::Stalled`].
fn wait_for(
    downloader: &mut Running,
    decoder: &mut Running,
    watch: &Watch<'_, impl Clock>,
) -> Result<Outcome, YoutubeError> {
    let mut downloader_failed_first = false;
    let mut last_received = watch.received.load(Ordering::Relaxed);
    let mut last_data = watch.clock.now();
    loop {
        if watch.cancel.load(Ordering::Relaxed) {
            downloader.stop();
            decoder.stop();
            return Ok(Outcome::Cancelled);
        }
        let received = watch.received.load(Ordering::Relaxed);
        if received == last_received {
            if watch.clock.now().saturating_duration_since(last_data) >= STALL_LIMIT {
                downloader.stop();
                decoder.stop();
                return Ok(Outcome::Stalled);
            }
        } else {
            last_received = received;
            last_data = watch.clock.now();
        }
        if let Some(decoder_status) = decoder.try_wait().map_err(YoutubeError::Process)? {
            let downloader_status = downloader_after_decoder(downloader, watch)?;
            return Ok(match downloader_status {
                DownloaderEnd::Cancelled => Outcome::Cancelled,
                DownloaderEnd::Exited(_) | DownloaderEnd::Stopped
                    if !decoder_status.success() && !downloader_failed_first =>
                {
                    Outcome::DecoderFailed
                }
                DownloaderEnd::Exited(status) if !status.success() => Outcome::DownloaderFailed,
                DownloaderEnd::Exited(_) if !decoder_status.success() => Outcome::DecoderFailed,
                DownloaderEnd::Exited(_) => Outcome::Success,
                // ffmpeg ist fertig, yt-dlp lieferte aber noch.
                DownloaderEnd::Stopped => Outcome::DownloaderFailed,
            });
        }
        if !downloader_failed_first
            && let Some(status) = downloader.try_wait().map_err(YoutubeError::Process)?
        {
            downloader_failed_first = !status.success();
        }
        watch.clock.sleep(POLL_INTERVAL);
    }
}

/// Worauf [`wait_for`] achtet.
struct Watch<'a, C> {
    /// Abbrechen.
    cancel: &'a AtomicBool,
    /// So viele Bytes der Tonspur sind angekommen ([`Counting`]).
    received: &'a AtomicUsize,
    /// Die Uhr für Hänger und Gnadenfrist.
    clock: &'a C,
}

/// Ein Strom, der mitzählt, wie viel schon gelesen ist.
struct Counting<'a, R> {
    stream: R,
    received: &'a AtomicUsize,
}

impl<R: io::Read> io::Read for Counting<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.stream.read(buf)?;
        self.received.fetch_add(read, Ordering::Relaxed);
        Ok(read)
    }
}

/// Wie yt-dlp nach dem Ende von ffmpeg endete.
enum DownloaderEnd {
    /// Von selbst.
    Exited(ExitStatus),
    /// Lief nach [`DOWNLOADER_GRACE`] noch und wurde beendet.
    Stopped,
    /// Abgebrochen und beendet.
    Cancelled,
}

/// Wartet nach dem Ende von ffmpeg kurz auf yt-dlp. Ohne ffmpeg liest
/// niemand mehr, was es liefert; ein hängendes yt-dlp (Netz, Wiederholungen)
/// hielte sonst den Auftrag fest, ohne dass „Abbrechen“ wirkt.
fn downloader_after_decoder(
    downloader: &mut Running,
    watch: &Watch<'_, impl Clock>,
) -> Result<DownloaderEnd, YoutubeError> {
    let started = watch.clock.now();
    loop {
        if let Some(status) = downloader.try_wait().map_err(YoutubeError::Process)? {
            return Ok(DownloaderEnd::Exited(status));
        }
        if watch.cancel.load(Ordering::Relaxed) {
            downloader.stop();
            return Ok(DownloaderEnd::Cancelled);
        }
        if watch.clock.now().saturating_duration_since(started) > DOWNLOADER_GRACE {
            downloader.stop();
            return Ok(DownloaderEnd::Stopped);
        }
        watch.clock.sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_common_video_links() {
        for link in [
            "https://www.youtube.com/watch?v=abc123",
            "http://youtube.com/watch?v=abc123&t=42",
            "https://youtu.be/abc123",
            "youtu.be/abc123",
            "https://m.youtube.com/watch?v=abc123",
            "https://www.youtube.com/shorts/abc123",
            "https://www.youtube.com/live/abc123",
            "  https://www.youtube.com/watch?v=abc123\n",
        ] {
            assert!(link.parse::<VideoUrl>().is_ok(), "{link}");
        }
    }

    #[test]
    fn rejects_other_text() {
        for text in [
            "",
            "Hallo",
            "https://example.com/watch?v=abc",
            "https://www.youtube.com/",
            "https://www.youtube.com/@kanal",
            "https://youtu.be/",
            "https://www.youtube.com/watch?v=a b",
        ] {
            assert!(text.parse::<VideoUrl>().is_err(), "{text}");
        }
    }

    #[test]
    fn reads_chapters_from_yt_dlp() {
        let chapters = parse_chapters(
            r#"[{"start_time": 0, "title": "Intro", "end_time": 5}, {"start_time": 5.5, "title": "The cool thing", "end_time": 17}]"#,
        );
        assert_eq!(
            chapters,
            [
                Chapter {
                    start: Duration::ZERO,
                    title: "Intro".to_owned()
                },
                Chapter {
                    start: Duration::from_millis(5_500),
                    title: "The cool thing".to_owned()
                },
            ]
        );
        assert!(
            parse_chapters("NA").is_empty(),
            "„NA“ bedeutet keine Kapitel"
        );
        assert!(
            parse_chapters("null").is_empty(),
            "„null“ bedeutet keine Kapitel"
        );
        assert_eq!(
            parse_chapters(
                r#"[{"start_time": 0, "title": null}, {"start_time": 5.5, "title": "Rest"}]"#
            ),
            [Chapter {
                start: Duration::from_millis(5_500),
                title: "Rest".to_owned()
            }],
            "ein Kapitel ohne Titel nimmt die anderen nicht mit"
        );
    }

    #[test]
    fn language_defaults_to_english() {
        let info = |language: Option<&str>| VideoInfo {
            title: String::new(),
            language: language.map(str::to_owned),
            duration: None,
            chapters: Vec::new(),
        };
        assert_eq!(info(Some("de")).model_language(), Language::German);
        assert_eq!(info(Some("de-DE")).model_language(), Language::German);
        assert_eq!(info(Some("en")).model_language(), Language::English);
        assert_eq!(info(Some("fr")).model_language(), Language::French);
        assert_eq!(info(Some("tr")).model_language(), Language::English);
        assert_eq!(info(None).model_language(), Language::English);
    }

    /// [`wait_for`] mit der Uhr des Systems, ohne dass Tonspur ankommt.
    fn waited(downloader: &mut Running, decoder: &mut Running, cancel: &AtomicBool) -> Outcome {
        let received = AtomicUsize::new(0);
        let watch = Watch {
            cancel,
            received: &received,
            clock: &SystemClock,
        };
        wait_for(downloader, decoder, &watch).unwrap()
    }

    #[test]
    fn a_download_without_data_stops_after_the_stall_limit() {
        let (mut downloader, mut decoder) = (shell("sleep 30"), shell("sleep 30"));
        let received = AtomicUsize::new(0);
        let clock = crate::clock::FakeClock::new();
        let watch = Watch {
            cancel: &AtomicBool::new(false),
            received: &received,
            clock: &clock,
        };
        let outcome = wait_for(&mut downloader, &mut decoder, &watch).unwrap();
        assert!(
            matches!(outcome, Outcome::Stalled),
            "hängt, ohne dass etwas kommt"
        );
        assert!(downloader.try_wait().unwrap().is_some(), "yt-dlp beendet");
    }

    /// Ein Shell-Befehl als Prozess.
    fn shell(script: &str) -> Running {
        process::spawn(process::command("/bin/sh").args(["-c", script])).unwrap()
    }

    /// Eine Kette aus zwei Shell-Befehlen wie yt-dlp → ffmpeg.
    fn chain(downloader: &str, decoder: &str) -> (Running, Running) {
        let mut downloader = process::spawn(
            process::command("/bin/sh")
                .args(["-c", downloader])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .unwrap();
        let audio = downloader
            .take_stdout()
            .map_or_else(Stdio::null, Stdio::from);
        let decoder = process::spawn(
            process::command("/bin/sh")
                .args(["-c", decoder])
                .stdin(audio)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .unwrap();
        (downloader, decoder)
    }

    #[test]
    fn cancel_does_not_wait_for_a_grandchild_of_the_downloader() {
        // Wie yt-dlp mit seinem ffmpeg für einen Bereich: Der Enkel hält
        // die Ausgaben von yt-dlp; `sleep 8; true` erzwingt einen echten
        // Enkel.
        let (downloader, decoder) = chain("sleep 8; true", "cat > /dev/null");
        let cancel = AtomicBool::new(false);
        let (result, after_cancel) = thread::scope(|scope| {
            let fetching = thread::Builder::new()
                .spawn_scoped(scope, || collect(downloader, decoder, &cancel))
                .unwrap();
            SystemClock.sleep(Duration::from_millis(300));
            cancel.store(true, Ordering::Relaxed);
            let cancelled = SystemClock.now();
            (fetching.join().unwrap(), cancelled.elapsed())
        });
        assert!(
            matches!(result, Err(YoutubeError::Cancelled)),
            "{:?}",
            result.err()
        );
        assert!(
            after_cancel < Duration::from_secs(3),
            "Abbruch erst nach {after_cancel:?} zurück"
        );
    }

    #[test]
    fn a_failing_decoder_reports_its_own_message() {
        let (downloader, decoder) = chain("exit 0", "echo 'Invalid data' >&2; exit 1");
        let error = collect(downloader, decoder, &AtomicBool::new(false)).unwrap_err();
        assert_eq!(error.to_string(), "ffmpeg: Invalid data");
    }

    #[test]
    fn a_finished_chain_does_not_wait_for_a_background_grandchild() {
        // ffmpeg endet erfolgreich, ein Hintergrundprozess behält seine
        // Ausgabe: Der Auftrag ist trotzdem gleich fertig.
        let (downloader, decoder) = chain("exit 0", "sleep 8 & exit 0");
        let started = SystemClock.now();
        let result = collect(downloader, decoder, &AtomicBool::new(false));
        assert!(result.is_ok(), "{:?}", result.err());
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "erst nach {:?} fertig",
            started.elapsed()
        );
    }

    #[test]
    fn a_failing_decoder_is_not_blamed_on_the_downloader() {
        // ffmpeg scheitert, yt-dlp liefert weiter und scheitert dann am
        // geschlossenen Rohr, wie Python es meldet.
        let (mut downloader, mut decoder) = chain(
            "trap '' PIPE; while echo audio; do :; done 2>/dev/null; \
             echo 'ERROR: unable to write data: [Errno 32] Broken pipe' >&2; exit 1",
            "head -c 100 >/dev/null; echo 'Invalid data found' >&2; exit 1",
        );
        let outcome = waited(&mut downloader, &mut decoder, &AtomicBool::new(false));
        assert!(
            matches!(outcome, Outcome::DecoderFailed),
            "ffmpeg scheiterte zuerst"
        );
    }

    #[test]
    fn a_failing_downloader_is_reported_even_if_the_decoder_fails_after_it() {
        let (mut downloader, mut decoder) = chain(
            "echo 'ERROR: Video unavailable' >&2; exit 1",
            "sleep 0.5; cat >/dev/null; echo 'no input' >&2; exit 1",
        );
        let outcome = waited(&mut downloader, &mut decoder, &AtomicBool::new(false));
        assert!(
            matches!(outcome, Outcome::DownloaderFailed),
            "yt-dlp scheiterte zuerst"
        );
    }

    #[test]
    fn a_hanging_downloader_does_not_hold_a_failed_decoder() {
        let cancel = AtomicBool::new(false);
        let (mut downloader, mut decoder) = (shell("sleep 30"), shell("exit 1"));
        let started = SystemClock.now();
        let outcome = waited(&mut downloader, &mut decoder, &cancel);
        assert!(
            matches!(outcome, Outcome::DecoderFailed),
            "ffmpeg ist gescheitert"
        );
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "yt-dlp wird nach der Gnadenfrist beendet, nicht erst nach 30 s"
        );
        assert!(
            downloader.try_wait().unwrap().is_some(),
            "yt-dlp ist beendet und eingesammelt"
        );
    }

    #[test]
    fn a_downloader_that_ends_soon_after_the_decoder_succeeds() {
        let cancel = AtomicBool::new(false);
        let (mut downloader, mut decoder) = (shell("sleep 0.3"), shell("exit 0"));
        let outcome = waited(&mut downloader, &mut decoder, &cancel);
        assert!(
            matches!(outcome, Outcome::Success),
            "yt-dlp endete innerhalb der Gnadenfrist"
        );
    }

    #[test]
    fn cancel_stops_both_tools() {
        let cancel = AtomicBool::new(true);
        let (mut downloader, mut decoder) = (shell("sleep 30"), shell("sleep 30"));
        let outcome = waited(&mut downloader, &mut decoder, &cancel);
        assert!(matches!(outcome, Outcome::Cancelled), "abgebrochen");
    }

    #[test]
    fn refusals_get_a_short_message() {
        let refused = downloader_failure(
            b"ERROR: [youtube] abc123: unable to download video data: HTTP Error 403: Forbidden\n",
        );
        assert!(
            matches!(
                refused,
                YoutubeError::Refused {
                    reason: Refusal::Forbidden,
                    hint: _
                }
            ),
            "{refused:?}"
        );
        let bot = downloader_failure(
            b"ERROR: [youtube] abc123: Sign in to confirm you\xe2\x80\x99re not a bot.\n",
        );
        assert!(
            matches!(
                bot,
                YoutubeError::Refused {
                    reason: Refusal::BotCheck,
                    hint: _
                }
            ),
            "{bot:?}"
        );
        assert_eq!(
            bot.english(),
            format!(
                "YouTube refuses the download (bot check) – {}",
                hint_english(&bot)
            )
        );
        let other = downloader_failure(b"ERROR: [youtube] abc123: Video unavailable\n");
        assert_eq!(other.to_string(), "yt-dlp: Video unavailable");
        // Eine Altersprüfung ist keine Bot-Prüfung: yt-dlp sagt selbst, was los ist.
        let age = downloader_failure(
            b"ERROR: [youtube] 3JZIIvleyaw: Sign in to confirm your age. This video may be inappropriate for some users.\n",
        );
        assert_eq!(
            age.to_string(),
            "yt-dlp: Sign in to confirm your age. This video may be inappropriate for some users."
        );
    }

    /// Der Hinweis hängt davon ab, ob Deno installiert ist.
    fn hint_english(error: &YoutubeError) -> &'static str {
        let YoutubeError::Refused { hint, reason: _ } = error else {
            panic!("keine Verweigerung: {error:?}");
        };
        hint.english()
    }
}
