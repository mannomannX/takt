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

use takt_rt_core::Tick;

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
    dropped: u32,
    sent: u32,
    lossless: bool,
}

impl<P: Port> Telemetry<P> {
    /// Ueber einer Leitung, mit dem Ring, den der Wirt stellt: Die Puffer
    /// der Telemetrie liegen nicht in der Arena (12.11), und als Wert
    /// laege der Ring auf dem Stack, unter jedem Tick (FB-458).
    pub fn new(port: P, ring: &'static mut [u8]) -> Self {
        Telemetry { port, ring, head: 0, len: 0, dropped: 0, sent: 0, lossless: false }
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
    /// faellt es weg.
    pub fn write_byte(&mut self, b: u8) {
        let cap = self.ring.len();
        if self.len == cap {
            if self.lossless {
                self.drain(DRAIN_ROUNDS);
            } else {
                self.flush();
            }
            if self.len == cap {
                self.dropped = self.dropped.saturating_add(1);
                return;
            }
        }
        // `head` und `len` liegen unter `cap`: Ein Abzug genuegt, und eine
        // Division mit veraenderlichem Teiler waere auf RV32 ein Aufruf.
        let at = self.head.saturating_add(self.len);
        match self.ring.get_mut(if at >= cap { at.saturating_sub(cap) } else { at }) {
            Some(slot) => {
                *slot = b;
                self.len = self.len.saturating_add(1);
                self.sent = self.sent.saturating_add(1);
            }
            None => self.dropped = self.dropped.saturating_add(1),
        }
    }

    /// Gibt an die Leitung, was sie nimmt, und schliesst das Paket ab.
    pub fn flush(&mut self) {
        while self.len > 0 && self.ring.get(self.head).is_some_and(|b| self.port.try_write(*b)) {
            self.head = self.head.saturating_add(1);
            if self.head == self.ring.len() {
                self.head = 0;
            }
            self.len = self.len.saturating_sub(1);
        }
        self.port.flush();
    }

    /// Leert den Ring mit hoechstens `rounds` Anlaeufen; wahr, wenn er leer ist.
    pub fn drain(&mut self, rounds: u32) -> bool {
        for _ in 0..rounds {
            self.flush();
            if self.len == 0 {
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
            if self.len == 0 && self.port.idle() {
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
        self.sent = 0;
    }

    /// Wie viele Bytes seit [`Telemetry::mark`] in den Ring gingen; die
    /// Abschlusszeile nennt sie, und der Wirt vergleicht mit dem, was ankam.
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
        // Von hinten; die Ziffern einer u64 passen in 20 Byte.
        let mut buf = [0u8; 20];
        let mut used = 0usize;
        let mut slots = buf.iter_mut().rev();
        let mut put = |d: u8| {
            if let Some(slot) = slots.next() {
                *slot = d;
                used = used.saturating_add(1);
            }
        };
        // In u64 nur, bis der Rest in u32 passt: Auf RV32 ist die Division
        // in u64 ein Aufruf, die in u32 ein Befehl.
        let mut wide = n;
        while wide > u64::from(u32::MAX) {
            put(decimal_digit(wide % 10));
            wide /= 10;
        }
        let mut narrow = u32::try_from(wide).unwrap_or(u32::MAX);
        loop {
            put(decimal_digit(u64::from(narrow % 10)));
            narrow /= 10;
            if narrow == 0 {
                break;
            }
        }
        for b in buf.get(buf.len().saturating_sub(used)..).unwrap_or(&[]) {
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

/// Fuer `write!`: Fliesskommazahlen kommen aus `core::fmt`, das die
/// kuerzeste Ziffernfolge druckt, die den Wert eindeutig zurueckgibt (4.2).
impl<P: Port> core::fmt::Write for Telemetry<P> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write(s);
        Ok(())
    }
}

/// Die Hexziffer der unteren vier Bit von `n`, klein geschrieben.
pub(crate) fn hex_digit(n: u8) -> u8 {
    b"0123456789abcdef".get(usize::from(n & 15)).copied().unwrap_or(b'?')
}

/// Die Dezimalziffer von `d`; der Aufrufer gibt `d < 10`.
fn decimal_digit(d: u64) -> u8 {
    usize::try_from(d).ok().and_then(|i| b"0123456789".get(i)).copied().unwrap_or(b'?')
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

    /// Die Bilanz zaehlt ab der Marke, was in den Ring ging — Verworfenes
    /// nicht, denn das kommt nie an.
    #[test]
    fn the_count_starts_at_the_mark_and_skips_dropped_bytes() {
        let mut t = Telemetry::new(line(64), ring(64));
        t.write("Kopf\r\n");
        t.mark();
        t.write("t=1 out a 1\r\n");
        assert_eq!(t.sent(), 13);
        let mut full = Telemetry::new(line(0), ring(16));
        full.mark();
        full.write("0123456789");
        assert_eq!((full.sent(), full.dropped()), (4, 6));
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
