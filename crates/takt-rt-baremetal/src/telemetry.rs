//! Telemetrie ueber einen Ringpuffer (12.2, 12.3), fuer jedes Board.
//!
//! Die Leitung ist langsamer als der Tick, und ein Byte, das auf sie
//! wartet, haelt die Steuerung an (FB-227). Darum schreibt das Programm
//! in einen Ring, und [`Telemetry::flush`] gibt einmal je Tick ab, was
//! die Leitung nimmt — ohne zu warten. Ist der Ring voll, wird verworfen
//! und gezaehlt; 12.2: „wer nicht mitkommt, verwirft und zaehlt, statt
//! die Steuerung aufzuhalten". Zahlen entstehen ohne `core::fmt` und in
//! 32-Bit-Schritten, weil die 64-Bit-Division auf RV32 und Cortex-M4 ein
//! Bibliotheksaufruf ist.
//!
//! Das Board liefert nur die Leitung ([`Port`]): ein Byte, wenn Platz
//! ist, und den Abschluss eines Pakets.
//!
//! **Fliesskomma formatiert der Leitungspfad** (12.5): Ein Haken legt Marke
//! und Bits in den Ring ([`takt_rt_core::digits::float_record`]), und erst
//! [`Telemetry::flush`] schreibt die Textform, wenn die Zahl an der Reihe
//! ist. Was die Leitung von ihr noch nicht nahm, wartet im Ueberhang. Im
//! Schritt bleibt so je Wert feste Arbeit; der Formatierer laeuft am Ende des
//! Ticks ([`Telemetry::after_tick`]). Nur im Lauf in logischer Zeit, der
//! nichts verwerfen darf, laeuft er schon im Schritt, wenn der Trace eines
//! einzigen Ticks den Ring fuellt. Gezaehlt wird darum, was an die Leitung
//! ging, nicht, was in den Ring kam.

use takt_rt_core::Tick;
pub(crate) use takt_rt_core::digits::hex as hex_digit;
use takt_rt_core::digits::{self, FLOAT_MARK, FLOAT_RECORD};

/// Anlaeufe, den Ring zu leeren, wo der Tick nicht mehr zaehlt (Ende,
/// Panic): reichlich fuer einen lesenden Host, endlich ohne ihn.
pub const DRAIN_ROUNDS: u32 = 1_000_000;

/// Die Leitung eines Boards.
pub trait Port {
    /// Nimmt ein Byte an, wenn Platz ist — ohne zu warten.
    fn try_write(&mut self, b: u8) -> bool;

    /// Schickt ab, was die Leitung gesammelt hat (ein USB-Paket).
    fn flush(&mut self) {}

    /// Ob alles, was die Leitung angenommen hat, hinaus ist und nicht mehr
    /// in ihrem eigenen Puffer steht (FB-314).
    fn idle(&mut self) -> bool {
        true
    }

    /// Wie oft die Gegenstelle die Leitung angehalten hat, etwa mit `XOFF`
    /// (FB-306). Verwirft die Telemetrie, waehrend die Leitung stand, las der
    /// Wirt nicht mit; eine Leitung ohne Rueckstau haelt nie an.
    fn halts(&self) -> u32 {
        0
    }
}

/// Der Ring vor der Leitung.
pub struct Telemetry<P: Port> {
    port: P,
    ring: &'static mut [u8],
    head: usize,
    len: usize,
    /// So viele Byte am Anfang des Rings kamen vor der Marke `takt trace`
    /// und zaehlen nicht ([`Telemetry::sent`]).
    uncounted: usize,
    /// Die Textform der Zahl, die zuletzt an der Reihe war, und wie viel
    /// davon die Leitung schon nahm.
    spill: digits::Text,
    spilled: u8,
    /// Ob der Ueberhang nach der Marke kam und zaehlt.
    spill_counts: bool,
    /// So viele Byte einer verworfenen Zahl folgen noch und fallen mit weg.
    skipping: u8,
    dropped: u32,
    sent: u32,
    lossless: bool,
}

