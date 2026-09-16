//! Die ECDSA-Vektoren aus `grammar/takt-native.md` (Block `takt-crypto`):
//! RFC 6979 A.2.5 und ein eigener Schluessel, dazu die Faelle, die
//! `false` ergeben muessen. Sie laufen mit dem Feature `ecdsa`; ohne es
//! bleibt nur die Auskunft `Unavailable`.

struct Vector {
    line: usize,
    key: [u8; 64],
    digest: [u8; 32],
    sig: [u8; 64],
    want: bool,
}

fn hex<const N: usize>(line: usize, text: &str) -> [u8; N] {
    let bytes: Vec<u8> = text
        .as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16).expect("Hex"))
        .collect();
    bytes.try_into().unwrap_or_else(|v: Vec<u8>| panic!("Zeile {line}: {} statt {N} Byte", v.len()))
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
        let (head, want) = line.split_once(':').expect("`:`");
        let w: Vec<&str> = head.split_whitespace().collect();
        assert_eq!(w.first().copied(), Some("ecdsa_p256_verify"), "Zeile {}", i + 1);
        assert_eq!(w.len(), 4, "Zeile {}: Schluessel, Digest, Signatur", i + 1);
        out.push(Vector {
            line: i + 1,
            key: hex(i + 1, w[1]),
            digest: hex(i + 1, w[2]),
            sig: hex(i + 1, w[3]),
            want: want.trim() == "1",
        });
    }
    assert!(out.len() >= 6, "die Spezifikation traegt ihre Vektoren: {}", out.len());
    out
}

#[cfg(feature = "ecdsa")]
#[test]
fn every_vector_holds() {
    let wrong: Vec<String> = load()
        .iter()
        .filter(|v| takt_crypto::ecdsa_p256_verify(&v.key, &v.digest, &v.sig) != Ok(v.want))
        .map(|v| format!("Zeile {}", v.line))
        .collect();
    assert!(wrong.is_empty(), "Vektoren weichen ab: {}", wrong.join(", "));
    assert!(takt_crypto::MANIFEST.contains("p256"));
}

#[cfg(feature = "ecdsa")]
#[test]
fn garbage_never_panics() {
    let mut x = 0x2026_0916u64;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x as u8
    };
    for _ in 0..200 {
        let key: [u8; 64] = std::array::from_fn(|_| next());
        let digest: [u8; 32] = std::array::from_fn(|_| next());
        let sig: [u8; 64] = std::array::from_fn(|_| next());
        assert_eq!(takt_crypto::ecdsa_p256_verify(&key, &digest, &sig), Ok(false));
    }
    assert_eq!(takt_crypto::ecdsa_p256_verify(&[0; 64], &[0; 32], &[0; 64]), Ok(false));
}

#[cfg(not(feature = "ecdsa"))]
#[test]
fn without_the_feature_the_answer_is_unavailable() {
    assert_eq!(takt_crypto::ecdsa_p256_verify(&[0; 64], &[0; 32], &[0; 64]), Err(takt_crypto::Unavailable));
    assert!(!takt_crypto::MANIFEST.contains("p256"));
    let _ = load();
}
