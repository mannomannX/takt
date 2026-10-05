//! Ein Fuellventil als Takt-Programm in einem Rust-Projekt (12.11).
//!
//! `build.rs` uebersetzt `takt/valve.takt` fuer das Ziel des Baus; das
//! Modul [`valve`] bringt Arena, Konstanten, den Trait `Drivers` und die
//! Huelle `Program`. Dieses Crate stellt nur die Geraete: [`Tank`].
//!
//! **Was die Huelle zusichert, haelt der Uebersetzer** (12.11): Die Arena
//! und das Treiberobjekt sind geliehen, solange das Programm lebt.
//!
//! ```
//! let mut arena = rust_host::valve::Arena::new();
//! let mut tank = rust_host::Tank::default();
//! let mut program = rust_host::valve::Program::init(&mut arena, &mut tank);
//! let jobs = program.jobs();
//! drop((jobs, program));
//! drop(arena);
//! ```
//!
//! Zwei Programme auf derselben Arena uebersetzen nicht:
//!
//! ```compile_fail,E0499
//! let mut arena = rust_host::valve::Arena::new();
//! let (mut t1, mut t2) = (rust_host::Tank::default(), rust_host::Tank::default());
//! let a = rust_host::valve::Program::init(&mut arena, &mut t1);
//! let b = rust_host::valve::Program::init(&mut arena, &mut t2);
//! drop((a, b));
//! ```
//!
//! Ein Zugriff des Wirts in die Arena, waehrend das Programm lebt, ebenso:
//!
//! ```compile_fail,E0502
//! let mut arena = rust_host::valve::Arena::new();
//! let mut tank = rust_host::Tank::default();
//! let program = rust_host::valve::Program::init(&mut arena, &mut tank);
//! let size = core::mem::size_of_val(&arena);
//! drop((program, size));
//! ```
//!
//! Ein Verschieben der Arena ebenso:
//!
//! ```compile_fail,E0505
//! let mut arena = rust_host::valve::Arena::new();
//! let mut tank = rust_host::Tank::default();
//! let program = rust_host::valve::Program::init(&mut arena, &mut tank);
//! let moved = arena;
//! drop((program, moved));
//! ```
//!
//! Der Griff des Job-Kontexts lebt nicht laenger als die Arena:
//!
//! ```compile_fail,E0597
//! let jobs = {
//!     let mut arena = rust_host::valve::Arena::new();
//!     let mut tank = rust_host::Tank::default();
//!     let mut program = rust_host::valve::Program::init(&mut arena, &mut tank);
//!     program.jobs()
//! };
//! drop(jobs);
//! ```
//!
//! Und ein Treiberobjekt ohne die Methode eines Kanals uebersetzt nicht
//! (12.6: starke Bindung); die Meldung nennt `out_tank_valve`:
//!
//! ```compile_fail,E0046
//! struct Half;
//! impl rust_host::valve::Drivers for Half {
//!     fn alive_tank(&mut self, _now: i64) -> bool {
//!         true
//!     }
//! }
//! ```

/// Das Programm (erzeugt): `Arena`, `TICK_NS`, `Drivers`, `Program`.
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod valve {
    include!(env!("TAKT_VALVE_RS"));
}

/// Ein interner Strom mit `drop_oldest` (8.6), ohne Treiber.
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod drop_oldest {
    include!(env!("TAKT_DROP_OLDEST_RS"));
}

/// Eine gescopte Instanz, deren Besitzer ueber einen Fault geht (5.11), ohne Treiber.
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod exit_fault {
    include!(env!("TAKT_EXIT_FAULT_RS"));
}

/// Ein Ueberlauf in einem inaktiven Tick (8.6, 9.4).
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod overflow_late {
    include!(env!("TAKT_OVERFLOW_LATE_RS"));
}

/// Ein Ueberlauf, der einen Leser in FAULTED trifft (9.3, 9.6).
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod faulted_pending {
    include!(env!("TAKT_FAULTED_PENDING_RS"));
}

