//! Die rechnende Haelfte der Board-Unterstuetzung (12.3, 13.8).
//!
//! **Warum dieses Crate getrennt steht.** Ein Board-Crate braucht
//! `unsafe` fuer Registerzugriffe und liegt darum ausserhalb des
//! Workspace (9.5: Treiber sind Trusted Computing Base). Damit waere auch
//! alles andere dort — und das meiste ist gar kein Registerzugriff,
//! sondern Arithmetik: Perioden in Timer-Schritte, Zyklen in
//! Nanosekunden, zwei 32-Bit-Zaehler zu einem 64-Bit-Wert.
//!
//! Diese Rechnungen sind die Stellen, an denen Fehler *still* bleiben:
//! Ein falscher Prescaler laesst das Programm laufen, nur mit falscher
//! Zeit. Sie gehoeren darum dorthin, wo sie getestet werden koennen — auf
//! den Wirt, in den Workspace, unter `forbid`.
//!
//! Was drueben bleibt, ist, was ohne Hardware sinnlos waere: ein Register
//! zu schreiben. Die Ausnahme wird dadurch so klein, wie sie sein muss.

#![no_std]

pub mod clock;
pub mod console;
pub mod counter;
pub mod cycles;
pub mod fifo;
pub mod platform;
pub mod pll;
pub mod uart;
pub mod watchdog;

pub use clock::{PeriodError, counts_for, ns_per_count, prescaler_for};
pub use counter::Counter64;
pub use cycles::Measurement;
pub use fifo::ByteFifo;
