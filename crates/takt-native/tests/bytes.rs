//! Die kanonische Byteform ohne Allokation: feste Breiten je Bestandteil
//! (little-endian, ohne Padding) und der volle Puffer.

use takt_native::bytes::{Encoder, Overflow};

#[test]
fn every_part_has_its_fixed_width() {
    let mut buf = [0u8; 64];
    let mut e = Encoder::new(&mut buf);
    e.bool(true).expect("Platz");
    e.int(-2, 2).expect("Platz");
    e.int(0x0102_0304, 4).expect("Platz");
    e.f32(0x3f80_0000).expect("Platz");
    e.f64(0x3ff0_0000_0000_0000).expect("Platz");
    e.duration(1).expect("Platz");
    e.len(3).expect("Platz");
    e.discriminant(-1).expect("Platz");
    e.raw(&[0xaa, 0xbb]).expect("Platz");
    let want: Vec<u8> = [
        vec![1],
        vec![0xfe, 0xff],
        vec![4, 3, 2, 1],
        vec![0, 0, 0x80, 0x3f],
        vec![0, 0, 0, 0, 0, 0, 0xf0, 0x3f],
        vec![1, 0, 0, 0, 0, 0, 0, 0],
        vec![3, 0, 0, 0],
        vec![0xff; 8],
        vec![0xaa, 0xbb],
    ]
    .concat();
    assert_eq!(e.written(), &want[..]);
}

#[test]
fn a_full_buffer_refuses_the_next_byte() {
    let mut buf = [0u8; 4];
    let mut e = Encoder::new(&mut buf);
    assert_eq!(e.len(7), Ok(()));
    assert_eq!(e.bool(true), Err(Overflow));
    assert_eq!(e.written(), &[7, 0, 0, 0]);
}
