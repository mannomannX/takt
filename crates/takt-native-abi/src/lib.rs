//! Die kuratierten Natives (4.5) hinter der C-ABI, die der erzeugte Code
//! ruft: `takt_native_<name>` und `takt_native_map_*` (3.9).
//!
//! **Eine Implementierung** (FB-293). 4.5 legt die Natives in die Runtime;
//! die Rechnung steht in `takt-native` und `takt-crypto`, und Interpreter,
//! Linux-Runtime, Wirtsrahmen und Boards rufen dieselbe. Hier steht nur die
//! Grenze: Zeiger und Laengen werden Slices, ein Ergebnis aus Bytes geht in
//! kanonischer Form (5.9) in den Puffer des Aufrufers.
//!
//! **Die Form an der Grenze** ist die des Codegens (`native_call`): ein
//! `bytes<N>` als Zeiger auf die Daten und Laenge, ein Record in
//! kanonischer Byteform als Zeiger und Laenge, ein Skalar als Wert; ein
//! Ergebnis aus Bytes oder ein Record geht nach `out`.
//!
//! **Total.** Jeder Einstieg nimmt jede Eingabe: Eine Laenge unter eins
//! ist leer, ein `Sha256Ctx`, der keiner ist, der leere Zustand.

#![no_std]
#![allow(unsafe_code, reason = "C-ABI der Natives fuer den erzeugten Code; 9.5 fuehrt sie in der TCB")]

use core::slice;

use takt_native::map::ByteMap;
use takt_native::sha256::{CTX_MAX_BYTES, Ctx};
use takt_native::{crc, fft, sha256};

/// Ein `bytes<32>`-Ergebnis in kanonischer Form: Laenge, dann der Digest.
pub const DIGEST_BYTES: usize = 4 + 32;

/// Die Bytes hinter einem Zeiger des erzeugten Codes.
///
/// # Safety
///
/// Ist `n` positiv, zeigt `p` auf `n` lesbare Bytes, die waehrend des
/// Aufrufs niemand schreibt.
unsafe fn input<'a>(p: *const u8, n: i32) -> &'a [u8] {
    match usize::try_from(n) {
        // SAFETY: vom Aufrufer zugesagt.
        Ok(n) if n > 0 && !p.is_null() => unsafe { slice::from_raw_parts(p, n) },
        _ => &[],
    }
}

/// Eine Laenge des erzeugten Codes; negativ ist null.
fn size(n: i32) -> usize {
    usize::try_from(n).unwrap_or(0)
}

/// Schreibt einen Digest als `bytes<32>` in kanonischer Form.
///
/// # Safety
///
/// `out` zeigt auf [`DIGEST_BYTES`] schreibbare Bytes.
unsafe fn put_digest(out: *mut u8, d: &[u8; 32]) {
    if out.is_null() {
        return;
    }
    // SAFETY: vom Aufrufer zugesagt; ein Bytefeld braucht keine Ausrichtung.
    let out = unsafe { &mut *out.cast::<[u8; DIGEST_BYTES]>() };
    out[..4].copy_from_slice(&32u32.to_le_bytes());
    out[4..].copy_from_slice(d);
}

/// Ein `Sha256Ctx` in kanonischer Form; was keiner ist, ist der leere Zustand.
///
/// # Safety
///
/// Wie [`input`].
unsafe fn ctx(p: *const u8, n: i32) -> Ctx {
    // SAFETY: vom Aufrufer zugesagt.
    Ctx::from_bytes(unsafe { input(p, n) }).unwrap_or_default()
}

/// Schreibt einen `Sha256Ctx` in kanonischer Form; sie passt immer in
/// [`CTX_MAX_BYTES`].
///
/// # Safety
///
/// `out` zeigt auf [`CTX_MAX_BYTES`] schreibbare Bytes.
unsafe fn put_ctx(out: *mut u8, ctx: &Ctx) {
    if out.is_null() {
        return;
    }
    // SAFETY: vom Aufrufer zugesagt; ein Bytefeld braucht keine Ausrichtung.
    let out = unsafe { &mut *out.cast::<[u8; CTX_MAX_BYTES]>() };
    let _ = ctx.to_bytes(out);
}

