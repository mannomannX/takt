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
}

/// Der Ring vor der Leitung.
pub struct Telemetry<P: Port, const N: usize> {
    port: P,
    ring: [u8; N],
    head: usize,
    len: usize,
    dropped: u32,
}

impl<P: Port, const N: usize> Telemetry<P, N> {
    /// Ueber einer Leitung.
    pub fn new(port: P) -> Self {
        Telemetry { port, ring: [0; N], head: 0, len: 0, dropped: 0 }
    }

    /// Ein Byte in den Ring; ist er voll, geht erst die Leitung, dann
    /// faellt es weg.
    pub fn write_byte(&mut self, b: u8) {
        if self.len == N {
            self.flush();
            if self.len == N {
                self.dropped = self.dropped.saturating_add(1);
                return;
            }
        }
        self.ring[(self.head + self.len) % N] = b;
        self.len += 1;
    }

    /// Gibt an die Leitung, was sie nimmt, und schliesst das Paket ab.
    pub fn flush(&mut self) {
        while self.len > 0 && self.port.try_write(self.ring[self.head]) {
            self.head = (self.head + 1) % N;
            self.len -= 1;
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

    /// Wie viele Bytes ohne Platz verworfen wurden.
    pub fn dropped(&self) -> u32 {
        self.dropped
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
    pub fn write_u64(&mut self, mut n: u64) {
        let mut buf = [0u8; 20];
        let mut i = buf.len();
        while n > u64::from(u32::MAX) {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
        let mut n = n as u32;
        loop {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        for b in &buf[i..] {
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
        const HEX: &[u8; 16] = b"0123456789abcdef";
        self.write("0x");
        self.write_byte(HEX[usize::from(b >> 4)]);
        self.write_byte(HEX[usize::from(b & 15)]);
    }

    /// Die Metazeile eines Ticks (grammar/trace.md, `time`).
    pub fn write_time(&mut self, t: &Tick) {
        self.write("t=");
        self.write_u64(t.k);
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
impl<P: Port, const N: usize> core::fmt::Write for Telemetry<P, N> {
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

    /// Eine Leitung, die je Paket `cap` Bytes nimmt.
    struct Line {
        cap: usize,
        room: usize,
        sent: Vec<u8>,
        packets: usize,
    }

    impl Port for Line {
        fn try_write(&mut self, b: u8) -> bool {
            if self.room == 0 {
                return false;
            }
            self.room -= 1;
            self.sent.push(b);
            true
        }

        fn flush(&mut self) {
            self.packets += 1;
            self.room = self.cap;
        }
    }

    fn line(cap: usize) -> Line {
        Line { cap, room: cap, sent: Vec::new(), packets: 0 }
    }

    fn text<const N: usize>(t: &Telemetry<Line, N>) -> String {
        String::from_utf8(t.port.sent.clone()).unwrap()
    }

    #[test]
    fn numbers_print_without_core_fmt() {
        let mut t = Telemetry::<_, 64>::new(line(64));
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
        let mut t = Telemetry::<_, 8>::new(line(0));
        for b in b"0123456789" {
            t.write_byte(*b);
        }
        assert_eq!((t.pending(), t.dropped()), (8, 2));
        assert!(!t.drain(3));
    }

    #[test]
    fn flush_sends_one_packet_in_order_and_wraps() {
        let mut t = Telemetry::<_, 16>::new(line(4));
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

    #[test]
    fn the_time_line_follows_the_trace_grammar() {
        let mut t = Telemetry::<_, 128>::new(line(128));
        t.write_time(&Tick { k: 7, now: 0, took: 22_000, drift: -125, overrun: false, slept: 3 });
        t.flush();
        assert_eq!(text(&t), "t=7 time took=22000 drift=-125 slept=3\r\n");
    }
}
