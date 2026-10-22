//! Text an der Schreibmarke einfügen, ohne die Zwischenablage zu verlieren.
//!
//! Ablauf: Zwischenablage sichern → Text hinein → ⌘V senden → kurz warten →
//! alten Inhalt zurück – aber nur, wenn noch das Diktat darin liegt: Hat in
//! der Zwischenzeit jemand etwas anderes kopiert, bleibt das. ⌘V wird als Tastencode der Taste V gesendet; das ist
//! unabhängig vom Tastaturlayout (QWERTZ) und funktioniert anders als
//! zeichenweises Tippen auch mit Umlauten.
//!
//! Diktat und wiederhergestellter Inhalt werden als „nicht in den Verlauf“
//! markiert (`org.nspasteboard.ConcealedType`, <http://nspasteboard.org>):
//! Zwischenablage-Verläufe wie Raycast, Maccy, Paste oder der von macOS
//! speichern das Diktat dann nicht und legen den alten Inhalt nicht doppelt
//! ab. Bewusst Kopiertes (Transkripte, Verlauf im Panel) bleibt normal.
//!
//! Gesichert werden Text und Bilder. Formatierter Text (RTF/HTML) kommt als
//! reiner Text zurück, andere Formate (kopierte Dateien, Tabellen) gehen
//! verloren; ist die Zwischenablage danach leer, steht das so im README.
//!
//! Benötigt das Recht „Bedienungshilfen“.

use crate::note::Explain;
use std::path::PathBuf;
use std::time::Duration;

use crate::clock::{Clock, SystemClock};

use arboard::{Clipboard, ImageData, SetExtApple};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

/// Tastencode der Taste V (`kVK_ANSI_V`), layoutunabhängig.
const KEYCODE_V: CGKeyCode = 9;

/// Wartezeit, bis die Zwischenablage den neuen Inhalt sicher anbietet.
const BEFORE_PASTE: Duration = Duration::from_millis(40);

/// Wartezeit, bis die Ziel-App eingefügt hat, bevor der alte Inhalt
/// zurückkommt. Zu kurz: die App fügt den alten Inhalt ein.
const BEFORE_RESTORE: Duration = Duration::from_millis(400);

/// Fehler beim Einfügen.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PasteError {
    /// Die Zwischenablage ist nicht erreichbar oder lehnt ab.
    #[error("Zwischenablage: {0}")]
    Clipboard(#[from] arboard::Error),
    /// Das Tastenereignis für ⌘V ließ sich nicht erzeugen.
    #[error("⌘V konnte nicht erzeugt werden")]
    KeyEvent,
    /// ⌘V ließ sich nicht erzeugen, und der alte Inhalt der
    /// Zwischenablage ließ sich nicht zurückholen: Sie enthält jetzt das
    /// Diktat.
    #[error(
        "⌘V konnte nicht erzeugt werden, und die Zwischenablage ließ sich nicht wiederherstellen ({0}) – sie enthält jetzt das Diktat"
    )]
    KeyEventAndRestore(arboard::Error),
}

impl Explain for PasteError {
    fn english(&self) -> String {
        match self {
            Self::Clipboard(source) => format!("clipboard: {source}"),
            Self::KeyEvent => "could not create ⌘V".to_owned(),
            Self::KeyEventAndRestore(source) => format!(
                "could not create ⌘V, and the clipboard could not be restored ({source}) – it now holds the dictation"
            ),
        }
    }
}

/// Wie ein gelungenes Einfügen mit der Zwischenablage umging.
#[derive(Debug)]
pub(crate) enum Pasted {
    /// Der alte Inhalt ist zurück.
    Restored,
    /// Inzwischen hat jemand anderes kopiert; das bleibt.
    LeftNewer,
    /// Eingefügt, aber der alte Inhalt ließ sich nicht zurückholen.
    NotRestored(PasteError),
}

/// Was vor dem Einfügen in der Zwischenablage lag.
#[derive(Debug)]
enum Saved {
    /// Text.
    Text(String),
    /// Ein Bild.
    Image(ImageData<'static>),
    /// Nichts (oder ein Format, das hier nicht gelesen wird).
    Empty,
}

/// Fügt `text` an der Schreibmarke der vordersten App ein und stellt danach
/// die Zwischenablage wieder her, sofern sie inzwischen niemand geändert hat.
///
/// # Errors
/// [`PasteError`], wenn nicht eingefügt wurde; die Zwischenablage ist dann
/// wiederhergestellt, außer bei [`PasteError::KeyEventAndRestore`].
pub(crate) fn paste_text(text: &str) -> Result<Pasted, PasteError> {
    let mut board = MacBoard {
        clipboard: Clipboard::new()?,
    };
    paste_with(&mut board, &MacKeys, &SystemClock, text)
}

/// Die Zwischenablage, wie das Einfügen sie braucht.
trait Board {
    /// Was gesichert wird.
    type Saved;

