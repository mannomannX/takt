//! `takt size` gegen das erzeugte Objekt (11.5, 12.3).
//!
//! **Warum gemessen wird.** 11.5 nennt jeden gerechneten Posten `exakt`
//! — „aus der MIR gerechnet". Das ist eine Behauptung ueber ein Binary,
//! das der Compiler noch nicht gesehen hat, und fuer `baremetal` wird
//! daraus ein Compile-Fehler, wenn die Summe die Hardware sprengt. Eine
//! Zahl, auf die sich eine Ablehnung stuetzt, gehoert gemessen.
//!
//! **Was hier schon geht und was auf M5 wartet.** Die Abschnitte
//! (`.text`, `.rodata`) und der Stackrahmen je Funktion lassen sich auf
//! dem Wirt messen; das genuegt, um die Rechnung zu pruefen. Ob die
//! Summe *passt*, entscheidet erst die Hardware-Konfiguration (8.10,
//! M6) gegen ein echtes Ziel (M5) — hier entsteht das Werkzeug und der
//! Vergleich der Groessenordnung.
//!
//! Die Tests ueberspringen sich ohne clang und ohne die Binutils.

use takt_llvm::inspect::Binutils;
use takt_llvm::toolchain::{Clang, find};
use takt_mir::analysis::size;
use takt_mir::program::Program;

mod common;

/// Ein Programm mit Mustern: Es traegt DFA-Tabellen, und die sind der
/// groesste gerechnete Posten (11.5).
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

/// Uebersetzt die IR zu einem Objekt und liefert dessen Pfad.
fn object(p: &Program, dir: &std::path::Path, clang: &std::path::Path) -> Option<std::path::PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let ll = dir.join("programm.ll");
    let obj = dir.join("programm.o");
    std::fs::write(&ll, common::ir_of(p)).ok()?;
    let mut cmd = std::process::Command::new(clang);
    let out = Clang::deterministic(&mut cmd)
        .args(["-Wno-override-module", "-O1", "-c"])
        .arg(&ll)
        .arg("-o")
        .arg(&obj)
        .output()
        .ok()?;
    out.status.success().then_some(obj)
}

/// Die gerechneten DFA-Tabellen stehen wirklich im Objekt (11.5).
///
/// **Der Posten war zu gross.** `dfa_bytes` zaehlte je *Muster*, der
/// Codegen emittiert aber je *Zustand* einen Produkt-DFA (11.2: „alle
/// Muster der Handler eines Zustands werden zu einem Produkt-DFA
/// vereinigt"). Dazu zaehlte die Rechnung die akzeptierenden Zustaende
/// als Tabelle, obwohl der Codegen sie als Vergleichskette emittiert
/// (`dfa::run`). Beides zusammen ergab bei `23_patterns` 1696 Byte
/// gegen 424 gemessene (FB-121).
///
/// Der Test haelt die Rechnung an der Messung fest: Die Tabellen sind
/// der Loewenanteil von `.rodata`, also darf der Posten nicht groesser
/// sein als der Abschnitt.
#[test]
fn the_dfa_tables_fit_into_rodata() {
    let Clang::At(clang) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let tools = Binutils::host();
    if !tools.available() {
        eprintln!("uebersprungen: binutils nicht gefunden");
        return;
    }
    let p = corpus(NAME);
    let dir = std::env::temp_dir().join("takt-measured-dfa");
    let _ = std::fs::remove_dir_all(&dir);
    let Some(obj) = object(&p, &dir, &clang) else {
        panic!("das Objekt liess sich nicht bauen");
    };
    let Some(sections) = tools.sections(&obj) else {
        eprintln!("uebersprungen: `size` liest dieses Format nicht");
        return;
    };

    let report = size::size(&p);
    let gerechnet = report
        .items
        .iter()
        .find(|i| i.name.starts_with("DFA-Tabellen"))
        .map(|i| i.bytes)
        .expect("der Posten steht im Bericht");

    assert!(gerechnet > 0, "das Programm traegt Muster, also auch Tabellen");
    assert!(
        gerechnet <= sections.rodata,
        "`takt size` rechnet {gerechnet} Byte DFA-Tabellen, `.rodata` hat aber nur {}; \
         die Rechnung nennt Bytes, die nicht existieren",
        sections.rodata
    );
}

