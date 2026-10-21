//! Zahlen als Ziffern ohne `core::fmt` (12.3, 12.5): fuer die Leitung der
//! Boards und die Haken des Trace.
//!
//! **Warum nicht `core::fmt`.** Unter `xip_flash` liegt alles, was der Tick
//! ruft, im RAM (`takt check-image`); `core::fmt` ist gross und laege mit
//! allem, was es mitbringt, dort. Eine Zahl je Tick braucht nur ihre Ziffern.

/// Die Dezimalziffern von `n`, hinten in `buf`; zurueck kommt der belegte
/// Teil. Die Ziffern einer `u64` passen in 20 Byte.
pub fn decimal(n: u64, buf: &mut [u8; 20]) -> &[u8] {
    let mut used = 0usize;
    let mut slots = buf.iter_mut().rev();
    let mut put = |d: u8| {
        if let Some(slot) = slots.next() {
            *slot = d;
            used = used.saturating_add(1);
        }
    };
    // In u64 nur, bis der Rest in u32 passt: Auf RV32 ist die Division in
    // u64 ein Aufruf, die in u32 ein Befehl.
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
    let from = buf.len().saturating_sub(used);
    buf.get(from..).unwrap_or(&[])
}

/// Die Hexziffer der unteren vier Bit von `n`, klein geschrieben.
pub fn hex(n: u8) -> u8 {
    b"0123456789abcdef".get(usize::from(n & 15)).copied().unwrap_or(b'?')
}

/// Die Dezimalziffer von `d`; der Aufrufer gibt `d < 10`.
fn decimal_digit(d: u64) -> u8 {
    usize::try_from(d).ok().and_then(|i| b"0123456789".get(i)).copied().unwrap_or(b'?')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digits_of_the_edges_of_u64() {
        let mut buf = [0u8; 20];
        assert_eq!(decimal(0, &mut buf), b"0");
        assert_eq!(decimal(u64::from(u32::MAX), &mut buf), b"4294967295");
        assert_eq!(decimal(u64::from(u32::MAX) + 1, &mut buf), b"4294967296");
        assert_eq!(decimal(u64::MAX, &mut buf), b"18446744073709551615");
        assert_eq!((hex(0x0a), hex(0xff)), (b'a', b'f'));
    }
}
