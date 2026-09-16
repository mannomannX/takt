//! Der Profilaufsatz `linux_rt` (12.2, 12.8).
//!
//! **Was dieses Crate tut und was nicht.** 12.2 verlangt vier Dinge vom
//! Tick-Thread: `SCHED_FIFO` auf einem isolierten Core (`isolcpus`,
//! `nohz_full`), `mlockall`, vorab beruehrte Seiten und einen periodischen
//! Timer mit absoluten Deadlines.
//!
//! Drei davon tut die Runtime selbst. Das vierte — `SCHED_FIFO` — tut sie
//! **nicht**, und das ist eine Entscheidung:
//!
//! - `isolcpus` und `nohz_full` in derselben Zeile sind
//!   Kernel-Bootparameter. Kein Programm setzt sie; sie sind
//!   Betriebskonfiguration.
//! - Ein Programm, das sich selbst Echtzeitprioritaet gibt, braucht
//!   `CAP_SYS_NICE` oder root. Das hiesse, ein Binaer auszuliefern, das
//!   privilegiert laufen *muss* — schlechter fuer Sicherheit und Betrieb
//!   als `chrt -f 80 ./programm`.
//! - Linux hat fuer genau diesen Zweck ein Standardwerkzeug, und 8.10
//!   hat fuer solche Angaben bereits einen Ort.
//!
//! **Stattdessen misst die Runtime, was sie bekommen hat, und sagt es.**
//! Eine Zeitgarantie, die still ausfaellt, ist schlimmer als eine, die
//! fehlt: Eine Messung unter ihr sieht gueltig aus. [`Guarantee`] steht
//! darum im Lauf-Header (11.3) — der Unterschied zwischen „lief unter
//! `linux_rt`" und „lief unter `linux_rt` *mit* der Zusage" ist in der
//! Aufzeichnung sichtbar, nicht angenommen.

pub mod clock;
pub mod guarantee;
pub mod jobs;
pub mod nvm;
pub mod tunables;

pub use clock::{RealtimeClock, wall_clock_ns};
pub use guarantee::{Guarantee, Scheduling, prepare};
pub use jobs::ThreadJobs;
pub use nvm::FileNvm;
pub use tunables::FileTunables;