/// Der Posten „Flash (Code, Konstanten)" laesst sich messen (11.5).
///
/// Er steht im Bericht auf `offen`, weil die Rechnung ihn aus der MIR
/// nicht kennt — Codegroesse haengt am Ziel und an der Optimierung. Der
/// Test belegt, dass die Messung ihn liefert; sie in den Bericht zu
/// nehmen ist eine Entscheidung fuer M5, wo ein Ziel feststeht.
#[test]
fn the_flash_share_can_be_measured() {
    let Clang::At(clang) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let tools = Binutils::host();
    if !tools.available() {
        eprintln!("uebersprungen: binutils nicht gefunden");
        return;
    }
    let p = corpus(NAME);
    let dir = std::env::temp_dir().join("takt-measured-flash");
    let _ = std::fs::remove_dir_all(&dir);
    let Some(obj) = object(&p, &dir, &clang) else {
        panic!("das Objekt liess sich nicht bauen");
    };
    let Some(sections) = tools.sections(&obj) else {
        eprintln!("uebersprungen: `size` liest dieses Format nicht");
        return;
    };

    assert!(sections.text > 0, "ein Programm mit zwei Schrittfunktionen hat Code");
    assert!(sections.rodata > 0, "ein Programm mit Mustern hat Konstanten");
    assert!(sections.flash() >= sections.text + sections.rodata, "`flash()` fasst beide");
    eprintln!("{NAME}: .text {} B, .rodata {} B, Flash {} B", sections.text, sections.rodata, sections.flash());
}

/// Die Schrittfunktion steht als Symbol im Objekt, und ihr Stackrahmen
/// ist messbar (12.3).
///
/// 12.3 rechnet die Stacktiefe als „laengsten Pfad im azyklischen
/// Aufrufgraphen plus die `stack`-Vertraege nativer Funktionen". Der
/// Rahmen je Funktion ist die Zahl, aus der dieser Pfad entsteht — und
/// er ist nur am Objekt zu haben.
///
/// Der Test prueft nicht *wie gross* er ist: Das haengt an Ziel und
/// Optimierung, und eine Schranke waere hier eine Zahl ohne Quelle. Er
/// prueft, dass die Messung funktioniert — damit M5 sie benutzen kann,
/// statt sie dann erst zu bauen.
#[test]
fn the_step_function_has_a_measurable_frame() {
    let Clang::At(clang) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let tools = Binutils::host();
    if !tools.available() {
        eprintln!("uebersprungen: binutils nicht gefunden");
        return;
    }
    let p = corpus(NAME);
    let dir = std::env::temp_dir().join("takt-measured-stack");
    let _ = std::fs::remove_dir_all(&dir);
    let Some(obj) = object(&p, &dir, &clang) else {
        panic!("das Objekt liess sich nicht bauen");
    };
    let Some(symbols) = tools.symbols(&obj) else {
        eprintln!("uebersprungen: `nm` liest dieses Format nicht");
        return;
    };

    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    let step = format!("{machine}_step");
    assert!(
        symbols.iter().any(|s| s.name == step && s.kind == 'T'),
        "`{step}` steht nicht als globales Symbol im Objekt; gefunden: {:?}",
        symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
    );

    let Some(frame) = tools.stack_frame(&obj, &step) else {
        eprintln!("uebersprungen: `objdump` liest dieses Format nicht");
        return;
    };
    eprintln!("{step}: Stackrahmen {frame} B");
    // Ein Rahmen von null hiesse, die Funktion kaeme mit Registern aus —
    // bei einer Schrittfunktion mit Fensterdurchlauf und Automat waere
    // das ein Hinweis darauf, dass die Messung ins Leere lief.
    assert!(frame > 0, "die Schrittfunktion hat keinen messbaren Rahmen; die Messung greift nicht");
}

