//! Reproduzierbare Builds (11.3).
//!
//! 11.3: „Gleiche Quelle plus gleiche Toolchain-Version ergeben
//! bitidentische Binaries: feste LLVM-Flags, keine Zeitstempel oder
//! Pfade im Binary, deterministische Symbolordnung."
//!
//! **Warum das eine Zusage und keine Bequemlichkeit ist.** 11.3 nennt
//! zwei Gruende: Secure-Boot-Signaturpipelines und die Zertifizierung
//! (13.4) setzen es voraus. Dazu kommt ein dritter, der die Abnahme
//! selbst betrifft — der Logik-Hash identifiziert ein Programm im
//! Lauf-Header (8.4), und ein Hash taugt nur, wenn dieselbe Quelle
//! dasselbe Ergebnis liefert.
//!
//! **Vier Stufen, vier Tests.** Die Eigenschaft kann an mehreren
//! Stellen brechen, und jede hat ihre eigene Ursache: eine `HashMap` im
//! Codegen macht die *IR* unstet, ein Zeitstempel oder Pfad das
//! *Objekt*, eine ungeordnete Symboltabelle das *Binary* — und alles
//! davon kann je Zielarchitektur verschieden sein. Ein Test ueber alles
//! zusammen saehe nur, dass etwas kaputt ist; vier sagen, was und wo.
//!
//! **Warum die Zielarchitektur eine eigene Stufe ist.** Die ersten drei
//! Stufen pruefen den Wirt. Ein Programm, das auf eine MCU geht, wird
//! aber fuer ein anderes Ziel uebersetzt, und dort greift eine andere
//! Werkzeugkette: ein anderer Linker, andere Startdateien, andere
//! Standardbibliothek. 11.3 macht keine Ausnahme fuer das Ziel, also
//! darf der Test keine machen — sonst gaelte die Zusage nur dort, wo
//! ohnehin entwickelt wird, und nicht dort, wo signiert wird (13.4).
//!
//! Alle ausser der ersten brauchen clang und ueberspringen sich ohne,
//! wie die uebrigen Werkzeugkettentests.

use takt_llvm::Target;
use takt_llvm::toolchain::{Clang, find};
use takt_mir::program::Program;

mod common;

/// Ein Programm mit genug Vielfalt, dass eine ungeordnete Iteration
/// auffiele: mehrere Maschinen, Kanaele, Zustaende und ein Strom.
const NAME: &str = "23_patterns.takt";

fn corpus(name: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    assert!(!out.diagnostics.iter().any(|d| d.is_error()), "{name}");
    out.program.unwrap_or_else(|| panic!("{name}: kein Programm"))
}

/// **Stufe 1**: Zweimal uebersetzen ergibt dieselbe IR.
///
/// Das ist die Stufe, die der Codegen allein verantwortet. Sie bricht,
/// sobald irgendwo ueber eine `HashMap` iteriert wird — deren Reihenfolge
/// ist in Rust je Lauf anders, und genau dafuer gibt es diesen Test.
#[test]
fn the_same_source_yields_the_same_ir() {
    let p = corpus(NAME);
    let a = common::ir_of(&p);
    let b = common::ir_of(&p);
    assert_eq!(a.len(), b.len(), "die IR hat verschiedene Laenge");
    if a != b {
        let line = a.lines().zip(b.lines()).position(|(x, y)| x != y).unwrap_or(0);
        panic!(
            "die IR weicht ab, erste Abweichung in Zeile {line}:\n  a: {}\n  b: {}",
            a.lines().nth(line).unwrap_or(""),
            b.lines().nth(line).unwrap_or("")
        );
    }
}

