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
/// den Tick. **Ohne Zeitgarantie misst die Pruefung die Zeit und sagt es**
/// (12.2, 13.8; FB-860): Auf einem Wirt ohne PREEMPT_RT nennt der Kopf den
/// Grund, und das Urteil sagt `ohne Zeiturteil`.
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
    if !realtime_kernel() {
        assert!(verdict.to_string().contains("ohne Zeiturteil: kein PREEMPT_RT"), "{verdict}");
    }
}

/// Laeuft der Wirt unter PREEMPT_RT (`/sys/kernel/realtime`)?
fn realtime_kernel() -> bool {
    std::fs::read_to_string("/sys/kernel/realtime").is_ok_and(|s| s.trim() == "1")
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
