//! Was die Steuerung vom Mac braucht: Mikrofon, Zwischenablage, Einfügen,
//! Text um die Schreibmarke und die Anzeige. Hinter einer Schnittstelle,
//! damit Tests die Abläufe ohne Mac-Dienste prüfen können.

use std::path::PathBuf;
use std::time::Duration;

use crate::app::status::UiUpdate;
use crate::audio::Pcm16k;
use crate::audio::chunk::PartTiming;
use crate::audio::record::{self, Captured, Interrupted, RecordError, Recording};
use crate::context::{self, Cursor};
use crate::helper::HelperError;
use crate::paste::{self, PasteError, Pasted};

/// Eine laufende Aufnahme.
pub(crate) trait Microphone {
    /// Der nächste Teil bis zu einer Sprechpause, falls nach `timing`
    /// einer fertig ist.
    ///
    /// # Errors
    /// [`RecordError`], wenn das Mikrofon ausfällt; was bis dahin
    /// aufgenommen war, liefert danach [`Microphone::finish`].
    fn next_part(&mut self, timing: PartTiming) -> Result<Option<Pcm16k>, RecordError>;

    /// Der aktuelle Pegel (Effektivwert), für die Anzeige.
    fn level(&self) -> f32;

    /// Wie lange die Aufnahme ist, die noch in keinem Teil steckt.
    fn unsent(&self) -> Duration;

    /// Beendet die Aufnahme und liefert den Rest.
    ///
    /// # Errors
    /// [`Interrupted`], wenn das Mikrofon ausgefallen ist; mit dem Rest bis
    /// dahin.
    fn finish(self: Box<Self>) -> Result<Captured, Interrupted>;
}

/// Die Dienste des Macs, die die Steuerung nutzt.
pub(crate) trait Platform {
    /// Öffnet das Mikrofon.
    ///
    /// # Errors
    /// [`RecordError`], wenn es kein Mikrofon gibt oder das Recht fehlt.
    fn record(&self) -> Result<Box<dyn Microphone>, RecordError>;

    /// Fügt `text` an der Schreibmarke der vorderen App ein; sagt, ob die
    /// Zwischenablage danach wieder die alte ist.
    ///
    /// # Errors
    /// [`PasteError`], wenn nicht eingefügt wurde.
    fn paste(&self, text: &str) -> Result<Pasted, PasteError>;

    /// Legt `text` in die Zwischenablage.
    ///
    /// # Errors
    /// [`PasteError`], wenn die Zwischenablage versagt.
    fn copy(&self, text: &str) -> Result<(), PasteError>;

    /// Der Text in der Zwischenablage; `None`, wenn keiner darin liegt.
    ///
    /// # Errors
    /// [`PasteError`], wenn die Zwischenablage nicht lesbar ist.
    fn clipboard_text(&self) -> Result<Option<String>, PasteError>;

    /// Die im Finder kopierten Dateien; leer, wenn keine darin liegen.
    ///
    /// # Errors
    /// [`PasteError`], wenn die Zwischenablage nicht lesbar ist.
    fn clipboard_files(&self) -> Result<Vec<PathBuf>, PasteError>;

    /// Die App vorn und der Text um ihre Schreibmarke.
    ///
    /// # Errors
    /// [`HelperError`], wenn nicht einmal die App bekannt ist.
    fn cursor(&self) -> Result<Cursor, HelperError>;

    /// Zeigt eine Änderung in Menüleisten-Symbol und Panel.
    fn show(&self, update: UiUpdate);
}

/// Der echte Mac; Änderungen der Anzeige gehen an `update_ui`.
pub(crate) struct Mac<U: Fn(UiUpdate)> {
    update_ui: U,
}

impl<U: Fn(UiUpdate)> Mac<U> {
    /// Der Mac mit dieser Anzeige.
    pub(crate) const fn new(update_ui: U) -> Self {
        Self { update_ui }
    }
}

impl<U: Fn(UiUpdate)> Platform for Mac<U> {
    fn record(&self) -> Result<Box<dyn Microphone>, RecordError> {
        Ok(Box::new(record::start()?))
    }

    fn paste(&self, text: &str) -> Result<Pasted, PasteError> {
        paste::paste_text(text)
    }

    fn copy(&self, text: &str) -> Result<(), PasteError> {
        paste::copy_text(text)
    }

    fn clipboard_text(&self) -> Result<Option<String>, PasteError> {
        paste::clipboard_text()
    }

    fn clipboard_files(&self) -> Result<Vec<PathBuf>, PasteError> {
        paste::clipboard_files()
    }

    fn cursor(&self) -> Result<Cursor, HelperError> {
        context::read()
    }

    fn show(&self, update: UiUpdate) {
        (self.update_ui)(update);
    }
}

impl Microphone for Recording {
    fn next_part(&mut self, timing: PartTiming) -> Result<Option<Pcm16k>, RecordError> {
        self.take_part(timing)
    }

    fn level(&self) -> f32 {
        self.current_level()
    }

    fn unsent(&self) -> Duration {
        self.buffered()
    }

    fn finish(self: Box<Self>) -> Result<Captured, Interrupted> {
        self.into_captured()
    }
}