/// **Stufe 2**: Dieselbe IR ergibt dasselbe Objekt — auch aus einem
/// anderen Verzeichnis.
///
/// Der Pfad ist der haeufigste Weg, auf dem ein Build seine Umgebung ins
/// Ergebnis traegt (Debug-Info, `__FILE__`, Fehlermeldungen). 11.3
/// verlangt ausdruecklich „keine Zeitstempel oder Pfade im Binary", also
/// wird aus zwei verschieden tiefen Verzeichnissen uebersetzt.
#[test]
fn the_same_ir_yields_the_same_object() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let p = corpus(NAME);
    let ir = common::ir_of(&p);

    let root = std::env::temp_dir().join("takt-repro");
    let _ = std::fs::remove_dir_all(&root);
    let flach = root.join("a");
    let tief = root.join("b/tiefer/noch_tiefer");
    std::fs::create_dir_all(&flach).expect("Verzeichnis");
    std::fs::create_dir_all(&tief).expect("Verzeichnis");

    let mut objekte = Vec::new();
    for (i, dir) in [&flach, &tief].into_iter().enumerate() {
        // Zwei Uebersetzungen in derselben Sekunde truegen denselben
        // Zeitstempel und waeren zufaellig gleich (FB-165).
        if i == 1 {
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
        let ll = dir.join("programm.ll");
        let obj = dir.join("programm.o");
        std::fs::write(&ll, &ir).expect("IR");
        let mut cmd = std::process::Command::new(&path);
        let out = Clang::deterministic(&mut cmd)
            .current_dir(dir)
            .args(["-Wno-override-module", "-O1", "-c", "programm.ll", "-o", "programm.o"])
            .output()
            .expect("clang");
        assert!(out.status.success(), "clang: {}", String::from_utf8_lossy(&out.stderr));
        objekte.push(std::fs::read(&obj).expect("Objekt"));
    }

    assert_eq!(objekte[0].len(), objekte[1].len(), "die Objekte haben verschiedene Groesse");
    assert!(
        objekte[0] == objekte[1],
        "die Objektdatei haengt vom Uebersetzungsverzeichnis ab; 11.3 verlangt Unabhaengigkeit"
    );
    // Und der Pfad steht auch nicht als Text darin — ein Objekt, das
    // zufaellig gleich gross ist, aber den Pfad traegt, waere ein
    // Fehlschlag, den der Vergleich oben nicht immer faengt.
    let text = String::from_utf8_lossy(&objekte[0]).to_string();
    assert!(!text.contains("noch_tiefer"), "der Pfad steht im Objekt");
}

/// **Stufe 3**: Zwei vollstaendige Uebersetzungen ergeben dasselbe
/// Binary.
///
/// Das ist die Zusage, wie 11.3 sie formuliert. Sie schliesst den Linker
/// ein, und der ist die Stelle, an der eine ungeordnete Symboltabelle
/// sichtbar wuerde.
#[test]
fn the_same_source_yields_the_same_binary() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let p = corpus(NAME);
    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    let ir = common::ir_of(&p);
    let harness = takt_conformance::harness::build_with(&p, &machine, 4, &[]).source;

    // Selbst uebersetzen statt ueber `run_native`: Der Test bestimmt
    // dann, wo die Dateien liegen und was mit ihnen geschieht — ein
    // Helfer, der sein Verzeichnis aufraeumt, waere hier eine
    // Abhaengigkeit von einem Nebeneffekt.
    let root = std::env::temp_dir().join("takt-repro-bin");
    let _ = std::fs::remove_dir_all(&root);
    let mut binaries = Vec::new();
    for lauf in 0..2 {
        if lauf == 1 {
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
        let dir = root.join(format!("lauf{lauf}"));
        std::fs::create_dir_all(&dir).expect("Verzeichnis");
        let ll = dir.join("programm.ll");
        let c = dir.join("rahmen.c");
        let exe = dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" });
        std::fs::write(&ll, &ir).expect("IR");
        std::fs::write(&c, &harness).expect("Rahmen");
        let mut cmd = std::process::Command::new(&path);
        let out = Clang::deterministic(&mut cmd)
            .args(["-Wno-override-module", "-O1"])
            .arg(&ll)
            .arg(&c)
            .arg("-o")
            .arg(&exe)
            .output()
            .expect("clang");
        assert!(out.status.success(), "clang: {}", String::from_utf8_lossy(&out.stderr));
        binaries.push(std::fs::read(&exe).unwrap_or_else(|e| panic!("{}: {e}", exe.display())));
    }

    assert_eq!(binaries[0].len(), binaries[1].len(), "die Binaries haben verschiedene Groesse");
    assert!(
        binaries[0] == binaries[1],
        "zwei Uebersetzungen derselben Quelle ergeben verschiedene Binaries; 11.3 verlangt Bitgleichheit"
    );
    // **Der Vergleich allein genuegt nicht.** Zwei Laeufe in derselben
    // Sekunde tragen denselben Zeitstempel und sind zufaellig gleich —
    // darum liegt zwischen ihnen eine Sekunde. Im PE-Kopf steht kein
    // Zeitpunkt von der Uhr: lld schreibt im reproduzierbaren Modus einen
    // Hash des Inhalts in das Feld (FB-165), gleich fuer beide Laeufe.
    assert_eq!(
        pe_timestamp(&binaries[0]),
        pe_timestamp(&binaries[1]),
        "der PE-Kopf traegt einen Zeitpunkt von der Uhr; 11.3 verbietet ihn"
    );
}