/// `crc32(b)`.
///
/// # Safety
///
/// `b` zeigt auf `n` lesbare Bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_crc32(b: *const u8, n: i32) -> u32 {
    // SAFETY: vom Aufrufer zugesagt.
    crc::crc32(unsafe { input(b, n) })
}

/// `crc32c(b)`.
///
/// # Safety
///
/// `b` zeigt auf `n` lesbare Bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_crc32c(b: *const u8, n: i32) -> u32 {
    // SAFETY: vom Aufrufer zugesagt.
    crc::crc32c(unsafe { input(b, n) })
}

/// `crc16(b)`.
///
/// # Safety
///
/// `b` zeigt auf `n` lesbare Bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_crc16(b: *const u8, n: i32) -> u16 {
    // SAFETY: vom Aufrufer zugesagt.
    crc::crc16(unsafe { input(b, n) })
}

/// `sum8(b)`.
///
/// # Safety
///
/// `b` zeigt auf `n` lesbare Bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_sum8(b: *const u8, n: i32) -> u8 {
    // SAFETY: vom Aufrufer zugesagt.
    crc::sum8(unsafe { input(b, n) })
}

/// `sha256(b)`.
///
/// # Safety
///
/// `b` zeigt auf `n` lesbare Bytes, `out` auf [`DIGEST_BYTES`] schreibbare.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_sha256(b: *const u8, n: i32, out: *mut u8) {
    // SAFETY: vom Aufrufer zugesagt.
    unsafe { put_digest(out, &sha256::sha256(input(b, n))) }
}

/// `hmac_sha256(key, msg)`.
///
/// # Safety
///
/// `key` zeigt auf `kn` lesbare Bytes, `msg` auf `mn`, `out` auf
/// [`DIGEST_BYTES`] schreibbare.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_hmac_sha256(key: *const u8, kn: i32, msg: *const u8, mn: i32, out: *mut u8) {
    // SAFETY: vom Aufrufer zugesagt.
    unsafe { put_digest(out, &sha256::hmac_sha256(input(key, kn), input(msg, mn))) }
}

/// `sha256_init()`.
///
/// # Safety
///
/// `out` zeigt auf [`CTX_MAX_BYTES`] schreibbare Bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_sha256_init(out: *mut u8) {
    // SAFETY: vom Aufrufer zugesagt.
    unsafe { put_ctx(out, &Ctx::new()) }
}

/// `sha256_update(ctx, chunk)`.
///
/// # Safety
///
/// `c` zeigt auf `cn` lesbare Bytes, `d` auf `n`, `out` auf
/// [`CTX_MAX_BYTES`] schreibbare, die keinen der Eingaenge ueberlappen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_sha256_update(c: *const u8, cn: i32, d: *const u8, n: i32, out: *mut u8) {
    // SAFETY: vom Aufrufer zugesagt.
    unsafe {
        let mut state = ctx(c, cn);
        state.update(input(d, n));
        put_ctx(out, &state);
    }
}

/// `sha256_final(ctx)`.
///
/// # Safety
///
/// `c` zeigt auf `cn` lesbare Bytes, `out` auf [`DIGEST_BYTES`] schreibbare.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_sha256_final(c: *const u8, cn: i32, out: *mut u8) {
    // SAFETY: vom Aufrufer zugesagt.
    unsafe { put_digest(out, &ctx(c, cn).finish()) }
}

/// `ecdsa_p256_verify(key, digest, sig)` (4.5, ein Job); falsche Laengen
/// pruefen nicht.
///
/// # Safety
///
/// `key`, `digest` und `sig` zeigen auf `kn`, `dn` und `sn` lesbare Bytes.
#[cfg(feature = "ecdsa")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_ecdsa_p256_verify(
    key: *const u8,
    kn: i32,
    digest: *const u8,
    dn: i32,
    sig: *const u8,
    sn: i32,
) -> bool {
    // SAFETY: vom Aufrufer zugesagt.
    let (key, digest, sig) = unsafe { (input(key, kn), input(digest, dn), input(sig, sn)) };
    match (key.try_into(), digest.try_into(), sig.try_into()) {
        (Ok(key), Ok(digest), Ok(sig)) => takt_crypto::ecdsa_p256_verify(key, digest, sig).unwrap_or(false),
        _ => false,
    }
}

