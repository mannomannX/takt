//! Der Port fuer Systeme mit Faeden (12.11, M11 Schritte 18, 18c):
//! `examples/posix` faehrt das Pruefprogramm ueber `ports/rtos` und den
//! POSIX-Adapter in Echtzeit, und das Urteil der Portpruefung nimmt den Lauf
//! an. Die Vorlage `c-port` (`ports/template`) uebersetzt mit demselben Kern.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use takt_llvm::toolchain::find;

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    dir
}

/// **Der POSIX-Port besteht die Portpruefung** (12.11, 13.8): Die Lieferform
/// des Pruefprogramms in der Form `rtos`, mit der Nachricht je Job nach dem
/// Durchsatz dieses Rechners und den Stack-Reserven aus
/// `corpus-try/hw/linux-x86_64.hw`, dazu das SDK-Archiv, der Port und
/// `examples/posix/main.c`, mit clang gebunden. Der Lauf ueber
/// [`takt_conformance::port::TICKS`] Ticks in Echtzeit muss jede Pflicht
/// erfuellen: Trace wie der Interpreter, Jobs ueber Tickgrenzen bei
/// puenktlichen Ticks, die fremde Last ohne Fehler, beide Stacks unter ihrer
/// Schranke, das Journal schrieb. Der Test laeuft allein (nextest,
/// `threads-required`): Neben anderen Tests verspaetete ein voller Runner
/// den Tick.
#[test]
#[cfg_attr(
    not(all(target_os = "linux", target_arch = "x86_64")),
    ignore = "der POSIX-Port wird unter Linux x86-64 geprueft (corpus-try/hw/linux-x86_64.hw)"
)]
fn a_posix_port_passes_the_port_test() {
    let Some(clang) =
        takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")
    else {
        return;
    };
    let dir = scratch("takt-posix-port");
    let embed = dir.join("embed");
    let out = embed.to_str().expect("Pfad");
    let hardware = "corpus-try/hw/linux-x86_64.hw";
    let run = takt(&["port-test", "--emit", "embed", "--form", "rtos", "--hardware", hardware, "--out", out]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let sdk = takt_conformance::harness::sdk_library_for(None).unwrap_or_else(|e| panic!("SDK-Archiv: {e}"));
    let ports = root().join("crates/takt-embed-c/ports");
    let exe = dir.join("posix_port");
    let build = Command::new(clang)
        .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"])
        .arg("-I")
        .arg(&embed)
        .arg("-I")
        .arg(ports.join("rtos"))
        .arg("-I")
        .arg(ports.join("posix"))
        .arg(root().join("examples/posix/main.c"))
        .arg(ports.join("rtos/takt_rtos.c"))
        .arg(ports.join("posix/takt_os.c"))
        .arg(embed.join("libport_test.a"))
        .arg(&sdk)
        .args(takt_llvm::toolchain::Clang::HOST_LIBS)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang startet");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    if let Ok(n) = std::env::var("TAKT_PROBE_RUNS") {
        probe(&exe, &embed, n.parse().expect("Zahl"));
        return;
    }
    let ran = Command::new(&exe).arg(takt_conformance::port::TICKS.to_string()).output().expect("der Port startet");
    let log = String::from_utf8_lossy(&ran.stdout);
    assert!(ran.status.success(), "der Port brach ab ({}): {}", ran.status, String::from_utf8_lossy(&ran.stderr));
    let source = std::fs::read_to_string(embed.join("port_test.takt")).expect("Quelle");
    let verdict = takt_conformance::port::judge(&log, &source);
    // Die gemessenen Tiefen sind die Reserven der Hardware-Konfiguration.
    eprintln!("{verdict}");
    assert!(verdict.passed(), "{verdict}\n{}{}", String::from_utf8_lossy(&run.stdout), excerpt(&log));
    // 12.11: Faeden eines Prozesses teilen seinen Speicher.
    assert_eq!(verdict.protection.map(|c| c.name()), Some("keiner"), "{verdict}");
}