/// **Stufe 4**: Auch je Zielarchitektur ist die Uebersetzung
/// reproduzierbar (11.3, 12.8).
///
/// Die Stufen davor pruefen den Wirt. Diese prueft *jedes* Ziel, das M4
/// traegt — x86-64 und aarch64 —, weil jede Zielkette ihren eigenen
/// Linker und ihre eigenen Startdateien mitbringt.
///
/// **Was diese Stufe nicht faengt.** Der Zeitstempel aus FB-119 ist ein
/// PE-Feld; ELF traegt keines, und die Ziele hier sind beide ELF. Ein
/// Lauf ohne `SOURCE_DATE_EPOCH` bleibt darum gruen — nachgemessen: auch
/// ueber eine Sekundengrenze hinweg sind zwei ELF-Binaries gleich. Die
/// Stufe prueft also die *uebrigen* Quellen von Unbestimmtheit je Ziel
/// (Symbolordnung, Pfade, Linkerzustand); den Zeitstempel prueft Stufe 3
/// auf dem Wirt, wo er auftreten kann.
///
/// Der Test ueberspringt sich ohne die Cross-Kette; `tools/linux.sh`
/// bringt sie mit.
#[test]
fn every_target_builds_reproducibly() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    if !cross_available() {
        eprintln!("uebersprungen: aarch64-Werkzeugkette fehlt (tools/Dockerfile.linux baut sie)");
        return;
    }
    let p = corpus(NAME);
    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    let harness = takt_conformance::harness::build(&p, &machine, 4).source;

    let root = std::env::temp_dir().join("takt-repro-ziel");
    let _ = std::fs::remove_dir_all(&root);
    let mut geprueft = 0;
    for target in [Target::X86_64_LINUX, Target::AARCH64_LINUX] {
        let ir = common::ir_for(&p, target.triple);
        let mut binaries = Vec::new();
        for lauf in 0..2 {
            let dir = root.join(format!("{}-{lauf}", target.name));
            std::fs::create_dir_all(&dir).expect("Verzeichnis");
            let ll = dir.join("programm.ll");
            let c = dir.join("rahmen.c");
            let obj = dir.join("programm.o");
            let exe = dir.join("lauf");
            std::fs::write(&ll, &ir).expect("IR");
            std::fs::write(&c, &harness).expect("Rahmen");

            let mut cmd = std::process::Command::new(&path);
            let out = Clang::deterministic(&mut cmd)
                .args(["-Wno-override-module", "-O1", "-c"])
                .arg(format!("--target={}", target.triple))
                .arg(&ll)
                .arg("-o")
                .arg(&obj)
                .output()
                .expect("clang");
            assert!(out.status.success(), "{}: {}", target.name, String::from_utf8_lossy(&out.stderr));

            let linker = if target == Target::AARCH64_LINUX { "aarch64-linux-gnu-gcc" } else { "cc" };
            let mut cmd = std::process::Command::new(linker);
            let out = Clang::deterministic(&mut cmd).arg(&c).arg(&obj).arg("-o").arg(&exe).output().expect("Linker");
            assert!(out.status.success(), "{}: {}", target.name, String::from_utf8_lossy(&out.stderr));
            binaries.push(std::fs::read(&exe).expect("Binary"));
        }
        assert_eq!(binaries[0].len(), binaries[1].len(), "{}: verschiedene Groesse", target.name);
        assert!(
            binaries[0] == binaries[1],
            "{}: zwei Uebersetzungen ergeben verschiedene Binaries; 11.3 verlangt Bitgleichheit",
            target.name
        );
        geprueft += 1;
    }
    assert_eq!(geprueft, 2, "es wurden nicht beide Ziele geprueft");
    eprintln!("{geprueft} Ziele reproduzierbar uebersetzt");
}

/// Ist die Werkzeugkette fuer aarch64 da?
///
/// Dieselbe Pruefung wie in `targets.rs`; sie steht dort und hier, weil
/// Integrationstests keine Module teilen ausser ueber `common` — und
/// `common` ist der Ort fuer das Bauen, nicht fuer das Suchen.
fn cross_available() -> bool {
    std::process::Command::new("qemu-aarch64").arg("--version").output().is_ok_and(|o| o.status.success())
        && std::process::Command::new("aarch64-linux-gnu-gcc")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
}

/// Der `TimeDateStamp` aus dem PE-Kopf (Windows), falls es einer ist.
///
/// `None` auf jedem anderen Format — ELF hat kein solches Feld, und
/// Mach-O traegt es an anderer Stelle. Der Test prueft dann nur den
/// Vergleich, und das ist dort auch die ganze Aussage.
fn pe_timestamp(bin: &[u8]) -> Option<u32> {
    if bin.len() < 0x40 || &bin[..2] != b"MZ" {
        return None;
    }
    let at = u32::from_le_bytes(bin[0x3C..0x40].try_into().ok()?) as usize;
    if bin.len() < at + 12 || &bin[at..at + 4] != b"PE\0\0" {
        return None;
    }
    Some(u32::from_le_bytes(bin[at + 8..at + 12].try_into().ok()?))
}
