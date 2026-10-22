//! Globale Tasten: fn antippen = Deutsch, fn + ⇧ = zweite Sprache, Esc = Abbruch.
//!
//! Statt fn geht auch eine rechte Sondertaste ([`TriggerKey`], in den
//! Einstellungen wählbar); „fn“ steht unten für die gewählte Taste.
//!
//! Ein `CGEventTap` liest alle Tastenereignisse und Mausklicks mit.
//! „Antippen“ heißt: fn gedrückt und wieder losgelassen, ohne dass
//! dazwischen eine andere Taste oder ein Klick kam, und kürzer als
//! `MAX_TAP` (1,5 s). fn + Pfeil, fn + F-Tasten, ⌘-Klick und die
//! Globus-Kürzel lösen deshalb nie etwas aus. ⇧ ist eine Sondertaste und
//! zählt nicht als „andere Taste“.
//!
//! Esc wird nur abgefangen (und nicht an die App weitergegeben), solange
//! `escape_capture` gesetzt ist, also während einer Aufnahme und während
//! des Überarbeitens.
//!
//! Benötigt die Rechte „Bedienungshilfen“ und „Eingabeüberwachung“.

use crate::clock::{Clock as _, SystemClock};
use crate::note::Explain;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use core_foundation::base::TCFType;
use core_foundation::runloop::CFRunLoop;
use core_foundation::string::CFString;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField,
};

use serde::{Deserialize, Serialize};

/// Längster Druck, der noch als Antippen zählt.
pub(crate) const MAX_TAP: Duration = Duration::from_millis(1500);

/// Tastencode von Esc (`kVK_Escape`).
const KEYCODE_ESCAPE: i64 = 53;

/// Geräteabhängige Bits der rechten Sondertasten in den Ereignis-Flags
/// (`NX_DEVICERALTKEYMASK`, `NX_DEVICERCMDKEYMASK`, `NX_DEVICERCTLKEYMASK`
/// aus `IOKit/hidsystem/IOLLEvent.h`).
const DEVICE_RIGHT_OPTION: u64 = 0x40;
/// Siehe [`DEVICE_RIGHT_OPTION`].
const DEVICE_RIGHT_COMMAND: u64 = 0x10;
/// Siehe [`DEVICE_RIGHT_OPTION`].
const DEVICE_RIGHT_CONTROL: u64 = 0x2000;

/// Die Taste, deren Antippen eine Aufnahme startet und beendet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum TriggerKey {
    /// fn bzw. Globus (Standard).
    #[default]
    #[serde(rename = "fn")]
    Fn,
    /// Rechte Wahltaste ⌥.
    #[serde(rename = "rechte_wahltaste")]
    RightOption,
    /// Rechte Befehlstaste ⌘.
    #[serde(rename = "rechte_befehlstaste")]
    RightCommand,
    /// Rechte Control-Taste ⌃.
    #[serde(rename = "rechte_controltaste")]
    RightControl,
}

impl TriggerKey {
    /// Alle Tasten, in der Reihenfolge der Auswahl.
    pub(crate) const ALL: [Self; 4] = [
        Self::Fn,
        Self::RightOption,
        Self::RightCommand,
        Self::RightControl,
    ];

    /// Wert in `settings.toml`.
    #[must_use]
    pub(crate) const fn file_value(self) -> &'static str {
        match self {
            Self::Fn => "fn",
            Self::RightOption => "rechte_wahltaste",
            Self::RightCommand => "rechte_befehlstaste",
            Self::RightControl => "rechte_controltaste",
        }
    }

    /// Tastencode (`kVK_Function`, `kVK_RightOption` …).
    const fn keycode(self) -> i64 {
        match self {
            Self::Fn => 63,
            Self::RightOption => 61,
            Self::RightCommand => 54,
            Self::RightControl => 62,
        }
    }

    /// Merker der Sondertaste zum Überarbeiten: ⌥, außer die Diktiertaste
    /// ist selbst die rechte ⌥ – dann ⌃.
    const fn polish_flag(self) -> CGEventFlags {
        match self {
            Self::RightOption => CGEventFlags::CGEventFlagControl,
            Self::Fn | Self::RightCommand | Self::RightControl => {
                CGEventFlags::CGEventFlagAlternate
            }
        }
    }

    /// Ist die Taste laut den Ereignis-Flags gedrückt? Bei den rechten
    /// Sondertasten zählt das geräteabhängige Bit der rechten Taste: Das
    /// allgemeine (⌘, ⌥, ⌃) bliebe gesetzt, solange die linke gehalten wird,
    /// und das Loslassen der rechten ginge verloren.
    const fn is_down(self, flags: CGEventFlags) -> bool {
        let mask = match self {
            Self::Fn => CGEventFlags::CGEventFlagSecondaryFn.bits(),
            Self::RightOption => DEVICE_RIGHT_OPTION,
            Self::RightCommand => DEVICE_RIGHT_COMMAND,
            Self::RightControl => DEVICE_RIGHT_CONTROL,
        };
        flags.bits() & mask != 0
    }
}

