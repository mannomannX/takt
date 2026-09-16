//! `FileTunables` (8.4): Zeilen der Aufzeichnung werden je Tick-Grenze als
//! Satz ausgeliefert; was der Aufsatz nicht aufloest, faellt weg.

use takt_rt_core::loopcore::Tunables;
use takt_rt_linux::FileTunables;

fn resolve(name: &str, value: &str) -> Option<(u32, Vec<u8>)> {
    let index = match name {
        "KP" => 0,
        "KI" => 1,
        _ => return None,
    };
    let v: i64 = value.split(' ').next()?.parse().ok()?;
    Some((index, v.to_le_bytes().to_vec()))
}

#[test]
fn a_tick_delivers_its_set_in_order() {
    let mut t = FileTunables::parse(
        "t=0 out y 1\nt=3 tune KP 5 pct\nt=3 tune KI 7 pct\nt=8 tune KP 9 pct\nt=8 tune ZZ 1\n",
        &resolve,
    );
    assert_eq!(t.pending(), 3, "die unbekannte `ZZ` faellt weg, `out` ist keine `tune`-Zeile");
    let mut seen = Vec::new();
    t.poll(2, &mut |p, v| seen.push((p, v.to_vec())));
    assert!(seen.is_empty());
    t.poll(3, &mut |p, v| seen.push((p, v.to_vec())));
    assert_eq!(seen, vec![(0, 5i64.to_le_bytes().to_vec()), (1, 7i64.to_le_bytes().to_vec())]);
    seen.clear();
    // Ein uebersprungener Tick holt seinen Satz nach (9.9).
    t.poll(10, &mut |p, v| seen.push((p, v.to_vec())));
    assert_eq!(seen, vec![(0, 9i64.to_le_bytes().to_vec())]);
    assert_eq!(t.pending(), 0);
}