/// `fft256(x)` (4.5): 256 Werte in kanonischer Form, 1024 Byte fuer `f32`,
/// 2048 fuer `f64`; das Ergebnis in derselben Form nach `out`. Eine andere
/// Laenge schreibt nichts.
///
/// # Safety
///
/// `x` zeigt auf `n` lesbare Bytes, `out` auf `n` schreibbare.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_fft256(x: *const u8, n: i32, out: *mut u8) {
    let mut buf = [0u8; fft::BYTES_F64];
    // SAFETY: vom Aufrufer zugesagt.
    let Some(len) = fft::fft256(unsafe { input(x, n) }, &mut buf) else { return };
    if !out.is_null() {
        // SAFETY: `out` fasst `n` Byte, und `len` ist `n`.
        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), out, len) };
    }
}

/// `rsa3072_verify(key, digest, sig)` (4.5, ein Job): RSASSA-PSS mit
/// SHA-256; falsche Laengen pruefen nicht.
///
/// # Safety
///
/// `key`, `digest` und `sig` zeigen auf `kn`, `dn` und `sn` lesbare Bytes.
#[cfg(feature = "rsa")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_rsa3072_verify(
    key: *const u8,
    kn: i32,
    digest: *const u8,
    dn: i32,
    sig: *const u8,
    sn: i32,
) -> bool {
    // SAFETY: vom Aufrufer zugesagt.
    let (key, digest, sig) = unsafe { (input(key, kn), input(digest, dn), input(sig, sn)) };
    takt_crypto::rsa3072_verify(key, digest, sig).unwrap_or(false)
}

/// `aes_gcm_decrypt(key, nonce, aad, data, tag)` (4.5, ein Job): der
/// Klartext nach `out`, seine Laenge; `-1`, wenn der Tag nicht stimmt oder
/// eine Laenge nicht passt — der Job endet dann mit `Err(FAILED)`.
///
/// # Safety
///
/// Jeder Zeiger zeigt auf die Zahl lesbarer Bytes, die neben ihm steht,
/// `out` auf `dn` schreibbare.
#[cfg(feature = "aes-gcm")]
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn takt_native_aes_gcm_decrypt(
    key: *const u8,
    kn: i32,
    nonce: *const u8,
    nn: i32,
    aad: *const u8,
    an: i32,
    data: *const u8,
    dn: i32,
    tag: *const u8,
    tn: i32,
    out: *mut u8,
) -> i32 {
    if out.is_null() && dn > 0 {
        return -1;
    }
    // SAFETY: vom Aufrufer zugesagt.
    let (key, nonce, aad, data, tag) =
        unsafe { (input(key, kn), input(nonce, nn), input(aad, an), input(data, dn), input(tag, tn)) };
    let out: &mut [u8] = if data.is_empty() {
        &mut []
    } else {
        // SAFETY: `out` fasst `dn` Byte, vom Aufrufer zugesagt.
        unsafe { slice::from_raw_parts_mut(out, data.len()) }
    };
    match takt_crypto::aes_gcm_decrypt(key, nonce, aad, data, tag, out) {
        Ok(Some(len)) => i32::try_from(len).unwrap_or(-1),
        _ => -1,
    }
}

/// Die Slots einer Map (3.9): `cap` Slots zu `1 + klen + vlen` Byte.
///
/// # Safety
///
/// `s` zeigt auf so viele Bytes, lesbar und schreibbar, die waehrend des
/// Aufrufs sonst niemand anfasst.
unsafe fn slots<'a>(s: *mut u8, cap: usize, klen: usize, vlen: usize) -> Option<ByteMap<'a>> {
    let bytes = cap.checked_mul(klen.checked_add(vlen)?.checked_add(1)?)?;
    // SAFETY: vom Aufrufer zugesagt.
    (!s.is_null()).then(|| ByteMap::new(unsafe { slice::from_raw_parts_mut(s, bytes) }, cap, klen, vlen))
}

/// `m.len`.
///
/// # Safety
///
/// Wie [`slots`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_map_len(s: *mut u8, cap: i32, klen: i32, vlen: i32) -> i32 {
    // SAFETY: vom Aufrufer zugesagt.
    let map = unsafe { slots(s, size(cap), size(klen), size(vlen)) };
    map.map_or(0, |m| i32::try_from(m.len()).unwrap_or(i32::MAX))
}