/// Welche der beiden Sprachen ein Antippen wählt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// fn allein: Deutsch.
    Primary,
    /// fn + ⇧: die zweite Sprache aus den Einstellungen.
    Secondary,
}

/// Was der Nutzer mit den Tasten ausgelöst hat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HotkeyEvent {
    /// fn wurde angetippt; ⇧ entscheidet über die Sprache, ⌥ übers
    /// Überarbeiten.
    Tap(Tap),
    /// Esc während einer Aufnahme.
    Escape,
}

/// Fehler des Tasten-Mitlesers.
#[derive(Debug, thiserror::Error)]
pub(crate) enum HotkeyError {
    /// macOS verweigert den Event-Tap – fast immer fehlt ein Recht.
    #[error(
        "Tasten können nicht mitgelesen werden: Rechte „Bedienungshilfen“ und \
         „Eingabeüberwachung“ in den Systemeinstellungen erteilen, dann Diktator neu starten"
    )]
    TapRefused,
    /// Der Empfänger der Ereignisse existiert nicht mehr.
    #[error("Empfänger der Tastenereignisse ist beendet")]
    ReceiverGone,
}

impl Explain for HotkeyError {
    fn english(&self) -> String {
        match self {
            Self::TapRefused => {
                "Keys cannot be read: grant “Accessibility” and “Input Monitoring” \
                                 in System Settings, then restart Diktator"
                    .to_owned()
            }
            Self::ReceiverGone => "the receiver of key events has ended".to_owned(),
        }
    }
}

/// Ob ⇧ während des Antippens gedrückt war.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shift {
    /// ⇧ nicht gedrückt.
    Up,
    /// ⇧ gedrückt.
    Down,
}

/// Ob die Taste zum Überarbeiten (⌥, bei rechter ⌥ als Diktiertaste ⌃)
/// während des Antippens gedrückt war.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Polish {
    /// Nicht gedrückt: Text roh einfügen.
    Up,
    /// Gedrückt: Text vor dem Einfügen überarbeiten lassen.
    Down,
}

/// Die Sondertasten, die beim Antippen zählen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Modifiers {
    /// ⇧: zweite Sprache.
    pub shift: Shift,
    /// ⌥: überarbeiten.
    pub polish: Polish,
}

impl Modifiers {
    /// Keine Sondertaste.
    #[cfg(test)]
    pub(crate) const NONE: Self = Self {
        shift: Shift::Up,
        polish: Polish::Up,
    };

    /// Was in einem der beiden gedrückt war (gedrückt bleibt gedrückt).
    #[must_use]
    pub(crate) const fn or(self, other: Self) -> Self {
        Self {
            shift: match (self.shift, other.shift) {
                (Shift::Up, Shift::Up) => Shift::Up,
                (Shift::Down, _) | (_, Shift::Down) => Shift::Down,
            },
            polish: match (self.polish, other.polish) {
                (Polish::Up, Polish::Up) => Polish::Up,
                (Polish::Down, _) | (_, Polish::Down) => Polish::Down,
            },
        }
    }
}

/// Was ein Antippen auslöst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tap {
    /// Erste oder zweite Sprache.
    pub slot: Slot,
    /// Roh oder überarbeitet.
    pub mode: Mode,
}

/// Roh einfügen oder vorher überarbeiten lassen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    /// So, wie das Spracherkennungsmodell ihn liefert.
    Raw,
    /// Vom Sprachmodell (Apple Intelligence oder lokal) überarbeitet.
    Polish,
}

