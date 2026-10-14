//! Eine serielle Schnittstelle aus `embedded-io` als Quelle eines
//! Eingabestroms (2.7 in plan/m11.md, 12.6; Merkmal `io`): Mit einem
//! Rahmer ([`crate::frame::Framed`]) wird daraus ein Strom, ohne eigenen
//! Treibercode.

use embedded_io::{Read, ReadReady};

use crate::frame::Source;

/// Eine Schnittstelle, die `embedded-io` liest: Der Schritt nimmt nur, was
/// bereitliegt (`ReadReady`), und wartet nie.
#[derive(Debug, Default)]
pub struct Serial<R>(pub R);

impl<R: Read + ReadReady> Source for Serial<R> {
    fn take(&mut self) -> Option<u8> {
        if !self.0.read_ready().ok()? {
            return None;
        }
        let mut byte = [0u8; 1];
        match self.0.read(&mut byte) {
            Ok(1) => byte.first().copied(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Serial;
    use crate::StreamInput;
    use crate::frame::{Framed, Lines};
    use core::convert::Infallible;
    use embedded_io::{ErrorType, Read, ReadReady};
    use std::vec::Vec;

    /// Eine nachgebaute Schnittstelle: Bytes, von denen je Tick ein Teil
    /// bereitliegt.
    struct Fake {
        bytes: Vec<u8>,
        ready: usize,
    }

    impl ErrorType for Fake {
        type Error = Infallible;
    }

    impl ReadReady for Fake {
        fn read_ready(&mut self) -> Result<bool, Infallible> {
            Ok(self.ready > 0 && !self.bytes.is_empty())
        }
    }

    impl Read for Fake {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, Infallible> {
            assert!(self.ready > 0, "gelesen, obwohl nichts bereitlag: das wuerde warten");
            let n = buf.len().min(self.ready).min(self.bytes.len());
            buf[..n].copy_from_slice(&self.bytes[..n]);
            self.bytes.drain(..n);
            self.ready -= n;
            Ok(n)
        }
    }

    /// Je `poll` eine Zeile; eine halbe wartet auf ihren Rest im naechsten
    /// Tick, und gelesen wird nie mehr, als bereitliegt.
    #[test]
    fn a_serial_line_stream_delivers_whole_lines_without_waiting() {
        let mut rx =
            Framed { source: Serial(Fake { bytes: b"hi\nthere\n".to_vec(), ready: 6 }), framer: Lines::<16>::new() };
        let mut buf = [0u8; 16];
        let first = rx.poll(&mut buf, 10).expect("erste Zeile");
        assert_eq!((&buf[..first.len], first.t, first.seq), (&b"hi"[..], 10, None));
        assert!(rx.poll(&mut buf, 10).is_none(), "`the` wartet auf den Rest");
        rx.source.0.ready = 10;
        let second = rx.poll(&mut buf, 20).expect("zweite Zeile");
        assert_eq!(&buf[..second.len], b"there");
    }
}
