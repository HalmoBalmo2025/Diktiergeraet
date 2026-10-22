//! Die Menüleisten-App: Steuerung, Modell-Thread und Anzeige.
//!
//! Threads und ihre Aufgaben:
//! - Hauptthread: liest Befehle der Oberfläche ([`ui`], Swift-Hilfsprogramm
//!   für Menüleisten-Symbol und Panel; in `main.rs`)
//! - Tasten-Thread: [`crate::hotkey::listen`]
//! - Steuerung: [`controller::Controller`] – Zustand, Aufnahme, Einfügen,
//!   [`history`] der letzten Diktate
//! - Modell-Thread: [`worker`] – hält das Modell und transkribiert
//! - Überarbeiten-Thread: [`crate::polish`] – fn + ⌥, hält ein lokales
//!   Sprachmodell
//! - Rückmeldung: [`crate::feedback`] – Klänge und Mitteilungen
//! - Taktgeber (`main.rs`): Pegel und fertige Teile während der Aufnahme
//! - Wächter-Thread: [`crate::watchdog::watch_parent`] – endet der Wächter,
//!   endet die App
//! - Schreib-Thread der Oberfläche: [`ui`]
//! - Lade-Thread je langem Transkript: [`source`] (`YouTube`, Datei)

pub(crate) mod controller;
pub(crate) mod history;
pub(crate) mod source;
pub(crate) mod status;
pub(crate) mod ui;
pub(crate) mod worker;