impl<P: Port> Telemetry<P> {
    /// Ueber einer Leitung, mit dem Ring, den der Wirt stellt: Die Puffer
    /// der Telemetrie liegen nicht in der Arena (12.11), und als Wert
    /// laege der Ring auf dem Stack, unter jedem Tick (FB-458).
    pub fn new(port: P, ring: &'static mut [u8]) -> Self {
        Telemetry {
            port,
            ring,
            head: 0,
            len: 0,
            uncounted: 0,
            spill: digits::Text::default(),
            spilled: 0,
            spill_counts: false,
            skipping: 0,
            dropped: 0,
            sent: 0,
            lossless: false,
        }
    }

    /// Verlustfrei: Ist der Ring voll, wartet [`Telemetry::write_byte`], bis
    /// die Leitung ihn geleert hat, statt zu verwerfen — hoechstens
    /// [`DRAIN_ROUNDS`] Anlaeufe lang. Nur fuer Laeufe, in denen die Zeit
    /// nicht zaehlt (13.8, [`crate::LogicalClock`]); im Betrieb verbietet
    /// 12.2 das Warten.
    pub fn lossless(mut self) -> Self {
        self.lossless = true;
        self
    }

    /// Ein Byte in den Ring; ist er voll, geht erst die Leitung, dann
    /// faellt es weg. Eine Zahl, [`FLOAT_MARK`] und ihre Bits, kommt ganz
    /// hinein oder gar nicht: Halb gelesen waere sie eine andere.
    pub fn write_byte(&mut self, b: u8) {
        if self.skipping > 0 {
            self.skipping = self.skipping.saturating_sub(1);
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let need = if b == FLOAT_MARK { FLOAT_RECORD } else { 1 };
        if self.room() < need {
            if self.lossless {
                self.drain(DRAIN_ROUNDS);
            } else {
                // Im Betrieb formatiert der Schritt nie: Die Leitung bekommt
                // nur, was schon Text ist.
                self.pass(false);
                self.port.flush();
            }
            if self.room() < need {
                self.dropped = self.dropped.saturating_add(1);
                self.skipping = u8::try_from(need.saturating_sub(1)).unwrap_or(u8::MAX);
                return;
            }
        }
        match self.ring.get_mut(self.index(self.len)) {
            Some(slot) => {
                *slot = b;
                self.len = self.len.saturating_add(1);
            }
            None => self.dropped = self.dropped.saturating_add(1),
        }
    }

    /// Eine Fliesskommazahl: Marke und Bits, die Textform schreibt
    /// [`Telemetry::flush`].
    pub fn write_f64(&mut self, x: f64) {
        for b in digits::float_record(x) {
            self.write_byte(b);
        }
    }

    /// Der freie Platz im Ring.
    fn room(&self) -> usize {
        self.ring.len().saturating_sub(self.len)
    }

    /// Die Stelle im Ring `i` Byte hinter dem Kopf, fuer `i` hoechstens die
    /// Groesse des Rings: `head` liegt darunter, ein Abzug genuegt, und eine
    /// Division mit veraenderlichem Teiler waere auf RV32 ein Aufruf.
    fn index(&self, i: usize) -> usize {
        let (cap, at) = (self.ring.len(), self.head.saturating_add(i));
        if at >= cap { at.saturating_sub(cap) } else { at }
    }

    /// Gibt an die Leitung, was sie nimmt, und schliesst das Paket ab.
    pub fn flush(&mut self) {
        self.pass(true);
        self.port.flush();
    }

    /// Am Ende eines Ticks (12.2): gibt an die Leitung, was sie nimmt.
    /// Verlustfrei wartet es, bis der Ring leer ist; so schreibt der naechste
    /// Tick in einen leeren und formatiert seine Zahlen nicht schon selbst.
    pub fn after_tick(&mut self) {
        if self.lossless {
            self.drain(DRAIN_ROUNDS);
        } else {
            self.flush();
        }
    }

    /// Gibt der Leitung Byte, solange sie nimmt. Ist eine Zahl an der
    /// Reihe, schreibt nur mit `numbers` ihre Textform in den Ueberhang.
    fn pass(&mut self, numbers: bool) {
        loop {
            if let Some(&b) = self.spill.as_bytes().get(usize::from(self.spilled)) {
                if !self.port.try_write(b) {
                    return;
                }
                self.spilled = self.spilled.saturating_add(1);
                if self.spill_counts {
                    self.sent = self.sent.saturating_add(1);
                }
                continue;
            }
            let Some(&b) = self.ring.get(self.head).filter(|_| self.len > 0) else { return };
            if b == FLOAT_MARK {
                if !numbers || self.len < FLOAT_RECORD {
                    return;
                }
                let mut bits = [0u8; 8];
                for (i, slot) in (1..).zip(bits.iter_mut()) {
                    *slot = self.ring.get(self.index(i)).copied().unwrap_or(0);
                }
                self.spill_counts = self.take(FLOAT_RECORD);
                self.spill = digits::float64_text(f64::from_le_bytes(bits));
                self.spilled = 0;
            } else if self.port.try_write(b) {
                if self.take(1) {
                    self.sent = self.sent.saturating_add(1);
                }
            } else {
                return;
            }
        }
    }

    /// Nimmt `n` Byte vom Kopf des Rings; wahr, wenn sie nach der Marke
    /// kamen und zaehlen.
    fn take(&mut self, n: usize) -> bool {
        let counts = self.uncounted == 0;
        self.uncounted = self.uncounted.saturating_sub(n);
        self.head = self.index(n);
        self.len = self.len.saturating_sub(n);
        counts
    }

    /// Ob nichts mehr auf die Leitung wartet, weder im Ring noch im Ueberhang.
    fn is_empty(&self) -> bool {
        self.len == 0 && usize::from(self.spilled) >= self.spill.as_bytes().len()
    }

    /// Leert den Ring mit hoechstens `rounds` Anlaeufen; wahr, wenn er leer ist.
    pub fn drain(&mut self, rounds: u32) -> bool {
        for _ in 0..rounds {
            self.flush();
            if self.is_empty() {
                return true;
            }
        }
        false
    }

    /// Wie [`Telemetry::drain`], und dazu, bis die Leitung auch ihren
    /// eigenen Puffer abgegeben hat; wahr, wenn alles hinaus ist. Vor einem
    /// Reset oder Tiefschlaf (12.7), der diesen Puffer mitnaehme (FB-314).
    pub fn finish(&mut self, rounds: u32) -> bool {
        for _ in 0..rounds {
            self.flush();
            if self.is_empty() && self.port.idle() {
                return true;
            }
        }
        false
    }

    /// Wie viele Bytes ohne Platz verworfen wurden.
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Wie oft die Gegenstelle die Leitung angehalten hat ([`Port::halts`]).
    pub fn halts(&self) -> u32 {
        self.port.halts()
    }

    /// Die Zeile `takt trace`, ab der [`Telemetry::sent`] zaehlt: Was davor
    /// kommt, gehoert nicht sicher zum Lauf — Bootmeldungen, ein Kopf, den
    /// der Wirt je nach Start verpasst (FB-304).
    pub fn mark(&mut self) {
        self.write("takt trace");
        self.newline();
        self.uncounted = self.len;
        self.spill_counts = false;
        self.sent = 0;
    }

    /// Wie viele Bytes seit [`Telemetry::mark`] an die Leitung gingen; die
    /// Abschlusszeile nennt sie, und der Wirt vergleicht mit dem, was ankam.
    /// Eine Zahl zaehlt mit ihrer Textform, erst wenn sie hinaus ist.
    pub fn sent(&self) -> u32 {
        self.sent
    }

    /// Wie viele Bytes noch im Ring stehen.
    pub fn pending(&self) -> usize {
        self.len
    }

    /// Ein Text.
    pub fn write(&mut self, s: &str) {
        for b in s.bytes() {
            self.write_byte(b);
        }
    }

    /// Zeilenende.
    pub fn newline(&mut self) {
        self.write("\r\n");
    }

    /// Eine Zahl, dezimal.
    pub fn write_u64(&mut self, n: u64) {
        let mut buf = [0u8; 20];
        for b in takt_rt_core::digits::decimal(n, &mut buf) {
            self.write_byte(*b);
        }
    }

    /// Eine Zahl mit Vorzeichen, dezimal.
    pub fn write_i64(&mut self, n: i64) {
        if n < 0 {
            self.write_byte(b'-');
        }
        self.write_u64(n.unsigned_abs());
    }

    /// Ein Byte als `0x..`, wie der Interpreter Stromelemente schreibt.
    pub fn write_hex8(&mut self, b: u8) {
        self.write("0x");
        self.write_byte(hex_digit(b >> 4));
        self.write_byte(hex_digit(b & 15));
    }

    /// Die Metazeile eines Ticks (grammar/trace.md, `time`), mit seiner
    /// Nummer im Trace ([`Tick::traced`]).
    pub fn write_time(&mut self, t: &Tick) {
        self.write("t=");
        self.write_u64(t.traced());
        self.write(" time took=");
        self.write_i64(t.took);
        self.write(" drift=");
        self.write_i64(t.drift);
        self.write(" slept=");
        self.write_u64(t.slept);
        self.newline();
    }
}

/// Fuer `write!` des Wirts ausserhalb des Schritts, etwa im Kopf oder vor der
/// Bilanz; der Trace des Programms kommt ueber die Haken.
impl<P: Port> core::fmt::Write for Telemetry<P> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write(s);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::string::String;
    use std::vec::Vec;

