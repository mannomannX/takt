//! Eine serielle Schnittstelle aus `embedded-io` als Quelle eines
//! Eingabestroms (2.7 in plan/m11.md, 12.6; Merkmal `io`): Mit einem
//! Rahmer ([`crate::frame::Framed`]) wird daraus ein Strom, ohne eigenen
//! Treibercode.

use embedded_io::{Read, ReadReady};

use crate::frame::{Pull, Source};

/// Eine Schnittstelle, die `embedded-io` liest: Der Schritt nimmt nur, was
/// bereitliegt (`ReadReady`), und wartet nie. Ein Fehler der Schnittstelle
/// (Ueberlauf, Rahmenfehler) ist eine Luecke im Strom (8.6, „Rahmung“).
#[derive(Debug, Default)]
pub struct Serial<R>(pub R);

impl<R: Read + ReadReady> Source for Serial<R> {
    fn pull(&mut self) -> Pull {
        match self.0.read_ready() {
            Ok(true) => {}
            Ok(false) => return Pull::Empty,
            Err(_) => return Pull::Lost,
        }
        let mut byte = [0u8; 1];
        match (self.0.read(&mut byte), byte.first()) {
            (Ok(1), Some(b)) => Pull::Byte(*b),
            (Ok(_), _) => Pull::Empty,
            (Err(_), _) => Pull::Lost,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Serial;
    use crate::StreamInput;
    use crate::frame::{Framed, Lines};
    use embedded_io::{ErrorKind, ErrorType, Read, ReadReady};
    use std::vec::Vec;

    /// Eine nachgebaute Schnittstelle: Bytes, von denen je Tick ein Teil
    /// bereitliegt, und das Lesen, an dem sie einen Ueberlauf meldet.
    struct Fake {
        bytes: Vec<u8>,
        ready: usize,
        overrun_at: Option<usize>,
        reads: usize,
    }

    impl Fake {
        fn new(bytes: &[u8], ready: usize) -> Fake {
            Fake { bytes: bytes.to_vec(), ready, overrun_at: None, reads: 0 }
        }
    }

    impl ErrorType for Fake {
        type Error = ErrorKind;
    }

    impl ReadReady for Fake {
        fn read_ready(&mut self) -> Result<bool, ErrorKind> {
            Ok(self.ready > 0 && !self.bytes.is_empty())
        }
    }

    impl Read for Fake {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, ErrorKind> {
            assert!(self.ready > 0, "gelesen, obwohl nichts bereitlag: das wuerde warten");
            self.reads += 1;
            if self.overrun_at == Some(self.reads) {
                return Err(ErrorKind::Other);
            }
            let n = buf.len().min(self.ready).min(self.bytes.len());
            buf[..n].copy_from_slice(&self.bytes[..n]);
            self.bytes.drain(..n);
            self.ready -= n;
            Ok(n)
        }
    }

    /// Je `poll` eine Zeile mit ihrer Folgenummer; eine halbe wartet auf
    /// ihren Rest im naechsten Tick, und gelesen wird nie mehr, als
    /// bereitliegt.
    #[test]
    fn a_serial_line_stream_delivers_whole_lines_without_waiting() {
        let mut rx = Framed::new(Serial(Fake::new(b"hi\nthere\n", 6)), Lines::<16>::new());
        let mut buf = [0u8; 17];
        let first = rx.poll(&mut buf, 10).expect("erste Zeile");
        assert_eq!((&buf[..first.len], first.t, first.seq), (&b"hi"[..], 10, Some(0)));
        assert!(rx.poll(&mut buf, 10).is_none(), "`the` wartet auf den Rest");
        rx.source.0.ready = 10;
        let second = rx.poll(&mut buf, 20).expect("zweite Zeile");
        assert_eq!((&buf[..second.len], second.seq), (&b"there"[..], Some(1)));
    }

    /// Ein Ueberlauf der Schnittstelle trifft die Zeile, in der er liegt: Sie
    /// fehlt, ihre Folgenummer auch, und der Rand sieht die Luecke (8.6,
    /// 12.6 Zeile 2). Die naechste Zeile kommt heil.
    #[test]
    fn an_overrun_costs_the_line_it_hits_and_its_number() {
        let mut uart = Fake::new(b"one\ntwo\nthree\n", 100);
        uart.overrun_at = Some(6);
        let mut rx = Framed::new(Serial(uart), Lines::<16>::new());
        let mut buf = [0u8; 17];
        let first = rx.poll(&mut buf, 0).expect("erste Zeile");
        assert_eq!((&buf[..first.len], first.seq), (&b"one"[..], Some(0)));
        let next = rx.poll(&mut buf, 0).expect("naechste Zeile");
        assert_eq!((&buf[..next.len], next.seq), (&b"three"[..], Some(2)), "`two` traf der Ueberlauf");
    }
}