/// Zustand der fn-Taste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FnState {
    /// fn ist nicht gedrückt.
    Released,
    /// fn ist gedrückt, bisher ohne andere Taste.
    Held {
        /// Seit wann.
        since: Instant,
        /// Welche Sondertasten irgendwann während des Drucks gedrückt waren.
        modifiers: Modifiers,
    },
    /// fn ist gedrückt, aber mit einer anderen Taste kombiniert worden.
    Combined,
}

/// Was ein Ereignis für das Erkennen des Antippens bedeutet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Input {
    /// fn wurde gedrückt; mit den Sondertasten, die in dem Moment gedrückt
    /// sind.
    TriggerDown(Modifiers),
    /// fn wurde losgelassen; mit den Sondertasten in dem Moment.
    TriggerUp(Modifiers),
    /// Eine andere Sondertaste wurde gedrückt oder losgelassen. Während fn
    /// gehalten wird, zählt sie für das Antippen – auch wenn sie vor fn
    /// wieder losgelassen wird.
    ModifiersChanged(Modifiers),
    /// Eine andere (nicht-Sonder-)Taste oder ein Mausklick: fn + Pfeil oder
    /// ⌘-Klick ist kein Antippen.
    Other,
}

/// Erkennt das Antippen von fn. Reine Logik ohne Mac-API, daher testbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TapDetector {
    state: FnState,
}

impl TapDetector {
    /// Startzustand: fn nicht gedrückt.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            state: FnState::Released,
        }
    }

    /// Ein Schritt: der neue Zustand und, beim Loslassen nach einem
    /// Antippen, was es auslöst. Zweite Sprache, wenn ⇧ irgendwann zwischen
    /// Drücken und Loslassen gedrückt war, sonst die erste (Deutsch);
    /// überarbeiten genauso mit ⌥. Ein erneutes Drücken beginnt von vorn.
    #[must_use]
    pub(crate) fn step(self, input: Input, now: Instant) -> (Self, Option<Tap>) {
        let held = |modifiers| FnState::Held {
            since: now,
            modifiers,
        };
        let (state, tap) = match self.state {
            FnState::Released => match input {
                Input::TriggerDown(modifiers) => (held(modifiers), None),
                Input::TriggerUp(_) | Input::ModifiersChanged(_) | Input::Other => {
                    (FnState::Released, None)
                }
            },
            FnState::Held { since, modifiers } => match input {
                Input::TriggerDown(pressed) => (held(pressed), None),
                Input::TriggerUp(pressed) => (
                    FnState::Released,
                    tap_after(now.saturating_duration_since(since), modifiers.or(pressed)),
                ),
                Input::ModifiersChanged(pressed) => (
                    FnState::Held {
                        since,
                        modifiers: modifiers.or(pressed),
                    },
                    None,
                ),
                Input::Other => (FnState::Combined, None),
            },
            FnState::Combined => match input {
                Input::TriggerDown(modifiers) => (held(modifiers), None),
                Input::TriggerUp(_) => (FnState::Released, None),
                Input::ModifiersChanged(_) | Input::Other => (FnState::Combined, None),
            },
        };
        (Self { state }, tap)
    }
}

/// Was ein Druck von `held` Dauer mit diesen Sondertasten auslöst; nichts,
/// wenn er länger als [`MAX_TAP`] war.
fn tap_after(held: Duration, modifiers: Modifiers) -> Option<Tap> {
    if held > MAX_TAP {
        return None;
    }
    Some(Tap {
        slot: match modifiers.shift {
            Shift::Up => Slot::Primary,
            Shift::Down => Slot::Secondary,
        },
        mode: match modifiers.polish {
            Polish::Up => Mode::Raw,
            Polish::Down => Mode::Polish,
        },
    })
}

/// Ordnet ein Sondertasten-Ereignis (Tastencode und Flags danach) für die
/// Diktiertaste `key` ein.
fn classify_flags(keycode: i64, flags: CGEventFlags, key: TriggerKey) -> Input {
    let modifiers = Modifiers {
        shift: if flags.contains(CGEventFlags::CGEventFlagShift) {
            Shift::Down
        } else {
            Shift::Up
        },
        polish: if flags.contains(key.polish_flag()) {
            Polish::Down
        } else {
            Polish::Up
        },
    };
    if keycode != key.keycode() {
        return Input::ModifiersChanged(modifiers);
    }
    if key.is_down(flags) {
        Input::TriggerDown(modifiers)
    } else {
        Input::TriggerUp(modifiers)
    }
}