/// Ein Pruefstand nach Drehbuch: `u/rx` liefert je Element zu seinem Tick
/// ein Byte, `u/resume` meldet zu seinem Tick `true`. Der Tick ist der des
/// Traces; derselbe Stimulus steht fuer den Interpreter daneben.
#[derive(Debug, Default)]
pub struct Script {
    /// Die Elemente von `u/rx`: Tick und Byte, in Lieferreihenfolge.
    pub rx: Vec<(u64, u8)>,
    /// Wann `u/resume` meldet.
    pub resume_at: Option<u64>,
    /// Die Periode T0, um aus der Tickgrenze den Tick zu machen.
    pub tick_ns: i64,
}

impl Script {
    /// Der Tick des Traces zur Tickgrenze `now`: Tick `k` tastet an `k * T0` ab.
    fn tick(&self, now: i64) -> u64 {
        u64::try_from(now / self.tick_ns.max(1)).unwrap_or(0)
    }

    /// Das naechste Element dieses Ticks, wenn eines wartet.
    fn next(&mut self, buf: &mut [u8], now: i64) -> Option<takt_embed::Piece> {
        let k = self.tick(now);
        let i = self.rx.iter().position(|(t, _)| *t == k)?;
        let (_, byte) = self.rx.remove(i);
        *buf.first_mut()? = byte;
        Some(takt_embed::Piece { len: 1, t: now, seq: None })
    }
}

impl overflow_late::Drivers for Script {
    fn poll_u_rx(&mut self, buf: &mut [u8], now: i64) -> Option<takt_embed::Piece> {
        self.next(buf, now)
    }
}

impl faulted_pending::Drivers for Script {
    fn in_u_resume(&mut self, now: i64) -> Option<takt_embed::Sample<bool>> {
        (self.resume_at == Some(self.tick(now))).then(|| takt_embed::Sample::good(true, now))
    }

    fn poll_u_rx(&mut self, buf: &mut [u8], now: i64) -> Option<takt_embed::Piece> {
        self.next(buf, now)
    }
}

/// Eine Zeile ueber `line<16>` (3.9).
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod long_line {
    include!(env!("TAKT_LONG_LINE_RS"));
}

/// Ein Geraet, das zu einem Tick eine Zeile liefert, so lang, wie es will.
#[derive(Debug)]
pub struct Line {
    /// Der Tick des Traces, zu dem die Zeile kommt.
    pub at: u64,
    /// Die Zeile.
    pub text: &'static [u8],
    /// Die Periode T0.
    pub tick_ns: i64,
}

impl long_line::Drivers for Line {
    fn poll_u_rx(&mut self, buf: &mut [u8], now: i64) -> Option<takt_embed::Piece> {
        if u64::try_from(now / self.tick_ns.max(1)).ok()? != self.at {
            return None;
        }
        self.at = u64::MAX;
        let n = self.text.len().min(buf.len());
        buf[..n].copy_from_slice(&self.text[..n]);
        Some(takt_embed::Piece { len: self.text.len(), t: now, seq: None })
    }
}

/// Das Ventil am Tank: stellt, was das Programm verlangt, und zaehlt, wie
/// oft es sich geoeffnet hat.
#[derive(Debug, Default)]
pub struct Tank {
    /// Ist das Ventil offen?
    pub open: bool,
    /// Wie oft es sich geoeffnet hat.
    pub openings: u32,
}

impl valve::Drivers for Tank {
    fn out_tank_valve(&mut self, value: bool, _now: i64) -> bool {
        self.openings += u32::from(value && !self.open);
        self.open = value;
        true
    }

    fn alive_tank(&mut self, _now: i64) -> bool {
        true
    }
}

/// Die Eingaenge des Geraets `sys` vom Wirt (12.7, 12.11).
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod sys_inputs {
    include!(env!("TAKT_SYS_INPUTS_RS"));
}

