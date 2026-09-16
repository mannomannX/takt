//! `map<K, V, N>` ueber Byte-Slots (3.9): Einfuegen, Finden, Entfernen per
//! Rueckwaertsverschiebung — nach jedem Schritt ist jeder Schluessel noch
//! auffindbar, und die Reihenfolge ist die der Slots.

use std::collections::BTreeMap;

use takt_native::map::ByteMap;

fn key(n: u64) -> [u8; 8] {
    n.to_le_bytes()
}

#[test]
fn every_key_stays_findable_after_removals() {
    let mut data = vec![0u8; 16 * (1 + 8 + 4)];
    let mut m = ByteMap::new(&mut data, 16, 8, 4);
    let mut model: BTreeMap<u64, u32> = BTreeMap::new();
    // Ein fester Ablauf aus Einfuegen und Entfernen mit Kollisionen.
    let mut x = 0x9e37_79b9u64;
    for step in 0..400u32 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let k = x % 40;
        if step % 3 == 2 {
            assert_eq!(m.remove(&key(k)), model.remove(&k).is_some(), "remove {k} in Schritt {step}");
        } else {
            let ok = m.insert(&key(k), &step.to_le_bytes());
            if ok {
                model.insert(k, step);
            } else {
                assert_eq!(model.len(), 16, "nur eine volle Map lehnt ab");
                assert!(!model.contains_key(&k));
            }
        }
        assert_eq!(m.len(), model.len(), "Schritt {step}");
        for (k, v) in &model {
            assert_eq!(m.get(&key(*k)), Some(&v.to_le_bytes()[..]), "Schluessel {k} nach Schritt {step}");
        }
        for k in 0..40u64 {
            if !model.contains_key(&k) {
                assert_eq!(m.get(&key(k)), None);
            }
        }
    }
}

#[test]
fn insert_replaces_and_a_full_map_refuses() {
    let mut data = vec![0u8; 2 * (1 + 8 + 4)];
    let mut m = ByteMap::new(&mut data, 2, 8, 4);
    assert!(m.insert(&key(1), &1u32.to_le_bytes()));
    assert!(m.insert(&key(1), &2u32.to_le_bytes()), "ein vorhandener Schluessel wird ersetzt");
    assert_eq!(m.get(&key(1)), Some(&2u32.to_le_bytes()[..]));
    assert!(m.insert(&key(2), &3u32.to_le_bytes()));
    assert!(!m.insert(&key(3), &4u32.to_le_bytes()), "voll");
    assert_eq!(m.len(), 2);
    assert!(!m.remove(&key(9)));
    assert!(m.remove(&key(1)));
    assert!(m.is_empty() || m.len() == 1);
}

#[test]
fn iteration_is_slot_order() {
    let mut data = vec![0u8; 8 * (1 + 8 + 4)];
    let mut m = ByteMap::new(&mut data, 8, 8, 4);
    for k in [5u64, 3, 11, 7] {
        assert!(m.insert(&key(k), &(k as u32).to_le_bytes()));
    }
    let order: Vec<u64> =
        (0..8).filter_map(|i| m.entry(i)).map(|(k, _)| u64::from_le_bytes(k.try_into().expect("8 Byte"))).collect();
    let mut expected: Vec<(usize, u64)> =
        [5u64, 3, 11, 7].iter().map(|k| ((m.hash(&key(*k)) as usize) % 8, *k)).collect();
    expected.sort();
    // Ohne Kollision steht jeder Schluessel auf seiner Hash-Position.
    if expected.windows(2).all(|w| w[0].0 != w[1].0) {
        assert_eq!(order, expected.iter().map(|(_, k)| *k).collect::<Vec<_>>());
    }
    assert_eq!(order.len(), 4);
}
