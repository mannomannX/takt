//! Die Aufzeichnung (12.5) liest ihre Vorversionen (11.3): Version 1
//! kannte keinen Parametervektor im Kopf, Version 2 traegt ihn, Version 3
//! dazu Zeitstempel und Folgenummern der Lieferungen (12.6).

use takt_interp::record::{RECORDING_VERSION, Recording};

const V1: &str = "#! takt-aufzeichnung 1
#! edition 1
#! logik 00ff
#! tick 10000000
#! ticks 2
#! param GAIN 3
t=1 in x 1
";

#[test]
fn version_one_still_reads_without_a_parameter_vector() {
    assert_eq!(RECORDING_VERSION, 4, "die Vorversionen dieses Tests sind 1 bis 3");
    let r = Recording::parse(V1).expect("Version 1 ist lesbar");
    assert_eq!(r.header.version, 1);
    assert!(r.header.overrides().is_empty(), "Version 1 nannte nur die Defaults, die ohnehin gelten");
    assert_eq!(r.inputs.lines.len(), 1);
}

/// Eine Lieferung der Version 2 hat keinen eigenen Zeitstempel; sie liest
/// sich wie zuvor, und der Lauf nimmt die Tickgrenze.
#[test]
fn version_two_reads_without_timestamps() {
    let v2 = V1.replacen("takt-aufzeichnung 1", "takt-aufzeichnung 2", 1);
    let r = Recording::parse(&v2).expect("Version 2 ist lesbar");
    assert_eq!(r.header.version, 2);
    assert_eq!(r.inputs.render(), "t=1 in x 1\n");
}

#[test]
fn a_newer_version_is_refused() {
    let newer = V1.replacen("takt-aufzeichnung 1", &format!("takt-aufzeichnung {}", RECORDING_VERSION + 1), 1);
    let e = Recording::parse(&newer).expect_err("neuer als diese Fassung");
    assert!(e.contains("11.3"), "{e}");
}

/// INT-013: Ein Kopf, dem eine Pflichtzeile fehlt oder der eine doppelt
/// traegt, ist kein Kopf. Version 0 machte `overrides()` leer, ein
/// fehlendes `ticks` einen Lauf von null Ticks — beides still.
#[test]
fn a_broken_header_is_refused() {
    let cases = [
        (V1.replacen("#! tick 10000000\n", "", 1), "ohne `tick`"),
        (V1.replacen("#! ticks 2\n", "", 1), "ohne `ticks`"),
        (V1.replacen("#! logik 00ff\n", "", 1), "ohne `logik`"),
        (V1.replacen("#! edition 1\n", "", 1), "ohne `edition`"),
        (V1.replacen("takt-aufzeichnung 1", "takt-aufzeichnung 0", 1), "Version 0"),
        (V1.replacen("takt-aufzeichnung 1", "takt-aufzeichnung x", 1), "Version x"),
        (V1.replacen("#! tick 10000000\n", "#! tick 10000000\n#! tick 20000000\n", 1), "doppelte Kopfzeile"),
        (V1.replacen("#! ticks 2\n", "#! ticks zwei\n", 1), "`ticks` keine Zahl"),
        (V1.replacen("#! tick 10000000\n", "#! tick 0\n", 1), "Tick null"),
        (V1.replacen("#! param GAIN 3\n", "#! param GAIN 3\n#! param GAIN 4\n", 1), "doppelter Parameter"),
        (V1.replacen("#! param GAIN 3\n", "#! param GAIN\n", 1), "Parameter ohne Wert"),
    ];
    let mut accepted = Vec::new();
    for (text, why) in &cases {
        if let Ok(r) = Recording::parse(text) {
            accepted.push(format!("  {why}: {:?}", r.header));
        }
    }
    assert!(accepted.is_empty(), "angenommen statt abgelehnt:\n{}", accepted.join("\n"));
}

/// Eine Aufzeichnung der Version 3 mit Zeitstempel und Folgenummer liest
/// sich als dieselbe Aufzeichnung zurueck.
#[test]
fn a_version_three_recording_reads_back_as_itself() {
    let text = V1.replacen("takt-aufzeichnung 1", "takt-aufzeichnung 3", 1)
        + "t=2 in rx \"go\" t=15000000 seq=4\nt=3 in p 5 bar t=25000000\n";
    let r = Recording::parse(&text).expect("Version 3 ist lesbar");
    assert_eq!(r.render(), text);
    let again = Recording::parse(&r.render()).expect("wieder lesbar");
    assert_eq!((again.header, again.inputs), (r.header, r.inputs));
}

/// SEM2-044: 12.10 und Pruefung 59 — die Freigabe `with polling =
/// unchecked` „erscheint im Lauf-Header".
#[test]
fn an_unchecked_polling_release_appears_in_the_header() {
    let mut p = takt_mir::program::Program::new(takt_mir::program::Config::new(1, 1_000_000));
    let mut m = takt_mir::machine::Machine::new("uart_poll");
    m.polling_unchecked = true;
    p.machines.push(m);
    let text = takt_interp::record::Header::of(&p, None, &[], 1).render();
    assert!(text.lines().any(|l| l.starts_with("#! ") && l.contains("polling") && l.contains("uart_poll")), "{text}");
}

/// SEM2-044, SEM2-045: Version 4 traegt die Maschinen mit ungeprueftem
/// Polling und s0 der `persist`-Variablen; beides liest sich zurueck, und
/// eine Datei der Version 3 ohne die Zeilen meint einen leeren Speicher.
#[test]
fn a_version_four_header_reads_back_as_itself() {
    let text = V1.replacen("takt-aufzeichnung 1", "takt-aufzeichnung 4", 1);
    let text =
        text.replacen("#! ticks", "#! polling-unchecked uart_poll\n#! persist 0700000000000000010000002a\n#! ticks", 1);
    let r = Recording::parse(&text).expect("Version 4 ist lesbar");
    assert_eq!(r.header.polling_unchecked, ["uart_poll"]);
    assert_eq!(r.header.persist.as_deref(), Some("0700000000000000010000002a"));
    let again = Recording::parse(&r.render()).expect("wieder lesbar");
    assert_eq!(again.header, r.header);
    let v3 = Recording::parse(&V1.replacen("takt-aufzeichnung 1", "takt-aufzeichnung 3", 1)).expect("Version 3");
    assert!(v3.header.polling_unchecked.is_empty() && v3.header.persist.is_none());
}