/// So oft sieht der Tasten-Mitleser nach, ob er enden soll.
const STOP_CHECK: Duration = Duration::from_millis(250);

/// Liest die Tasten mit und schickt [`HotkeyEvent`]s (umgewandelt in `T`)
/// an `sender`.
///
/// Läuft auf dem aufrufenden Thread, bis `stop` gesetzt wird (spätestens
/// nach [`STOP_CHECK`] gesehen) oder der Empfänger wegfällt. Schaltet macOS
/// den Tap ab (zu langsame Antwort, sichere Eingabe), wird er neu angelegt.
///
/// # Errors
/// [`HotkeyError::TapRefused`] ohne die nötigen Rechte,
/// [`HotkeyError::ReceiverGone`], wenn niemand mehr zuhört.
pub(crate) fn listen<T: From<HotkeyEvent>>(
    sender: &Sender<T>,
    escape_capture: &AtomicBool,
    key: TriggerKey,
    stop: &AtomicBool,
) -> Result<(), HotkeyError> {
    // Der Name des Standard-Modus der Laufschleife: so ohne `unsafe` (die
    // Konstante von Core Foundation ist eine C-Variable).
    let default_mode = CFString::from_static_string("kCFRunLoopDefaultMode");
    loop {
        let detector = Cell::new(TapDetector::new());
        let receiver_gone = Cell::new(false);
        let tap_disabled = Cell::new(false);
        let callback = |_proxy, event_type: CGEventType, event: &CGEvent| {
            let reaction = react(event_type, event, &detector, escape_capture, key);
            if let Some(hotkey) = reaction.event
                && sender.send(T::from(hotkey)).is_err()
            {
                receiver_gone.set(true);
                CFRunLoop::get_current().stop();
            }
            if reaction.tap == TapStatus::Disabled {
                tap_disabled.set(true);
                CFRunLoop::get_current().stop();
            }
            reaction.result
        };
        // In kurzen Stücken laufen, damit `stop` gesehen wird, auch wenn
        // keine Taste kommt.
        let run = || {
            while !stop.load(Ordering::Relaxed) && !receiver_gone.get() && !tap_disabled.get() {
                CFRunLoop::run_in_mode(default_mode.as_concrete_TypeRef(), STOP_CHECK, false);
            }
        };
        CGEventTap::with_enabled(
            CGEventTapLocation::Session,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            vec![
                CGEventType::KeyDown,
                CGEventType::FlagsChanged,
                CGEventType::LeftMouseDown,
                CGEventType::RightMouseDown,
                CGEventType::OtherMouseDown,
            ],
            callback,
            run,
        )
        .map_err(|()| HotkeyError::TapRefused)?;
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if receiver_gone.get() {
            return Err(HotkeyError::ReceiverGone);
        }
        // Sonst wurde der Tap von macOS abgeschaltet: neu anlegen.
    }
}

/// Antwort auf ein einzelnes Tastenereignis.
struct Reaction {
    /// Ausgelöstes Ereignis, falls eins.
    event: Option<HotkeyEvent>,
    /// Ob das Ereignis weitergegeben oder geschluckt wird.
    result: CallbackResult,
    /// Ob macOS den Tap abgeschaltet hat.
    tap: TapStatus,
}

/// Zustand des Event-Taps laut macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TapStatus {
    /// Läuft.
    Active,
    /// Von macOS abgeschaltet; muss neu angelegt werden.
    Disabled,
}

impl Reaction {
    /// Nichts passiert, Ereignis weiterreichen.
    const fn pass() -> Self {
        Self {
            event: None,
            result: CallbackResult::Keep,
            tap: TapStatus::Active,
        }
    }
}

