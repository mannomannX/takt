//! Die Chunk-Natives (4.5): Ein Digest haengt nicht davon ab, wo die
//! Chunks geschnitten sind, und `Sha256Ctx` ueberlebt die kanonische
//! Byteform (5.9) unveraendert.

use takt_native::sha256::{CTX_MAX_BYTES, Ctx, sha256};

fn digest_in_chunks(data: &[u8], cut: usize) -> [u8; 32] {
    let mut ctx = Ctx::new();
    for chunk in data.chunks(cut.max(1)) {
        ctx.update(chunk);
    }
    ctx.finish()
}

#[test]
fn the_cut_does_not_change_the_digest() {
    let data: Vec<u8> = (0..300u32).map(|i| (i * 7 % 251) as u8).collect();
    let whole = sha256(&data);
    for cut in [1, 3, 55, 56, 63, 64, 65, 128, 300] {
        assert_eq!(digest_in_chunks(&data, cut), whole, "Schnitt bei {cut}");
    }
}

#[test]
fn a_context_survives_the_byte_form() {
    let mut ctx = Ctx::new();
    ctx.update(b"abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789ABC");
    let mut buf = [0u8; CTX_MAX_BYTES];
    let n = ctx.to_bytes(&mut buf).expect("Platz");
    // 8 * 4 fuer `h`, 4 + 11 fuer die 75 mod 64 gefuellten Bytes, 8 fuer `total`.
    assert_eq!(n, 32 + 4 + 11 + 8);
    let back = Ctx::from_bytes(&buf[..n]).expect("gueltige Form");
    assert_eq!(back, ctx);
    let mut tail = [0u8; 2];
    tail.copy_from_slice(b"xy");
    let mut a = ctx.clone();
    a.update(&tail);
    let mut b = back;
    b.update(&tail);
    assert_eq!(a.finish(), b.finish());
}

#[test]
fn a_foreign_byte_form_is_refused() {
    let mut buf = [0u8; CTX_MAX_BYTES];
    let n = Ctx::new().to_bytes(&mut buf).expect("Platz");
    assert!(Ctx::from_bytes(&buf[..n]).is_some());
    assert!(Ctx::from_bytes(&buf[..n - 1]).is_none(), "zu kurz");
    buf[32] = 64;
    assert!(Ctx::from_bytes(&buf[..n + 64]).is_none(), "ein voller Block steht nie im Zustand");
    let mut small = [0u8; 8];
    assert!(Ctx::new().to_bytes(&mut small).is_err(), "der Puffer ist zu klein");
}
