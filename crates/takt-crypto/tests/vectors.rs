//! Die Vektoren aus `grammar/takt-native.md` (Bloecke `takt-crypto`):
//! ECDSA P-256 nach RFC 6979, RSA-3072 nach RSASSA-PSS und AES-GCM, je mit
//! den Faellen, die `0` oder `none` ergeben muessen. Jede Funktion laeuft
//! mit ihrem Feature; ohne es bleibt nur die Auskunft `Unavailable`.
//!
//! Dazu Fuzzing mit festem Startwert (13.8: Panic-Freiheit): Eingaben der
//! richtigen Laengen, damit die Rechnung bis ins Innere laeuft, und solche
//! der falschen.

struct Vector {
    line: usize,
    fun: String,
    args: Vec<Vec<u8>>,
    want: String,
}

fn hex(line: usize, text: &str) -> Vec<u8> {
    if text == "-" {
        return Vec::new();
    }
    text.as_bytes()
        .chunks(2)
        .map(|p| {
            u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16)
                .unwrap_or_else(|_| panic!("Zeile {line}: `{text}` ist kein Hex"))
        })
        .collect()
}

fn load() -> Vec<Vector> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../grammar/takt-native.md");
    let text = std::fs::read_to_string(path).expect("grammar/takt-native.md lesbar");
    let mut out = Vec::new();
    let mut inside = false;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with("```") {
            inside = line == "```takt-crypto";
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (head, want) = line.split_once(':').unwrap_or_else(|| panic!("Zeile {}: kein `:`", i + 1));
        let mut w = head.split_whitespace();
        let fun = w.next().unwrap_or_else(|| panic!("Zeile {}: keine Funktion", i + 1)).to_string();
        let args = w.map(|a| hex(i + 1, a)).collect();
        out.push(Vector { line: i + 1, fun, args, want: want.trim().to_string() });
    }
    out
}

/// Das Ergebnis einer Zeile in der Schreibweise der Datei; `None`, wenn
/// die Funktion ohne ihr Feature gebaut ist.
fn evaluate(v: &Vector) -> Option<String> {
    let bit = |ok: bool| if ok { "1" } else { "0" }.to_string();
    match (v.fun.as_str(), v.args.as_slice()) {
        ("ecdsa_p256_verify", [key, digest, sig]) => {
            let (Ok(key), Ok(digest), Ok(sig)) = (
                <&[u8; 64]>::try_from(key.as_slice()),
                <&[u8; 32]>::try_from(digest.as_slice()),
                <&[u8; 64]>::try_from(sig.as_slice()),
            ) else {
                return Some(bit(false));
            };
            takt_crypto::ecdsa_p256_verify(key, digest, sig).ok().map(bit)
        }
        ("rsa3072_verify", [key, digest, sig]) => takt_crypto::rsa3072_verify(key, digest, sig).ok().map(bit),
        ("aes_gcm_decrypt", [key, nonce, aad, data, tag]) => {
            let mut out = vec![0u8; data.len()];
            let opened = takt_crypto::aes_gcm_decrypt(key, nonce, aad, data, tag, &mut out).ok()?;
            Some(match opened {
                None => "none".to_string(),
                Some(0) => "-".to_string(),
                Some(n) => out[..n].iter().map(|b| format!("{b:02x}")).collect(),
            })
        }
        (fun, args) => panic!("Zeile {}: `{fun}` mit {} Eingaben", v.line, args.len()),
    }
}

#[test]
fn every_vector_holds() {
    let vectors = load();
    for fun in ["ecdsa_p256_verify", "rsa3072_verify", "aes_gcm_decrypt"] {
        assert!(vectors.iter().filter(|v| v.fun == fun).count() >= 6, "die Spezifikation traegt Vektoren fuer `{fun}`");
    }
    let mut wrong = Vec::new();
    for v in &vectors {
        // Der Test laeuft nur mit allen Features (`required-features`);
        // eine Funktion, die dann `Unavailable` sagt, ist ein Fehler.
        let got = evaluate(v).unwrap_or_else(|| panic!("Zeile {}: `{}` ohne Ergebnis", v.line, v.fun));
        assert!(takt_crypto::manifest(&v.fun).is_some(), "Zeile {}: Feature ohne Manifest", v.line);
        if got != v.want {
            wrong.push(format!("Zeile {}: `{}` erwartet {}, erhalten {got}", v.line, v.fun, v.want));
        }
    }
    assert!(wrong.is_empty(), "Vektoren weichen ab:\n{}", wrong.join("\n"));
}

struct Rng(u64);

impl Rng {
    fn byte(&mut self) -> u8 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 as u8
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }
}

