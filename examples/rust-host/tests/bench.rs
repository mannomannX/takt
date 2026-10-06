//! Das Messprogramm von `takt bench` auf dem Wirt (13.8, plan/m11.md 2.12):
//! gebaut wie jedes Programm (`takt_embed::build::Bench`), mit der Uhr des
//! Wirts als Zyklenzaehler und einem Puffer als Senke gefahren — und `takt
//! bench --import` nimmt das Protokoll an. So laesst sich jedes Board
//! kalibrieren, auf dem ein Wirt laeuft; hier ist der Wirt der PC.

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use rust_host::takt_bench;

/// Der Wirt: Nanosekunden seit dem Start als Zyklen, das Protokoll im Speicher.
struct Host {
    start: Instant,
    log: Vec<u8>,
}

impl takt_bench::Host for Host {
    fn cycles(&mut self) -> u32 {
        // Der Zaehler darf ueberlaufen; das Messprogramm rechnet mit dem Abstand.
        self.start.elapsed().as_nanos() as u32
    }

    fn put(&mut self, byte: u8) {
        self.log.push(byte);
    }
}

/// **Alle Schritte laufen, und der Import rechnet daraus eine Tabelle**:
/// Jeder Referenzkern ergibt denselben Digest wie seine C-Referenz, jede
/// Funktion der Mathematik das Ergebnis der Norm, kein Subnormal-Fall
/// faellt durch (4.2).
#[test]
fn the_bench_runs_on_the_host_and_imports() {
    let mut host = Host { start: Instant::now(), log: Vec::new() };
    takt_bench::begin(&mut host, 1_000_000_000);
    takt_bench::steps(&mut host, 20, |measure| measure());
    takt_bench::end(&mut host);
    let log = String::from_utf8(host.log).expect("das Protokoll ist Text");
    assert!(log.contains(&format!("takt bench {}", takt_bench::SUITE)), "{log}");

    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bench.log");
    std::fs::write(&path, &log).expect("Protokoll schreiben");
    let tool = std::env::var_os("TAKT").map(PathBuf::from).expect("`TAKT` nennt das Werkzeug (FB-392)");
    let out = Command::new(tool)
        .args(["bench", "--import"])
        .arg(&path)
        .args(["--target", "x86_64"])
        .output()
        .expect("takt aufrufbar");
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{text}");
    assert_eq!(text.matches("gleicher Digest").count(), 6, "{text}");
    assert!(!text.contains("WEICH"), "{text}");
}