/// **Der Bericht nimmt die Messung auf** (11.5, 12.3).
///
/// Die beiden Tests darueber belegen, dass sich Flash und Stackrahmen
/// messen lassen. Das genuegte lange nicht: Die Zahlen entstanden und
/// niemand las sie — `stack::depth` hatte ausser einem Test keinen
/// Aufrufer, und `size.rs` setzte den Flash-Posten weiter auf `offen`.
///
/// Dieser Test schliesst die Kette. Er prueft beides, weil beides
/// zusammengehoert: dass die Posten ohne Objekt `offen` bleiben — eine
/// Null, die niemand gemessen hat, waere in einer Summe namens
/// „belastbar" eine Luege — und dass sie mit Objekt `gemessen` werden.
#[test]
fn the_report_takes_the_measurement() {
    let Clang::At(clang) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let tools = Binutils::host();
    if !tools.available() {
        eprintln!("uebersprungen: binutils nicht gefunden");
        return;
    }
    let p = corpus(NAME);

    // Ohne Objekt: beide Posten offen, und die Summe sagt es.
    let plain = size::size(&p);
    for name in ["Flash (Code, Konstanten)", "Stack (Programmanteil)"] {
        let item = plain.items.iter().find(|i| i.name == name).unwrap_or_else(|| panic!("Posten `{name}` fehlt"));
        assert_eq!(item.origin, size::Origin::Open, "`{name}` ohne Objekt");
    }

    let dir = std::env::temp_dir().join("takt-measured-report");
    let _ = std::fs::remove_dir_all(&dir);
    let Some(obj) = object(&p, &dir, &clang) else {
        panic!("das Objekt liess sich nicht bauen");
    };
    let Some(sections) = tools.sections(&obj) else {
        eprintln!("uebersprungen: `size` liest dieses Format nicht");
        return;
    };

    let symbols: Vec<String> = p.fns.iter().map(takt_llvm::fns::symbol).collect();
    let frames: Vec<Option<u32>> =
        tools.stack_frames(&obj, &symbols).into_iter().map(|f| f.and_then(|n| u32::try_from(n).ok())).collect();
    let measured = size::Measured {
        flash: Some(sections.flash()),
        stack: takt_mir::analysis::stack::depth(&p, &frames, &[]),
        iram_text: Some(sections.iram_text),
        iram_rodata: Some(sections.iram_rodata),
    };

    let report = size::size(&p).with_object(&measured);
    let flash = report.items.iter().find(|i| i.name == "Flash (Code, Konstanten)").expect("Flash-Posten");
    assert_eq!(flash.origin, size::Origin::Measured, "mit Objekt ist der Flash gemessen");
    assert!(flash.bytes > 0, "und traegt eine Zahl");
    assert!(report.total() > plain.total(), "die Summe waechst um das Gemessene");
    eprintln!("{NAME}: Flash {} B gemessen, Summe {} B", flash.bytes, report.total());
}

/// Alle Rahmen in einem Durchlauf sind dieselben wie einzeln gelesen.
///
/// [`Binutils::stack_frames`] liest die Disassemblierung einmal statt je
/// Funktion — bei einem Programm mit vielen Funktionen ist das der
/// Unterschied zwischen einem Aufruf und N. Ein schnellerer Weg, der
/// andere Zahlen liefert, waere keiner.
#[test]
fn reading_all_frames_at_once_agrees_with_reading_them_singly() {
    let Clang::At(clang) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let tools = Binutils::host();
    if !tools.available() {
        eprintln!("uebersprungen: binutils nicht gefunden");
        return;
    }
    let p = corpus("02_units_and_data.takt");
    let dir = std::env::temp_dir().join("takt-measured-frames");
    let _ = std::fs::remove_dir_all(&dir);
    let Some(obj) = object(&p, &dir, &clang) else {
        panic!("das Objekt liess sich nicht bauen");
    };

    let symbols: Vec<String> = p.fns.iter().map(takt_llvm::fns::symbol).collect();
    if symbols.is_empty() {
        eprintln!("uebersprungen: das Programm hat keine Funktionen");
        return;
    }
    let batch = tools.stack_frames(&obj, &symbols);
    for (i, sym) in symbols.iter().enumerate() {
        assert_eq!(batch[i], tools.stack_frame(&obj, sym), "`{sym}`: Stapel- und Einzelmessung weichen ab");
    }
    eprintln!("{} Funktionen, Rahmen: {batch:?}", symbols.len());
}