#[test]
fn garbage_never_panics() {
    let mut rng = Rng(0x2026_0916);
    for round in 0..200 {
        let key: [u8; 64] = std::array::from_fn(|_| rng.byte());
        let digest: [u8; 32] = std::array::from_fn(|_| rng.byte());
        let sig: [u8; 64] = std::array::from_fn(|_| rng.byte());
        assert_ne!(takt_crypto::ecdsa_p256_verify(&key, &digest, &sig), Ok(true));
        // RSA: ein Modulus mit gesetztem oberstem Bit, ungerade, und eine
        // Signatur darunter — so laeuft die Potenz und die PSS-Pruefung.
        // Jede fuenfte Runde: Die Potenz kostet im Debug-Bau 150 ms.
        if round % 5 == 0 {
            let mut n = rng.bytes(384);
            n[0] |= 0x80;
            n[383] |= 1;
            let mut s = rng.bytes(384);
            s[0] &= 0x7f;
            assert_ne!(takt_crypto::rsa3072_verify(&n, &digest, &s), Ok(true));
        }
        let wrong = rng.bytes(round % 400);
        let _ = takt_crypto::rsa3072_verify(&wrong, &digest, &wrong);
        // AES-GCM: gueltige Laengen, zufaelliger Tag; dazu falsche Laengen
        // und ein zu kleiner Ausgabepuffer.
        let key = rng.bytes([16, 32, 24, 0][round % 4]);
        let nonce = rng.bytes(if round % 7 == 0 { 8 } else { 12 });
        let aad = rng.bytes(round % 40);
        let data = rng.bytes(round % 300);
        let tag = rng.bytes(16);
        let mut out = vec![0u8; data.len().saturating_sub(round % 3)];
        assert_ne!(
            takt_crypto::aes_gcm_decrypt(&key, &nonce, &aad, &data, &tag, &mut out).map(|r| r.is_some()),
            Ok(true)
        );
    }
}

/// Die erste Zeile einer Funktion aus der Spezifikation, die `want` liefert.
fn first(fun: &str, want: &str) -> Vector {
    load().into_iter().find(|v| v.fun == fun && v.want == want).unwrap_or_else(|| panic!("kein `{fun}` mit {want}"))
}

/// INT-032: Ein `none` hinterlaesst in `out` nichts von der Nutzlast —
/// auch nicht, wenn der Schluessel weder 16 noch 32 Byte hat.
#[test]
fn a_refused_decryption_leaves_no_ciphertext_behind() {
    let data = [0x5Au8; 24];
    for key_len in [0usize, 15, 16, 24, 31, 32, 33] {
        let key = vec![7u8; key_len];
        let mut out = [0u8; 24];
        let r = takt_crypto::aes_gcm_decrypt(&key, &[1; 12], b"aad", &data, &[2; 16], &mut out);
        assert_eq!(r, Ok(None), "Schluessel {key_len} Byte: ein falscher Tag oeffnet nichts");
        assert_eq!(out, [0u8; 24], "Schluessel {key_len} Byte: `out` traegt das Chiffrat");
    }
}

/// INT-032: Die Raender von RSASSA-PSS, die ohne privaten Schluessel zu
/// bilden sind: ein Modulus von 3071 Bit, ein gerader, eine Signatur
/// gleich dem Modulus und eine knapp darunter.
#[test]
fn rsa_refuses_the_edges_of_its_key_and_signature() {
    let v = first("rsa3072_verify", "1");
    let (key, digest, sig) = (&v.args[0], &v.args[1], &v.args[2]);
    assert_eq!(takt_crypto::rsa3072_verify(key, digest, sig), Ok(true), "der Vektor selbst gilt");
    let mut short = key.clone();
    short[0] &= 0x7f;
    assert_eq!(takt_crypto::rsa3072_verify(&short, digest, sig), Ok(false), "3071 Bit");
    let mut even = key.clone();
    even[383] &= 0xfe;
    assert_eq!(takt_crypto::rsa3072_verify(&even, digest, sig), Ok(false), "gerader Modulus");
    assert_eq!(takt_crypto::rsa3072_verify(key, digest, key), Ok(false), "sig = n");
    let mut below = key.clone();
    below[383] -= 1;
    assert_eq!(takt_crypto::rsa3072_verify(key, digest, &below), Ok(false), "sig = n - 1");
    let mut other = digest.clone();
    other[0] ^= 1;
    assert_eq!(takt_crypto::rsa3072_verify(key, &other, sig), Ok(false), "anderer Digest");
}

/// INT-032: `r` oder `s` gleich null oder gleich der Gruppenordnung ist
/// keine Signatur (FIPS 186-5, 6.4.2: 1 <= r, s <= n - 1).
#[test]
fn ecdsa_refuses_r_or_s_outside_the_group_order() {
    const N: [u8; 32] = [
        0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xbc, 0xe6,
        0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63, 0x25, 0x51,
    ];
    let v = first("ecdsa_p256_verify", "1");
    let key: [u8; 64] = v.args[0].as_slice().try_into().expect("Schluessel");
    let digest: [u8; 32] = v.args[1].as_slice().try_into().expect("Digest");
    let sig: [u8; 64] = v.args[2].as_slice().try_into().expect("Signatur");
    assert_eq!(takt_crypto::ecdsa_p256_verify(&key, &digest, &sig), Ok(true), "der Vektor selbst gilt");
    for (half, value) in [(0usize, N), (1, N), (0, [0u8; 32]), (1, [0u8; 32])] {
        let mut bad = sig;
        bad[32 * half..32 * half + 32].copy_from_slice(&value);
        assert_eq!(
            takt_crypto::ecdsa_p256_verify(&key, &digest, &bad),
            Ok(false),
            "{} = {value:02x?}",
            ["r", "s"][half]
        );
    }
}
