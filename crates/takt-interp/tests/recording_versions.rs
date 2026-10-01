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
    assert_eq!(RECORDING_VERSION, 3, "die Vorversionen dieses Tests sind 1 und 2");
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
    let v4 = V1.replacen("takt-aufzeichnung 1", "takt-aufzeichnung 4", 1);
    let e = Recording::parse(&v4).expect_err("neuer als diese Fassung");
    assert!(e.contains("11.3"), "{e}");
}