    /// Sichert den Inhalt.
    fn save(&mut self) -> Result<Self::Saved, PasteError>;

    /// Legt `text` hinein (nicht für Verläufe).
    fn set_text(&mut self, text: &str) -> Result<(), PasteError>;

    /// Liegt noch `text` darin? Unlesbar heißt nein: lieber das Diktat
    /// stehen lassen als etwas Fremdes überschreiben.
    fn holds(&mut self, text: &str) -> bool;

    /// Stellt den gesicherten Inhalt wieder her.
    fn restore(&mut self, saved: Self::Saved) -> Result<(), PasteError>;
}

/// Die Tastatur: ⌘V senden.
trait Keys {
    /// Sendet ⌘V an die vorderste App.
    fn command_v(&self) -> Result<(), PasteError>;
}

/// Ablauf des Einfügens: sichern → Text hinein → ⌘V → warten → alten Inhalt
/// zurück, wenn noch das Diktat darin liegt. Scheitert ⌘V, kommt der alte
/// Inhalt sofort zurück; scheitert danach nur das Zurückholen, ist trotzdem
/// eingefügt.
fn paste_with<B: Board>(
    board: &mut B,
    keys: &impl Keys,
    clock: &impl Clock,
    text: &str,
) -> Result<Pasted, PasteError> {
    let saved = board.save()?;
    board.set_text(text)?;
    clock.sleep(BEFORE_PASTE);
    if let Err(error) = keys.command_v() {
        return Err(match board.restore(saved) {
            Ok(()) => error,
            Err(PasteError::Clipboard(source)) => PasteError::KeyEventAndRestore(source),
            Err(other @ (PasteError::KeyEvent | PasteError::KeyEventAndRestore(_))) => other,
        });
    }
    clock.sleep(BEFORE_RESTORE);
    if !board.holds(text) {
        return Ok(Pasted::LeftNewer);
    }
    Ok(match board.restore(saved) {
        Ok(()) => Pasted::Restored,
        Err(error) => Pasted::NotRestored(error),
    })
}

/// Die Zwischenablage von macOS über arboard.
struct MacBoard {
    clipboard: Clipboard,
}

impl Board for MacBoard {
    type Saved = Saved;

    fn save(&mut self) -> Result<Saved, PasteError> {
        save(&mut self.clipboard)
    }

    fn set_text(&mut self, text: &str) -> Result<(), PasteError> {
        Ok(self.clipboard.set().exclude_from_history().text(text)?)
    }

    fn holds(&mut self, text: &str) -> bool {
        self.clipboard
            .get_text()
            .is_ok_and(|current| current == text)
    }