/// `m.insert(k, v)`; `false`, wenn die Map voll ist.
///
/// # Safety
///
/// Wie [`slots`]; `key` zeigt auf `klen` lesbare Bytes, `val` auf `vlen`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_map_insert(
    s: *mut u8,
    cap: i32,
    klen: i32,
    vlen: i32,
    key: *const u8,
    val: *const u8,
) -> bool {
    // SAFETY: vom Aufrufer zugesagt.
    let (map, key, val) = unsafe { (slots(s, size(cap), size(klen), size(vlen)), input(key, klen), input(val, vlen)) };
    map.is_some_and(|mut m| m.insert(key, val))
}

/// `m.get(k)`: der Wert nach `out`, wenn der Schluessel in der Map steht.
///
/// # Safety
///
/// Wie [`slots`]; `key` zeigt auf `klen` lesbare Bytes, `out` auf `vlen`
/// schreibbare ausserhalb der Slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_map_get(
    s: *mut u8,
    cap: i32,
    klen: i32,
    vlen: i32,
    key: *const u8,
    out: *mut u8,
) -> bool {
    // SAFETY: vom Aufrufer zugesagt.
    let (map, key) = unsafe { (slots(s, size(cap), size(klen), size(vlen)), input(key, klen)) };
    let Some(value) = map.as_ref().and_then(|m| m.get(key)) else { return false };
    if !out.is_null() {
        // SAFETY: vom Aufrufer zugesagt; `value` ist `vlen` Byte lang.
        unsafe { slice::from_raw_parts_mut(out, value.len()) }.copy_from_slice(value);
    }
    true
}

/// `m.remove(k)`.
///
/// # Safety
///
/// Wie [`slots`]; `key` zeigt auf `klen` lesbare Bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_native_map_remove(s: *mut u8, cap: i32, klen: i32, vlen: i32, key: *const u8) -> bool {
    // SAFETY: vom Aufrufer zugesagt.
    let (map, key) = unsafe { (slots(s, size(cap), size(klen), size(vlen)), input(key, klen)) };
    map.is_some_and(|mut m| m.remove(key))
}

/// Die Einstiege unter Messung (13.8): Das Messprogramm `natives` der
/// Bring-ups ruft je Vektor genau die Einstiege, die der erzeugte Code
/// ruft, jeden ueber einen Zeiger und einzeln gemessen.
pub mod measure {
    use core::hint::black_box;

    use takt_native::Native;
    use takt_native::sha256::CTX_MAX_BYTES;

    use super::DIGEST_BYTES;