/// Ein Wirt, der `previous_run` und die Wanduhr stellt, und ein Knopf.
#[derive(Debug)]
pub struct Host {
    /// Wie der vorige Lauf endete, als Diskriminante von `PreviousRun`.
    pub previous_run: u32,
    /// Die Wanduhr zur Tickgrenze 0.
    pub wall_at_zero: i64,
    /// Wann der Knopf gedrueckt ist.
    pub pressed_at: Option<u64>,
    /// Die Periode T0.
    pub tick_ns: i64,
}

impl sys_inputs::Drivers for Host {
    fn in_u_button(&mut self, now: i64) -> Option<takt_embed::Sample<bool>> {
        let k = u64::try_from(now / self.tick_ns.max(1)).ok()?;
        (self.pressed_at == Some(k)).then(|| takt_embed::Sample::good(true, now))
    }
}

impl sys_inputs::Sys for Host {
    fn previous_run(&mut self, now: i64) -> Option<takt_embed::Sample<u32>> {
        Some(takt_embed::Sample::good(self.previous_run, now))
    }

    fn clock(&mut self, now: i64) -> Option<takt_embed::Sample<i64>> {
        Some(takt_embed::Sample::good(self.wall_at_zero.saturating_add(now), now))
    }
}

/// Gleitkommazahlen in Stromelementen (5.9, 8.6).
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod float_elements {
    include!(env!("TAKT_FLOAT_ELEMENTS_RS"));
}

/// Ein Geraet, das je Tick ein Element in kanonischer Byteform liefert:
/// die Bits eines `f64`, auch solche, die keine endliche Zahl sind.
#[derive(Debug, Default)]
pub struct Bits {
    /// Tick des Traces und Bits des Elements, in Lieferreihenfolge.
    pub elements: Vec<(u64, u64)>,
    /// Die Periode T0.
    pub tick_ns: i64,
}

impl float_elements::Drivers for Bits {
    fn poll_u_rx(&mut self, buf: &mut [u8], now: i64) -> Option<takt_embed::Piece> {
        let k = u64::try_from(now / self.tick_ns.max(1)).ok()?;
        let i = self.elements.iter().position(|(t, _)| *t == k)?;
        let (_, bits) = self.elements.remove(i);
        buf.get_mut(..8)?.copy_from_slice(&bits.to_le_bytes());
        Some(takt_embed::Piece { len: 8, t: now, seq: None })
    }
}

/// Ein Lauf mit `persist`, Job, Schlaf und `next_run` (12.11), ohne Treiber.
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod lifecycle {
    include!(env!("TAKT_LIFECYCLE_RS"));
}

/// Schlaf ueber Maschinen mit eigener Periode und Phase (9.9, 7.2), ohne Treiber.
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod idle_multirate {
    include!(env!("TAKT_IDLE_MULTIRATE_RS"));
}

/// Tunables ueber den Weg der Schleife (8.4), ohne Treiber.
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod tuning {
    include!(env!("TAKT_TUNING_RS"));
}

/// Der Byte-Ring eines internen Stroms laeuft um (8.6, 9.6), ohne Treiber.
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod byte_ring {
    include!(env!("TAKT_BYTE_RING_RS"));
}

/// Ein Ausgabestrom, dessen Treiber den Sendepuffer ueberfaehrt (12.6 Zeile 6).
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod overfull_tx {
    include!(env!("TAKT_OVERFULL_TX_RS"));
}

/// Ein Sender, der zu einem Tick mehr freien Platz meldet, als der Strom
/// fasst.
#[derive(Debug)]
pub struct Overfull {
    /// Der Tick des Traces, zu dem er zu viel meldet.
    pub at: u64,
    /// Die Kapazitaet des Stroms.
    pub capacity: u32,
    /// Die Periode T0.
    pub tick_ns: i64,
}

impl overfull_tx::Drivers for Overfull {
    fn free_u_tx(&mut self, now: i64) -> Option<u32> {
        let k = u64::try_from(now / self.tick_ns.max(1)).ok()?;
        Some(if k == self.at { self.capacity + 1 } else { self.capacity })
    }

    fn alive_u(&mut self, _now: i64) -> bool {
        true
    }
}
