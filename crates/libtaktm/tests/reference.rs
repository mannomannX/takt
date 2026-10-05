//! Die korrekt gerundeten Funktionen gegen die unabhaengige Referenz.
//!
//! `random.txt` traegt Zufallsvektoren je Funktion und Breite aus
//! `tools/libtaktm.py random`, `hard_f32.txt` die `f32`-Argumente, die der
//! erschoepfende Lauf (`exhaustive.rs`) nicht selbst entscheiden kann, weil
//! ihr Wert zu nahe an einer Rundungsgrenze liegt. Beide hat die Referenz in
//! Dezimalarithmetik gerundet; hier muss jedes Bitmuster stimmen. `fma.txt`
//! traegt Tripel mit exakter Referenz in Bruechen (`tools/libtaktm.py fma`).
//!
//! Jede Datei hat genau die Zahl von Vektoren, die ihr Test nennt: Eine
//! gekuerzte Datei bestuende sonst still (INT-028).

mod common;

use common::{apply, load};

fn check(name: &str, count: usize) {
    let vectors = load(name);
    assert_eq!(vectors.len(), count, "{name}: Zahl der Vektoren");
    let wrong: Vec<String> = vectors
        .iter()
        .filter_map(|v| match apply(v) {
            Some(got) if got == v.want => None,
            Some(got) => Some(format!("{}\n  erhalten {got:x}", v.text)),
            None => Some(format!("{}: unbekannte Funktion", v.text)),
        })
        .collect();
    assert!(wrong.is_empty(), "{name}: {} von {} weichen ab:\n{}", wrong.len(), vectors.len(), wrong.join("\n"));
}

#[test]
fn random_vectors_match_the_reference() {
    check("random.txt", 5000);
}

/// INT-027: Die schweren `f64`-Faelle aus `tools/libtaktm.py hard` — die
/// Argumente, deren exakter Wert am naechsten an einem Mittelpunkt liegt,
/// darunter die Kreuzungen kleiner Argumente naeher als 2^-50 Stellen.
#[test]
fn hard_f64_cases_match_the_reference() {
    check("hard_f64.txt", 144);
}

#[test]
fn hard_f32_cases_match_the_reference() {
    check("hard_f32.txt", 1016);
}

/// INT-029: `fma` beider Breiten gegen die exakte Referenz — Ausloeschung
/// bis auf den Rundungsfehler des Produkts, subnormale Ergebnisse, das
/// Vorzeichen der Null bei `a * b = -c`, Ueberlauf und `f32`-Faelle, in
/// denen ein `f64`-Zwischenergebnis doppelt rundete.
#[test]
fn fma_vectors_match_the_exact_reference() {
    check("fma.txt", 669);
}