    use super::*;

    /// Eine Leitung, die je Paket `cap` Bytes nimmt und ein Byte noch
    /// `lag` Pakete lang in ihrem eigenen Puffer haelt.
    struct Line {
        cap: usize,
        room: usize,
        sent: Vec<u8>,
        packets: usize,
        lag: usize,
        busy: usize,
    }

    impl Port for Line {
        fn try_write(&mut self, b: u8) -> bool {
            if self.room == 0 {
                return false;
            }
            self.room -= 1;
            self.sent.push(b);
            self.busy = self.lag;
            true
        }

        fn flush(&mut self) {
            self.packets += 1;
            self.room = self.cap;
            self.busy = self.busy.saturating_sub(1);
        }

        fn idle(&mut self) -> bool {
            self.busy == 0
        }
    }

    fn line(cap: usize) -> Line {
        Line { cap, room: cap, sent: Vec::new(), packets: 0, lag: 0, busy: 0 }
    }

    fn text(t: &Telemetry<Line>) -> String {
        String::from_utf8(t.port.sent.clone()).unwrap()
    }

    /// Ein Ring von `n` Byte, der den Test ueberlebt.
    fn ring(n: usize) -> &'static mut [u8] {
        std::vec![0; n].leak()
    }

    #[test]
    fn numbers_print_without_core_fmt() {
        let mut t = Telemetry::new(line(64), ring(64));
        for (i, n) in [0, 4_294_967_295, 4_294_967_296, u64::MAX].into_iter().enumerate() {
            if i > 0 {
                t.write(" ");
            }
            t.write_u64(n);
        }
        t.write(" ");
        t.write_i64(-7);
        t.write(" ");
        t.write_i64(i64::MIN);
        t.write(" ");
        t.write_hex8(0x0a);
        assert!(t.drain(4));
        assert_eq!(text(&t), "0 4294967295 4294967296 18446744073709551615 -7 -9223372036854775808 0x0a");
    }

    #[test]
    fn a_full_ring_drops_and_counts() {
        let mut t = Telemetry::new(line(0), ring(8));
        for b in b"0123456789" {
            t.write_byte(*b);
        }
        assert_eq!((t.pending(), t.dropped()), (8, 2));
        assert!(!t.drain(3));
    }

    /// Verlustfrei wartet der Ring auf die Leitung, statt zu verwerfen;
    /// nimmt sie gar nichts, zaehlt er nach der Schranke doch.
    #[test]
    fn a_lossless_ring_waits_for_the_line() {
        let mut t = Telemetry::new(line(3), ring(8)).lossless();
        t.write("0123456789abcdefghij");
        assert!(t.drain(10));
        assert_eq!((text(&t).as_str(), t.dropped()), ("0123456789abcdefghij", 0));
        let mut stuck = Telemetry::new(line(0), ring(4)).lossless();
        stuck.write("0123456");
        assert_eq!(stuck.dropped(), 3);
    }

    #[test]
    fn flush_sends_one_packet_in_order_and_wraps() {
        let mut t = Telemetry::new(line(4), ring(16));
        t.write("abcdefghij");
        t.flush();
        assert_eq!((text(&t).as_str(), t.port.packets, t.pending()), ("abcd", 1, 6));
        t.flush();
        t.flush();
        assert_eq!(text(&t), "abcdefghij");
        t.write("klmnopqrstuvwxyz");
        assert!(t.drain(10));
        assert_eq!(text(&t), "abcdefghijklmnopqrstuvwxyz");
        assert_eq!(t.dropped(), 0);
    }

    /// **Fliesskomma formatiert der Leitungspfad** (12.5, FB-750): Im Ring
    /// stehen Marke und Bits; die Leitung bekommt die Textform aus T2, auch
    /// wenn sie je Paket nur drei Byte nimmt. `gesendet` zaehlt, was nach der
    /// Marke an die Leitung ging, mit der Textform jeder Zahl.
    #[test]
    fn the_line_path_writes_the_text_of_a_float() {
        let mut t = Telemetry::new(line(3), ring(64));
        t.write_f64(-0.5);
        t.mark();
        t.write("x ");
        t.write_f64(1.5e-7);
        t.write(" ");
        t.write_f64(1e30);
        assert_eq!(t.pending(), 9 + 12 + 2 + 9 + 1 + 9);
        assert!(t.drain(20));
        assert_eq!(text(&t), "-0.5takt trace\r\nx 1.5e-7 1.0e30");
        assert_eq!(t.sent(), 15);
    }

    /// **Eine Zahl kommt ganz in den Ring oder gar nicht**: Im Betrieb
    /// verwirft ein voller Ring die Zahl mit ihren Bits, und was danach kommt,
    /// liest sich weiter; formatiert wird dabei nichts.
    #[test]
    fn a_full_ring_drops_a_float_whole() {
        let mut t = Telemetry::new(line(0), ring(13));
        t.write("abcd");
        t.write_f64(2.5);
        t.write_f64(3.0);
        t.write("e");
        assert_eq!((t.pending(), t.dropped(), t.spill.as_bytes()), (13, 10, &[][..]));
        t.port.cap = 64;
        assert!(t.drain(4));
        t.write("f");
        assert!(t.drain(4));
        assert_eq!(text(&t), "abcd2.5f");
    }

    /// Ein leerer Ring heisst nicht, dass alles hinaus ist: `finish` wartet
    /// auch auf den Puffer der Leitung, begrenzt wie `drain`.
    #[test]
    fn finish_waits_until_the_line_is_idle() {
        let mut t = Telemetry::new(Line { lag: 3, ..line(4) }, ring(16));
        t.write("abcdefghij");
        assert!(t.drain(10) && !t.port.idle());
        assert!(t.finish(10) && t.port.idle());
        let mut stuck = Telemetry::new(Line { lag: 100, ..line(4) }, ring(16));
        stuck.write("abc");
        assert!(!stuck.finish(10));
    }

    /// Die Bilanz zaehlt ab der Marke, was an die Leitung ging — Verworfenes
    /// nicht, denn das kommt nie an, und was noch im Ring steht, erst, wenn es
    /// hinaus ist.
    #[test]
    fn the_count_starts_at_the_mark_and_skips_dropped_bytes() {
        let mut t = Telemetry::new(line(64), ring(64));
        t.write("Kopf\r\n");
        t.mark();
        t.write("t=1 out a 1\r\n");
        assert_eq!(t.sent(), 0);
        assert!(t.drain(4));
        assert_eq!(t.sent(), 13);
        let mut full = Telemetry::new(line(0), ring(16));
        full.mark();
        full.write("0123456789");
        assert_eq!((full.sent(), full.dropped()), (0, 6));
        full.port.cap = 64;
        assert!(full.drain(4));
        assert_eq!(full.sent(), 4);
    }

    /// **Die Zeitzeile nennt den Tick des Traces** (grammar/trace.md T1,
    /// FB-687): An seiner Grenze 7 rechnet der Kern Tick 8.
    #[test]
    fn the_time_line_follows_the_trace_grammar() {
        let mut t = Telemetry::new(line(128), ring(128));
        t.write_time(&Tick { k: 7, now: 0, took: 22_000, drift: -125, overrun: false, slept: 3 });
        t.flush();
        assert_eq!(text(&t), "t=8 time took=22000 drift=-125 slept=3\r\n");
    }
}