    /// Was eine Vektorzeile ergab.
    #[derive(Clone, Copy, Debug)]
    pub struct Run {
        bytes: [u8; 32],
        len: usize,
        /// Der Stack-Bedarf des Einstiegs der Zeile; bei mehreren Aufrufen
        /// der groesste.
        pub stack: u32,
        others: [(&'static str, u32); 2],
        count: usize,
    }

    impl Run {
        /// Das Ergebnis: eine Pruefsumme als acht Byte, hoechstwertiges
        /// zuerst, ein Digest Byte fuer Byte.
        pub fn result(&self) -> &[u8] {
            &self.bytes[..self.len]
        }

        /// Die weiteren Einstiege der Zeile mit ihrem Stack-Bedarf: Anfang
        /// und Ende einer Kette `sha256_init`, `sha256_update`, `sha256_final`.
        pub fn others(&self) -> &[(&'static str, u32)] {
            &self.others[..self.count]
        }

        fn scalar(v: u64, stack: u32) -> Run {
            let mut bytes = [0u8; 32];
            bytes[..8].copy_from_slice(&v.to_be_bytes());
            Run { bytes, len: 8, stack, others: [("", 0); 2], count: 0 }
        }

        fn digest(out: &[u8; DIGEST_BYTES], stack: u32) -> Run {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&out[4..]);
            Run { bytes, len: 32, stack, others: [("", 0); 2], count: 0 }
        }

        /// Ein Ergebnis aus Bytes ohne eigene Form (die Werte von `fft256`,
        /// ein Klartext): sein SHA-256, wie die Vektoren es schreiben.
        fn block(out: &[u8], stack: u32) -> Run {
            Run { bytes: takt_native::sha256::sha256(out), len: 32, stack, others: [("", 0); 2], count: 0 }
        }

        /// Kein Ergebnis: Das Messprogramm schreibt `-`.
        #[cfg(feature = "aes-gcm")]
        fn none(stack: u32) -> Run {
            Run { bytes: [0; 32], len: 0, stack, others: [("", 0); 2], count: 0 }
        }
    }

    /// Eine Laenge an der Grenze.
    fn len(b: &[u8]) -> i32 {
        i32::try_from(b.len()).unwrap_or(i32::MAX)
    }

    /// Ein Einstieg ueber einen Byteblock mit Skalar als Ergebnis.
    fn scalar<R: Into<u64> + Default>(
        entry: unsafe extern "C" fn(*const u8, i32) -> R,
        b: &[u8],
        measure: &mut dyn FnMut(&mut dyn FnMut()) -> u32,
    ) -> Run {
        let entry = black_box(entry);
        let mut r = R::default();
        // SAFETY: `b` ist ein Slice seiner Laenge.
        let stack = measure(&mut || r = unsafe { entry(b.as_ptr(), len(b)) });
        Run::scalar(r.into(), stack)
    }

    /// Rechnet eine Vektorzeile ueber die Einstiege; `measure` ruft die
    /// Funktion, die es bekommt, und liefert ihren Stack-Bedarf. `None`,
    /// wenn die Eingaben nicht zur Funktion passen.
    pub fn vector(f: Native, inputs: &[&[u8]], measure: &mut dyn FnMut(&mut dyn FnMut()) -> u32) -> Option<Run> {
        let one = || inputs.first().copied().filter(|_| inputs.len() == 1);
        Some(match f {
            Native::Crc32 => scalar(super::takt_native_crc32, one()?, measure),
            Native::Crc32c => scalar(super::takt_native_crc32c, one()?, measure),
            Native::Crc16 => scalar(super::takt_native_crc16, one()?, measure),
            Native::Sum8 => scalar(super::takt_native_sum8, one()?, measure),
            Native::Sha256 => {
                let (b, entry) = (one()?, black_box(super::takt_native_sha256 as unsafe extern "C" fn(_, _, _)));
                let mut out = [0u8; DIGEST_BYTES];
                // SAFETY: `b` ist ein Slice seiner Laenge, `out` fasst den Digest.
                let stack = measure(&mut || unsafe { entry(b.as_ptr(), len(b), out.as_mut_ptr()) });
                Run::digest(&out, stack)
            }
            Native::HmacSha256 => {
                let [key, msg] = inputs else { return None };
                let entry = black_box(super::takt_native_hmac_sha256 as unsafe extern "C" fn(_, _, _, _, _));
                let mut out = [0u8; DIGEST_BYTES];
                // SAFETY: wie oben, fuer beide Eingaben.
                let stack =
                    measure(&mut || unsafe { entry(key.as_ptr(), len(key), msg.as_ptr(), len(msg), out.as_mut_ptr()) });
                Run::digest(&out, stack)
            }
            Native::Sha256Init | Native::Sha256Update | Native::Sha256Final => chain(inputs, measure),
            Native::Fft256 => {
                let (x, entry) = (one()?, black_box(super::takt_native_fft256 as unsafe extern "C" fn(_, _, _)));
                let mut out = [0u8; takt_native::fft::BYTES_F64];
                // SAFETY: `x` ist ein Slice seiner Laenge, `out` fasst die
                // groesste kanonische Form.
                let stack = measure(&mut || unsafe { entry(x.as_ptr(), len(x), out.as_mut_ptr()) });
                Run::block(&out[..x.len().min(out.len())], stack)
            }
            #[cfg(feature = "ecdsa")]
            Native::EcdsaP256Verify => {
                let [key, digest, sig] = inputs else { return None };
                let entry = black_box(super::takt_native_ecdsa_p256_verify as unsafe extern "C" fn(_, _, _, _, _, _) -> _);
                let mut ok = false;
                // SAFETY: drei Slices ihrer Laenge.
                let stack = measure(&mut || {
                    ok = unsafe { entry(key.as_ptr(), len(key), digest.as_ptr(), len(digest), sig.as_ptr(), len(sig)) }
                });
                Run::scalar(u64::from(ok), stack)
            }
            #[cfg(feature = "rsa")]
            Native::Rsa3072Verify => {
                let [key, digest, sig] = inputs else { return None };
                let entry = black_box(super::takt_native_rsa3072_verify as unsafe extern "C" fn(_, _, _, _, _, _) -> _);
                let mut ok = false;
                // SAFETY: drei Slices ihrer Laenge.
                let stack = measure(&mut || {
                    ok = unsafe { entry(key.as_ptr(), len(key), digest.as_ptr(), len(digest), sig.as_ptr(), len(sig)) }
                });
                Run::scalar(u64::from(ok), stack)
            }
            #[cfg(feature = "aes-gcm")]
            Native::AesGcmDecrypt => {
                let [key, nonce, aad, data, tag] = inputs else { return None };
                let entry = black_box(
                    super::takt_native_aes_gcm_decrypt as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _, _) -> _,
                );
                let mut out = [0u8; PLAIN_MAX];
                if data.len() > PLAIN_MAX {
                    return None;
                }
                let mut got = -1;
                // SAFETY: fuenf Slices ihrer Laenge, `out` fasst `data`.
                let stack = measure(&mut || {
                    got = unsafe {
                        entry(
                            key.as_ptr(),
                            len(key),
                            nonce.as_ptr(),
                            len(nonce),
                            aad.as_ptr(),
                            len(aad),
                            data.as_ptr(),
                            len(data),
                            tag.as_ptr(),
                            len(tag),
                            out.as_mut_ptr(),
                        )
                    }
                });
                match usize::try_from(got) {
                    Ok(n) => Run::block(&out[..n], stack),
                    Err(_) => Run::none(stack),
                }
            }
            // Ohne ihr Feature kommt eine Funktion aus `takt-crypto` nicht
            // vor: Das Messprogramm hat dann keine Zeile fuer sie.
            #[allow(unreachable_patterns)]
            _ => return None,
        })
    }

