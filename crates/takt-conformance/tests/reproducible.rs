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
//! **Drei Stufen, drei Tests.** Die Eigenschaft kann an drei Stellen
//! brechen, und jede hat ihre eigene Ursache: eine `HashMap` im Codegen
//! macht die *IR* unstet, ein Zeitstempel oder Pfad das *Objekt*, und
//! eine ungeordnete Symboltabelle das *Binary*. Ein Test ueber alles
//! zusammen saehe nur, dass etwas kaputt ist; drei sagen, was.
//!
//! Die letzten beiden brauchen clang und ueberspringen sich ohne, wie
//! die uebrigen Werkzeugkettentests.

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
        let zeile = a.lines().zip(b.lines()).position(|(x, y)| x != y).unwrap_or(0);
        panic!(
            "die IR weicht ab, erste Abweichung in Zeile {zeile}:\n  a: {}\n  b: {}",
            a.lines().nth(zeile).unwrap_or(""),
            b.lines().nth(zeile).unwrap_or("")
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
    for dir in [&flach, &tief] {
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
    // der Test waere gruen und saehe nichts. Geprueft wird darum die
    // Ursache: dass im Kopf kein Zeitpunkt steht, der von der Uhr kommt.
    if let Some(stamp) = pe_timestamp(&binaries[0]) {
        assert_eq!(stamp, 0, "der PE-Kopf traegt einen Build-Zeitstempel ({stamp}); 11.3 verbietet ihn");
    }
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
