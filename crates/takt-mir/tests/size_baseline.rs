//! Die Baseline von `takt size` (11.5, D1): versioniert, roundtrip-fest,
//! liest ihre Vorversion (11.3), und der Vergleich nennt die Posten.

use takt_mir::analysis::size::{BASELINE_VERSION, Item, Origin, Size};

fn sample() -> Size {
    Size {
        items: vec![
            Item { name: "Maschinenzustaende (Overlay)".into(), bytes: 40, origin: Origin::Exact },
            Item { name: "Flash (Code, Konstanten)".into(), bytes: 0, origin: Origin::Open },
            Item { name: "Stack nativer Funktionen".into(), bytes: 256, origin: Origin::Contract },
        ],
    }
}

fn with_version(v: u16) -> String {
    sample().baseline().replacen(&format!("# takt-size {BASELINE_VERSION}"), &format!("# takt-size {v}"), 1)
}

#[test]
fn the_baseline_round_trips() {
    let text = sample().baseline();
    assert!(text.starts_with(&format!("# takt-size {BASELINE_VERSION}\n")), "{text}");
    let back = Size::from_baseline(&text).expect("lesbar");
    assert_eq!(back.items.len(), 3);
    assert_eq!((back.ram_total(), back.flash_total()), (sample().ram_total(), sample().flash_total()));
    assert!(sample().diff(&back).is_empty());
}

#[test]
fn an_older_version_reads_and_a_newer_is_refused() {
    assert!(Size::from_baseline(&with_version(0)).is_ok(), "11.3: Leser akzeptieren aeltere Versionen");
    let e = Size::from_baseline(&with_version(BASELINE_VERSION + 1)).expect_err("neuer als diese Fassung");
    assert!(e.contains("11.3"), "{e}");
    assert!(Size::from_baseline("Maschinenzustaende;1;exakt\n").is_err(), "ohne Kopf keine Baseline");
}

#[test]
fn the_difference_names_changed_new_and_missing_items() {
    let mut now = sample();
    now.items[0].bytes = 48;
    now.items.push(Item { name: "sched-Warteschlangen".into(), bytes: 16, origin: Origin::Exact });
    now.items.remove(2);
    let d = now.diff(&sample());
    let rows: Vec<(&str, Option<u64>, Option<u64>)> = d.iter().map(|d| (d.name.as_str(), d.before, d.after)).collect();
    assert_eq!(
        rows,
        [
            ("Maschinenzustaende (Overlay)", Some(40), Some(48)),
            ("sched-Warteschlangen", None, Some(16)),
            ("Stack nativer Funktionen", Some(256), None),
        ]
    );
}
