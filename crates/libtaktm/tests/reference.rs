//! Die korrekt gerundeten Funktionen gegen die unabhaengige Referenz.
//!
//! `random.txt` traegt Zufallsvektoren je Funktion und Breite aus
//! `tools/libtaktm.py random`, `hard_f32.txt` die `f32`-Argumente, die der
//! erschoepfende Lauf (`exhaustive.rs`) nicht selbst entscheiden kann, weil
//! ihr Wert zu nahe an einer Rundungsgrenze liegt. Beide hat die Referenz in
//! Dezimalarithmetik gerundet; hier muss jedes Bitmuster stimmen.

mod common;

use common::{apply, load};

fn check(name: &str, at_least: usize) {
    let vectors = load(name);
    assert!(vectors.len() >= at_least, "{name}: nur {} Vektoren", vectors.len());
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

#[test]
fn hard_f32_cases_match_the_reference() {
    check("hard_f32.txt", 1);
}