/// Wertet ein Tastenereignis aus.
fn react(
    event_type: CGEventType,
    event: &CGEvent,
    detector: &Cell<TapDetector>,
    escape_capture: &AtomicBool,
    key: TriggerKey,
) -> Reaction {
    let keycode = || event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
    match event_type {
        CGEventType::FlagsChanged => {
            let tapped = feed(detector, classify_flags(keycode(), event.get_flags(), key));
            Reaction {
                event: tapped.map(HotkeyEvent::Tap),
                ..Reaction::pass()
            }
        }
        CGEventType::KeyDown => on_key_down(keycode(), detector, escape_capture),
        // Ein Klick tippt nichts an: ⌘-Klick mit der rechten ⌘ als
        // Diktiertaste bleibt ein Klick.
        CGEventType::LeftMouseDown | CGEventType::RightMouseDown | CGEventType::OtherMouseDown => {
            other_input(detector);
            Reaction::pass()
        }
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => Reaction {
            tap: TapStatus::Disabled,
            ..Reaction::pass()
        },
        CGEventType::Null
        | CGEventType::LeftMouseUp
        | CGEventType::RightMouseUp
        | CGEventType::MouseMoved
        | CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged
        | CGEventType::KeyUp
        | CGEventType::ScrollWheel
        | CGEventType::TabletPointer
        | CGEventType::TabletProximity
        | CGEventType::OtherMouseUp
        | CGEventType::OtherMouseDragged => Reaction::pass(),
    }
}

/// Gibt `input` jetzt an den Erkenner.
fn feed(detector: &Cell<TapDetector>, input: Input) -> Option<Tap> {
    let (next, tapped) = detector.get().step(input, SystemClock.now());
    detector.set(next);
    tapped
}

/// Eine andere Taste oder ein Klick: Nur Loslassen löst etwas aus.
fn other_input(detector: &Cell<TapDetector>) {
    let tapped = feed(detector, Input::Other);
    debug_assert!(tapped.is_none(), "invariant: nur Loslassen löst aus");
}

