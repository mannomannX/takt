//! Die Aufzeichnung (12.5) liest ihre Vorversion (11.3): Version 1 kannte
//! keinen Parametervektor im Kopf, Version 2 traegt ihn.

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
    assert_eq!(RECORDING_VERSION, 2, "die Vorversion dieses Tests ist 1");
    let r = Recording::parse(V1).expect("Version 1 ist lesbar");
    assert_eq!(r.header.version, 1);
    assert!(r.header.overrides().is_empty(), "Version 1 nannte nur die Defaults, die ohnehin gelten");
    assert_eq!(r.inputs.lines.len(), 1);
}

#[test]
fn a_newer_version_is_refused() {
    let v3 = V1.replacen("takt-aufzeichnung 1", "takt-aufzeichnung 3", 1);
    let e = Recording::parse(&v3).expect_err("neuer als diese Fassung");
    assert!(e.contains("11.3"), "{e}");
}