    /// Der laengste Klartext, den die Messung von `aes_gcm_decrypt` fasst.
    #[cfg(feature = "aes-gcm")]
    const PLAIN_MAX: usize = 512;

    /// `sha256_init`, ein `sha256_update` je Chunk und `sha256_final`, jeder
    /// Aufruf einzeln gemessen.
    fn chain(chunks: &[&[u8]], measure: &mut dyn FnMut(&mut dyn FnMut()) -> u32) -> Run {
        let init = black_box(super::takt_native_sha256_init as unsafe extern "C" fn(_));
        let update = black_box(super::takt_native_sha256_update as unsafe extern "C" fn(_, _, _, _, _));
        let finish = black_box(super::takt_native_sha256_final as unsafe extern "C" fn(_, _, _));
        let size = |c: &[u8; CTX_MAX_BYTES]| {
            let filled = u32::from_le_bytes([c[32], c[33], c[34], c[35]]) as usize;
            i32::try_from(CTX_MAX_BYTES.min(44 + filled)).unwrap_or(0)
        };
        let mut ctx = [0u8; CTX_MAX_BYTES];
        // SAFETY: `ctx` fasst die kanonische Form.
        let first = measure(&mut || unsafe { init(ctx.as_mut_ptr()) });
        let mut stack = 0;
        for chunk in chunks {
            let mut next = [0u8; CTX_MAX_BYTES];
            let n = size(&ctx);
            // SAFETY: `ctx` traegt `n` Byte, `chunk` ist ein Slice seiner
            // Laenge, `next` fasst die kanonische Form.
            let used =
                measure(&mut || unsafe { update(ctx.as_ptr(), n, chunk.as_ptr(), len(chunk), next.as_mut_ptr()) });
            stack = stack.max(used);
            ctx = next;
        }
        let mut out = [0u8; DIGEST_BYTES];
        let n = size(&ctx);
        // SAFETY: `ctx` traegt `n` Byte, `out` fasst den Digest.
        let last = measure(&mut || unsafe { finish(ctx.as_ptr(), n, out.as_mut_ptr()) });
        Run { others: [("sha256_init", first), ("sha256_final", last)], count: 2, ..Run::digest(&out, stack) }
    }
}

/// Die statische Bibliothek des Wirts bricht bei einer Panik ab: Eine
/// Panik hier ist ein Fehler in der TCB (9.5), und der Lauf endet sichtbar.
#[cfg(feature = "host")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe extern "C" {
        fn abort() -> !;
    }
    // SAFETY: `abort` der C-Laufzeit nimmt nichts und kehrt nicht zurueck.
    unsafe { abort() }
}