/// Normale Taste gedrückt: beendet ein mögliches Antippen; Esc während der
/// Aufnahme wird geschluckt und gemeldet.
fn on_key_down(
    keycode: i64,
    detector: &Cell<TapDetector>,
    escape_capture: &AtomicBool,
) -> Reaction {
    other_input(detector);
    if keycode == KEYCODE_ESCAPE && escape_capture.load(Ordering::Relaxed) {
        return Reaction {
            event: Some(HotkeyEvent::Escape),
            result: CallbackResult::Drop,
            tap: TapStatus::Active,
        };
    }
    Reaction::pass()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIFT: Modifiers = Modifiers {
        shift: Shift::Down,
        polish: Polish::Up,
    };

    const OPTION: Modifiers = Modifiers {
        shift: Shift::Up,
        polish: Polish::Down,
    };

    fn raw(slot: Slot) -> Tap {
        Tap {
            slot,
            mode: Mode::Raw,
        }
    }

    /// Spielt Eingaben (Millisekunden ab Start, Eingabe) ab; liefert, was
    /// die letzte auslöste.
    fn play(inputs: &[(u64, Input)]) -> Option<Tap> {
        let start = SystemClock.now();
        let mut detector = TapDetector::new();
        let mut last = None;
        for (millis, input) in inputs {
            let (next, tapped) = detector.step(*input, start + Duration::from_millis(*millis));
            detector = next;
            last = tapped;
        }
        last
    }

    #[test]
    fn option_asks_for_polish_and_combines_with_shift() {
        let result = play(&[
            (0, Input::TriggerDown(OPTION)),
            (50, Input::ModifiersChanged(SHIFT)),
            (150, Input::TriggerUp(Modifiers::NONE)),
        ]);
        assert_eq!(
            result,
            Some(Tap {
                slot: Slot::Secondary,
                mode: Mode::Polish
            })
        );
    }

    #[test]
    fn short_tap_is_german() {
        let result = play(&[
            (0, Input::TriggerDown(Modifiers::NONE)),
            (120, Input::TriggerUp(Modifiers::NONE)),
        ]);
        assert_eq!(result, Some(raw(Slot::Primary)));
    }

    #[test]
    fn tap_with_shift_is_english() {
        let result = play(&[
            (0, Input::TriggerDown(Modifiers::NONE)),
            (120, Input::TriggerUp(SHIFT)),
        ]);
        assert_eq!(result, Some(raw(Slot::Secondary)));
    }

    #[test]
    fn shift_released_before_fn_is_still_english() {
        let result = play(&[
            (0, Input::TriggerDown(Modifiers::NONE)),
            (50, Input::ModifiersChanged(SHIFT)),
            (100, Input::ModifiersChanged(Modifiers::NONE)),
            (150, Input::TriggerUp(Modifiers::NONE)),
        ]);
        assert_eq!(result, Some(raw(Slot::Secondary)));
    }

    #[test]
    fn shift_held_at_fn_press_is_english() {
        let result = play(&[
            (0, Input::TriggerDown(SHIFT)),
            (150, Input::TriggerUp(Modifiers::NONE)),
        ]);
        assert_eq!(result, Some(raw(Slot::Secondary)));
    }

    #[test]
    fn shift_without_fn_changes_nothing() {
        let result = play(&[
            (0, Input::ModifiersChanged(SHIFT)),
            (10, Input::TriggerDown(Modifiers::NONE)),
            (150, Input::TriggerUp(Modifiers::NONE)),
        ]);
        assert_eq!(result, Some(raw(Slot::Primary)));
    }

    #[test]
    fn fn_with_arrow_or_click_is_no_tap() {
        let result = play(&[
            (0, Input::TriggerDown(Modifiers::NONE)),
            (50, Input::Other),
            (120, Input::TriggerUp(Modifiers::NONE)),
        ]);
        assert_eq!(result, None);
    }

    #[test]
    fn long_press_is_no_tap() {
        let result = play(&[
            (0, Input::TriggerDown(Modifiers::NONE)),
            (2000, Input::TriggerUp(Modifiers::NONE)),
        ]);
        assert_eq!(result, None);
        let limit = u64::try_from(MAX_TAP.as_millis()).unwrap();
        let at_limit = play(&[
            (0, Input::TriggerDown(Modifiers::NONE)),
            (limit, Input::TriggerUp(Modifiers::NONE)),
        ]);
        assert_eq!(
            at_limit,
            Some(raw(Slot::Primary)),
            "genau MAX_TAP zählt noch"
        );
    }

    #[test]
    fn release_without_press_is_no_tap() {
        assert_eq!(play(&[(0, Input::TriggerUp(Modifiers::NONE))]), None);
    }

    #[test]
    fn a_new_press_starts_over() {
        let result = play(&[
            (0, Input::TriggerDown(Modifiers::NONE)),
            (10, Input::Other),
            (20, Input::TriggerUp(Modifiers::NONE)),
            (30, Input::TriggerDown(Modifiers::NONE)),
            (60, Input::TriggerUp(Modifiers::NONE)),
        ]);
        assert_eq!(result, Some(raw(Slot::Primary)));
        let pressed_again = play(&[
            (0, Input::TriggerDown(SHIFT)),
            (1000, Input::TriggerDown(Modifiers::NONE)),
            (2000, Input::TriggerUp(Modifiers::NONE)),
        ]);
        assert_eq!(
            pressed_again,
            Some(raw(Slot::Primary)),
            "Zeit und Sondertasten zählen ab dem letzten Drücken"
        );
    }

    /// Flags aus den allgemeinen Bits und Gerätebits.
    fn flags(bits: &[u64]) -> CGEventFlags {
        CGEventFlags::from_bits_retain(bits.iter().fold(0, |all, bit| all | bit))
    }

    /// Gerätebits der linken Sondertasten (`NX_DEVICELCTLKEYMASK` …).
    const LEFT_CONTROL: u64 = 0x01;
    /// Siehe [`LEFT_CONTROL`].
    const LEFT_COMMAND: u64 = 0x08;
    /// Siehe [`LEFT_CONTROL`].
    const LEFT_OPTION: u64 = 0x20;

    /// Allgemeines Bit, linkes und rechtes Gerätebit jeder rechten Taste.
    fn bits_of(key: TriggerKey) -> Option<(u64, u64, u64)> {
        match key {
            TriggerKey::Fn => None,
            TriggerKey::RightOption => Some((
                CGEventFlags::CGEventFlagAlternate.bits(),
                LEFT_OPTION,
                DEVICE_RIGHT_OPTION,
            )),
            TriggerKey::RightCommand => Some((
                CGEventFlags::CGEventFlagCommand.bits(),
                LEFT_COMMAND,
                DEVICE_RIGHT_COMMAND,
            )),
            TriggerKey::RightControl => Some((
                CGEventFlags::CGEventFlagControl.bits(),
                LEFT_CONTROL,
                DEVICE_RIGHT_CONTROL,
            )),
        }
    }

    #[test]
    fn a_right_key_counts_only_with_its_own_bit() {
        for key in TriggerKey::ALL {
            let Some((general, left, right)) = bits_of(key) else {
                assert!(
                    TriggerKey::Fn.is_down(CGEventFlags::CGEventFlagSecondaryFn),
                    "fn über das allgemeine Bit"
                );
                continue;
            };
            assert!(
                !key.is_down(flags(&[general, left])),
                "{key:?}: die linke Taste ist nicht die Diktiertaste"
            );
            assert!(
                key.is_down(flags(&[general, left, right])),
                "{key:?}: rechte gedrückt, auch wenn die linke gehalten wird"
            );
        }
    }

    #[test]
    fn releasing_the_right_key_while_the_left_is_held_is_a_release() {
        for key in TriggerKey::ALL {
            let Some((general, left, right)) = bits_of(key) else {
                continue;
            };
            assert_eq!(
                classify_flags(key.keycode(), flags(&[general, left, right]), key),
                Input::TriggerDown(Modifiers::NONE),
                "{key:?}"
            );
            assert_eq!(
                classify_flags(key.keycode(), flags(&[general, left]), key),
                Input::TriggerUp(Modifiers::NONE),
                "{key:?}"
            );
        }
    }

    #[test]
    fn the_polish_key_is_control_only_when_right_option_dictates() {
        assert_eq!(
            TriggerKey::RightOption.polish_flag(),
            CGEventFlags::CGEventFlagControl
        );
        for key in [
            TriggerKey::Fn,
            TriggerKey::RightCommand,
            TriggerKey::RightControl,
        ] {
            assert_eq!(
                key.polish_flag(),
                CGEventFlags::CGEventFlagAlternate,
                "{key:?}"
            );
        }
        // Die rechte ⌥ diktiert, die linke ⌃ heißt überarbeiten.
        let control = flags(&[CGEventFlags::CGEventFlagControl.bits(), LEFT_CONTROL]);
        assert_eq!(
            classify_flags(TriggerKey::Fn.keycode(), control, TriggerKey::RightOption),
            Input::ModifiersChanged(OPTION)
        );
        assert_eq!(
            classify_flags(TriggerKey::RightControl.keycode(), control, TriggerKey::Fn),
            Input::ModifiersChanged(Modifiers::NONE),
            "bei fn ist ⌃ keine Taste zum Überarbeiten"
        );
    }

    #[test]
    fn another_keycode_only_changes_the_modifiers() {
        let shift = flags(&[CGEventFlags::CGEventFlagShift.bits()]);
        for key in TriggerKey::ALL {
            let other = TriggerKey::ALL
                .into_iter()
                .find(|other| *other != key)
                .unwrap();
            assert_eq!(
                classify_flags(other.keycode(), shift, key),
                Input::ModifiersChanged(SHIFT),
                "{key:?}"
            );
        }
    }

    #[test]
    fn escape_is_caught_only_while_capturing() {
        let detector = Cell::new(TapDetector::new());
        let capturing = AtomicBool::new(true);
        let reaction = on_key_down(KEYCODE_ESCAPE, &detector, &capturing);
        assert_eq!(reaction.event, Some(HotkeyEvent::Escape));
        assert!(matches!(reaction.result, CallbackResult::Drop));
        capturing.store(false, Ordering::Relaxed);
        let passed = on_key_down(KEYCODE_ESCAPE, &detector, &capturing);
        assert_eq!(passed.event, None);
        assert!(matches!(passed.result, CallbackResult::Keep));
    }

    /// Platz jeder Taste in einer Tabelle; der Compiler verlangt jede.
    const fn index(key: TriggerKey) -> usize {
        match key {
            TriggerKey::Fn => 0,
            TriggerKey::RightOption => 1,
            TriggerKey::RightCommand => 2,
            TriggerKey::RightControl => 3,
        }
    }

    #[test]
    fn all_lists_every_key() {
        let mut seen = [false; TriggerKey::ALL.len()];
        for key in TriggerKey::ALL {
            seen[index(key)] = true;
        }
        assert!(
            seen.iter().all(|seen| *seen),
            "ALL fehlt eine Taste: {seen:?}"
        );
    }
}