    fn restore(&mut self, saved: Saved) -> Result<(), PasteError> {
        restore(&mut self.clipboard, saved)
    }
}

/// Die Tastatur von macOS über `CGEvent`.
struct MacKeys;

impl Keys for MacKeys {
    fn command_v(&self) -> Result<(), PasteError> {
        press_command_v()
    }
}

/// Legt `text` in die Zwischenablage, ohne einzufügen (YouTube-Ergebnis).
///
/// # Errors
/// [`PasteError::Clipboard`].
pub(crate) fn copy_text(text: &str) -> Result<(), PasteError> {
    Clipboard::new()?.set_text(text)?;
    Ok(())
}

/// Liest den aktuellen Text der Zwischenablage (etwa einen YouTube-Link);
/// `None`, wenn kein Text darin liegt.
///
/// # Errors
/// [`PasteError::Clipboard`], wenn die Zwischenablage nicht lesbar ist.
pub(crate) fn clipboard_text() -> Result<Option<String>, PasteError> {
    match Clipboard::new()?.get_text() {
        Ok(text) => Ok(Some(text)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Liest die im Finder kopierten Dateien (⌘C auf eine Datei); leer, wenn
/// keine darin liegen.
///
/// # Errors
/// [`PasteError::Clipboard`], wenn die Zwischenablage nicht lesbar ist.
pub(crate) fn clipboard_files() -> Result<Vec<PathBuf>, PasteError> {
    match Clipboard::new()?.get().file_list() {
        Ok(files) => Ok(files),
        Err(arboard::Error::ContentNotAvailable) => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

/// Sichert Text oder Bild aus der Zwischenablage.
fn save(clipboard: &mut Clipboard) -> Result<Saved, PasteError> {
    match clipboard.get_text() {
        Ok(text) => return Ok(Saved::Text(text)),
        Err(arboard::Error::ContentNotAvailable) => {}
        Err(error) => return Err(error.into()),
    }
    match clipboard.get_image() {
        Ok(image) => Ok(Saved::Image(image)),
        Err(arboard::Error::ContentNotAvailable) => Ok(Saved::Empty),
        Err(error) => Err(error.into()),
    }
}

/// Stellt den gesicherten Inhalt wieder her.
fn restore(clipboard: &mut Clipboard, saved: Saved) -> Result<(), PasteError> {
    match saved {
        Saved::Text(text) => clipboard.set().exclude_from_history().text(text)?,
        Saved::Image(image) => clipboard.set().exclude_from_history().image(image)?,
        Saved::Empty => clipboard.clear()?,
    }
    Ok(())
}

/// Sendet ⌘V an die vorderste App.
fn press_command_v() -> Result<(), PasteError> {
    for key_down in [true, false] {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| PasteError::KeyEvent)?;
        let event = CGEvent::new_keyboard_event(source, KEYCODE_V, key_down)
            .map_err(|()| PasteError::KeyEvent)?;
        event.set_flags(CGEventFlags::CGEventFlagCommand);
        event.post(CGEventTapLocation::HID);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use std::cell::{Cell, RefCell};

    /// Eine Zwischenablage, die mitschreibt und beim Zurückholen scheitern
    /// kann.
    #[derive(Default)]
    struct FakeBoard {
        content: RefCell<String>,
        restore_fails: Cell<bool>,
    }

    impl Board for &FakeBoard {
        type Saved = String;

        fn save(&mut self) -> Result<String, PasteError> {
            Ok(self.content.borrow().clone())
        }

        fn set_text(&mut self, text: &str) -> Result<(), PasteError> {
            text.clone_into(&mut self.content.borrow_mut());
            Ok(())
        }

        fn holds(&mut self, text: &str) -> bool {
            *self.content.borrow() == text
        }

        fn restore(&mut self, saved: String) -> Result<(), PasteError> {
            if self.restore_fails.get() {
                return Err(PasteError::Clipboard(arboard::Error::ClipboardOccupied));
            }
            *self.content.borrow_mut() = saved;
            Ok(())
        }
    }

    /// Eine Tastatur, deren ⌘V gelingt oder scheitert.
    struct FakeKeys {
        fails: bool,
    }

    impl Keys for FakeKeys {
        fn command_v(&self) -> Result<(), PasteError> {
            if self.fails {
                Err(PasteError::KeyEvent)
            } else {
                Ok(())
            }
        }
    }

    fn board(content: &str) -> FakeBoard {
        let board = FakeBoard::default();
        content.clone_into(&mut board.content.borrow_mut());
        board
    }

    #[test]
    fn a_failed_command_v_gives_the_clipboard_back() {
        let saved = board("Eigenes");
        let error = paste_with(
            &mut &saved,
            &FakeKeys { fails: true },
            &FakeClock::new(),
            "Diktat",
        )
        .unwrap_err();
        assert!(matches!(error, PasteError::KeyEvent), "{error:?}");
        assert_eq!(
            *saved.content.borrow(),
            "Eigenes",
            "die Zwischenablage ist zurück"
        );
    }

    #[test]
    fn a_failed_command_v_without_restore_says_the_clipboard_is_replaced() {
        let saved = board("Eigenes");
        saved.restore_fails.set(true);
        let error = paste_with(
            &mut &saved,
            &FakeKeys { fails: true },
            &FakeClock::new(),
            "Diktat",
        )
        .unwrap_err();
        assert!(
            matches!(error, PasteError::KeyEventAndRestore(_)),
            "{error:?}"
        );
    }

    #[test]
    fn a_failed_restore_after_command_v_is_still_inserted() {
        let saved = board("Eigenes");
        saved.restore_fails.set(true);
        let pasted = paste_with(
            &mut &saved,
            &FakeKeys { fails: false },
            &FakeClock::new(),
            "Diktat",
        )
        .unwrap();
        assert!(matches!(pasted, Pasted::NotRestored(_)), "{pasted:?}");
    }

    #[test]
    fn a_successful_paste_restores_the_clipboard() {
        let saved = board("Eigenes");
        let pasted = paste_with(
            &mut &saved,
            &FakeKeys { fails: false },
            &FakeClock::new(),
            "Diktat",
        )
        .unwrap();
        assert!(matches!(pasted, Pasted::Restored), "{pasted:?}");
        assert_eq!(*saved.content.borrow(), "Eigenes");
    }
}