/// **Die Vorlage `c-port` passt zum Port** (12.11, 2.9): `ports/template`
/// mit `ports/rtos/takt_rtos.c` und dem Kopf des SDK uebersetzt ohne
/// Warnung. Aendert sich `takt_os.h`, faellt dieser Test, bis die Vorlage
/// folgt.
#[test]
fn the_port_template_compiles_with_the_thread_port() {
    let Some(clang) =
        takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")
    else {
        return;
    };
    let ports = root().join("crates/takt-embed-c/ports");
    let check = Command::new(clang)
        .args(["-fsyntax-only", "-std=c11", "-Wall", "-Wextra", "-Werror"])
        .arg("-I")
        .arg(ports.join("rtos"))
        .arg("-I")
        .arg(ports.join("template"))
        .arg("-I")
        .arg(root().join("crates/takt-embed-c/include"))
        .arg(ports.join("rtos/takt_rtos.c"))
        .arg(ports.join("template/takt_os.c"))
        .output()
        .expect("clang startet");
    assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
}

/// Sonde FB-860: je Variante `runs` Laeufe, je Lauf Ueberlaeufe, Verspaetungen,
/// Overrun-Zeilen, spaetester Tick und Jitter.
fn probe(exe: &Path, embed: &Path, runs: usize) {
    let source = std::fs::read_to_string(embed.join("port_test.takt")).expect("Quelle");
    let ticks = takt_conformance::port::TICKS.to_string();
    let exe = exe.to_str().expect("Pfad");
    let variants: [(&str, Vec<&str>); 4] = [
        ("wie bisher", vec![exe]),
        ("mlockall", vec!["env", "TAKT_PROBE_MLOCK=1", exe]),
        ("sudo chrt -f 80 + mlockall", vec!["sudo", "env", "TAKT_PROBE_MLOCK=1", "chrt", "-f", "80", exe]),
        ("sudo, Schrittfaden FIFO 80 + mlockall", vec!["sudo", "env", "TAKT_PROBE_MLOCK=1", "TAKT_PROBE_PRIO=80", exe]),
    ];
    for (name, command) in variants {
        let mut rows = Vec::new();
        for _ in 0..runs {
            let ran = Command::new(command[0]).args(&command[1..]).arg(&ticks).output().expect("startet");
            let log = String::from_utf8_lossy(&ran.stdout);
            let v = takt_conformance::port::judge(&log, &source);
            let count = |label: &str| takt_conformance::board::counter(&log, label).unwrap_or(0);
            let overruns = log.lines().filter(|l| l.contains(" runtime Overrun")).count();
            rows.push((
                count("ueberlaeufe"),
                count("verspaetet"),
                overruns,
                v.drift.late,
                v.drift.jitter.unwrap_or(-1),
            ));
            if !ran.status.success() || rows.len() == 1 {
                eprintln!("SONDE {name}: Status {} stderr {}", ran.status, String::from_utf8_lossy(&ran.stderr).trim());
            }
        }
        let bad = rows.iter().filter(|r| r.0 > 0 || r.1 > 0 || r.2 > 0).count();
        let mut late: Vec<i64> = rows.iter().map(|r| r.3).collect();
        late.sort_unstable();
        let mut jitter: Vec<i64> = rows.iter().map(|r| r.4).collect();
        jitter.sort_unstable();
        let sum = |f: fn(&(u64, u64, usize, i64, i64)) -> u64| rows.iter().map(f).sum::<u64>();
        eprintln!(
            "SONDE {name}: {runs} Laeufe, {bad} mit Zeitbefund; ueberlaeufe {} verspaetet {} Overrun {}; \
             spaetester Tick ns Median {} max {}; Jitter ns Median {} max {}",
            sum(|r| r.0),
            sum(|r| r.1),
            sum(|r| r.2 as u64),
            late[late.len() / 2],
            late[late.len() - 1],
            jitter[jitter.len() / 2],
            jitter[jitter.len() - 1]
        );
    }
}

/// Was ein Lauf, der durchfiel, vom Port zeigt: der Kopf, die Zeilen der
/// Jobs und von `hashes`, das Ende.
fn excerpt(log: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    let pick = |what: &str| lines.iter().filter(|l| l.contains(what)).take(8).copied().collect::<Vec<_>>().join("\n");
    let tail = lines.iter().rev().take(4).copied().collect::<Vec<_>>().join("\n");
    format!(
        "Kopf des Laufs:\n{}\nJobs:\n{}\n`hashes`:\n{}\nEnde des Laufs:\n{tail}",
        lines.iter().take(12).copied().collect::<Vec<_>>().join("\n"),
        pick(" job "),
        pick(" hashes ")
    )
}
