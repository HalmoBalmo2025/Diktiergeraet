//! Die Uhr: das eine Modul, das die Systemzeit liest und wartet.
//!
//! Wer Zeit braucht (Pausen, Leerlauf, Fristen), bekommt eine [`Clock`];
//! Tests geben eine Uhr, die sie selbst weiterstellen, und warten so nie
//! wirklich (testing.md).

use std::time::{Duration, Instant};

/// Eine Uhr: wie spät es ist und wie gewartet wird.
pub(crate) trait Clock {
    /// Jetzt. `Instant` steht still, während der Mac schläft: nur für
    /// Abstände innerhalb eines Laufs.
    fn now(&self) -> Instant;

    /// Wartet `duration`.
    fn sleep(&self, duration: Duration);
}

/// Die Uhr des Systems.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    #[expect(
        clippy::disallowed_methods,
        reason = "die eine Stelle, an der die Systemzeit gelesen wird"
    )]
    fn now(&self) -> Instant {
        Instant::now()
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "die eine Stelle, an der wirklich gewartet wird"
    )]
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Datum und Uhrzeit in der Zeitzone des Macs, für Zeitstempel im
/// Protokoll. Keine Abstände damit messen: Die Uhr des Systems kann
/// springen.
#[expect(
    clippy::disallowed_methods,
    reason = "die eine Stelle, an der die Ortszeit gelesen wird"
)]
pub(crate) fn local_time() -> jiff::Zoned {
    jiff::Zoned::now()
}

/// Eine Uhr für Tests: steht, bis jemand wartet ([`Clock::sleep`] stellt
/// sie weiter) oder sie mit [`FakeClock::advance`] weiterstellt.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct FakeClock {
    now: std::sync::Mutex<Instant>,
}

#[cfg(test)]
impl FakeClock {
    /// Eine Uhr, die bei der jetzigen Systemzeit steht.
    pub(crate) fn new() -> Self {
        Self {
            now: std::sync::Mutex::new(SystemClock.now()),
        }
    }

    /// Stellt die Uhr um `duration` weiter.
    pub(crate) fn advance(&self, duration: Duration) {
        let mut now = self.now.lock().unwrap();
        *now += duration;
    }
}

#[cfg(test)]
impl Clock for FakeClock {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap()
    }

    fn sleep(&self, duration: Duration) {
        self.advance(duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fake_clock_moves_only_when_told() {
        let clock = FakeClock::new();
        let start = clock.now();
        assert_eq!(clock.now(), start);
        clock.sleep(Duration::from_secs(3));
        clock.advance(Duration::from_secs(2));
        assert_eq!(clock.now().duration_since(start), Duration::from_secs(5));
    }
}
