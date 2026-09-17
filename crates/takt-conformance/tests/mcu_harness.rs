//! Der MCU-Rahmen uebersetzt zusammen mit dem erzeugten Code (M5, 12.1).
//!
//! **Was hier belegt wird.** `mcu_codegen.rs` zeigt, dass der Codegen
//! Objekte fuer die MCU-Ziele erzeugt. Das genuegt nicht: Ein Objekt ohne
//! Aufrufer laeuft nicht. Dieser Test setzt beides zusammen — erzeugten
//! Code und Rahmen — und laesst den Linker urteilen. Was er annimmt, hat
//! alle Symbole; was ihm fehlt, nennt er.
//!
//! Das ist die Stufe vor dem Hardwarelauf: Ein Programm, das linkt,
//! laesst sich flashen.

use takt_llvm::Target;
use takt_llvm::toolchain::Clang;

mod common;

const KORPUS: [&str; 3] = ["01_minimal.takt", "16_timing.takt", "19_faults.takt"];

fn corpus(name: &str) -> takt_mir::Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    takt_sema::compile(&src, &options).program.unwrap_or_else(|| panic!("{path}: uebersetzt nicht"))
}

/// Uebersetzt Programm und Rahmen fuer ein Ziel und linkt beides.
///
/// Der Linker laeuft mit `-r` (teilweise Bindung): Ein vollstaendiges
/// Binary braeuchte Startcode und Linker-Skript, die zum Board gehoeren
/// und nicht hierher. Was `-r` prueft, ist genau die Frage dieses Tests —
/// passen die Symbole zusammen?
fn link_for(clang: &Clang, target: Target, p: &takt_mir::Program, name: &str) -> Result<u64, String> {
    let Clang::At(path) = clang else {
        return Err("clang fehlt".into());
    };
    let dir = std::env::temp_dir().join(format!("takt-mcuh-{}-{}", target.name, name.replace('.', "_")));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let ll = dir.join("programm.ll");
    let c = dir.join("rahmen.c");
    let obj_ir = dir.join("programm.o");
    let obj_c = dir.join("rahmen.o");
    let linked = dir.join("zusammen.o");

    std::fs::write(&ll, common::ir_for(p, target.triple)).map_err(|e| e.to_string())?;
    std::fs::write(&c, takt_conformance::mcu::build(p).source).map_err(|e| e.to_string())?;

    for (src, out) in [(&ll, &obj_ir), (&c, &obj_c)] {
        let mut cmd = std::process::Command::new(path);
        let cmd = Clang::deterministic(&mut cmd)
            .args(["-Wno-override-module", "-O1", "-c", "-ffreestanding", "-nostdlib"])
            .arg(format!("--target={}", target.triple));
        if !target.march.is_empty() {
            cmd.arg(format!("-march={}", target.march));
        }
        let out = cmd.arg(src).arg("-o").arg(out).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("{}: {}", src.display(), String::from_utf8_lossy(&out.stderr)));
        }
    }

    // `-r`: teilweise Bindung, ohne Startcode. Sie beantwortet die Frage
    // dieses Tests — passen die Symbole? — ohne ein Linker-Skript zu
    // verlangen, das zum Board gehoert.
    let mut cmd = std::process::Command::new(path);
    let cmd = Clang::deterministic(&mut cmd).args(["-r", "-nostdlib"]).arg(format!("--target={}", target.triple));
    if !target.march.is_empty() {
        cmd.arg(format!("-march={}", target.march));
    }
    let out = cmd.arg(&obj_ir).arg(&obj_c).arg("-o").arg(&linked).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let size = std::fs::metadata(&linked).map_err(|e| e.to_string())?.len();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(size)
}

/// **Erzeugter Code und Rahmen passen zusammen.**
///
/// Der Linker ist hier der Pruefer: Ein fehlendes Symbol nennt er beim
/// Namen, und genau das ist die Frage — ruft der Rahmen, was der Codegen
/// erzeugt, und stellt er bereit, was der Codegen ruft?
#[test]
fn the_harness_links_with_the_generated_code() {
    let clang = takt_llvm::toolchain::find();
    if matches!(clang, Clang::Missing) {
        eprintln!("clang fehlt; uebersprungen");
        return;
    }
    let mut errors = Vec::new();
    for name in KORPUS {
        let p = corpus(name);
        for target in [Target::THUMBV7EM, Target::RISCV32IMAC] {
            match link_for(&clang, target, &p, name) {
                Ok(size) => eprintln!("{name} fuer {}: {size} Byte gebunden", target.name),
                Err(e) => errors.push(format!("{name} fuer {}: {e}", target.name)),
            }
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n\n"));
}

/// Der Rahmen nennt die Funktionen, die die Tickschleife braucht.
///
/// `takt-rt-baremetal` ruft sie ueber den `Program`-Trait; fehlt eine,
/// faellt es erst beim Binden des Bring-up-Programms auf — also spaet.
#[test]
fn the_harness_exports_what_the_loop_needs() {
    let p = corpus("01_minimal.takt");
    let src = takt_conformance::mcu::build(&p).source;
    for name in ["takt_mcu_init", "takt_mcu_tick", "takt_mcu_dump", "takt_mcu_pc"] {
        assert!(src.contains(&format!("void {name}")), "`{name}` fehlt im Rahmen");
    }
}

/// Der Rahmen bedient jede Funktion, die der erzeugte Code ruft.
///
/// Die ABI-Liste (`takt-llvm/src/abi.rs`) ist die eine Stelle, an der
/// beides zusammensteht; sie war schon einmal unvollstaendig (FB-117).
#[test]
fn the_harness_answers_the_whole_abi() {
    let p = corpus("19_faults.takt");
    let src = takt_conformance::mcu::build(&p).source;
    for name in [
        "takt_now",
        "takt_alert",
        "takt_log",
        "takt_fault",
        "takt_measure",
        "takt_verify",
        "takt_abort",
        "takt_verdict",
    ] {
        assert!(src.contains(name), "`{name}` fehlt im Rahmen — der erzeugte Code ruft es (abi.rs)");
    }
    assert!(src.contains("takt_fn_fault"), "das Fault-Flag reiner Funktionen fehlt (4.1)");
}

/// **Kein `stdio`, kein Heap.**
///
/// 12.3 verlangt beides: Auf der MCU gibt es keine libc, und „gesamter
/// Zustand statisch in `.bss`; kein Heap". Ein Rahmen, der `printf`
/// ruft, linkt dort nicht — und ein `malloc` waere ein Verstoss gegen
/// die Zusage der Sprache, nicht nur gegen eine Konvention.
#[test]
fn the_harness_is_freestanding() {
    let p = corpus("16_timing.takt");
    let src = takt_conformance::mcu::build(&p).source;
    for forbidden in ["stdio.h", "printf", "malloc", "stdlib.h"] {
        assert!(!src.contains(forbidden), "`{forbidden}` gehoert nicht in einen MCU-Rahmen (12.3)");
    }
    assert!(src.contains("unsigned char image"), "das Prozessabbild steht statisch");
}

/// **Der Pfad aus `@ hw(...)` wird zum Namen einer Treiberfunktion.**
///
/// 8.10 verlangt, dass die Abbildung auf Geraete ausserhalb des Programms
/// steht, und nennt den Pfad einen symbolischen Verweis dorthin. Der
/// Rahmen loest ihn nicht auf — er macht daraus einen Aufruf, den das
/// Board bedient. Dass der Name *aus der Adresse* entsteht und nicht aus
/// dem Channel-Namen, ist der Punkt: Zwei Programme, die denselben
/// Ausgang verschieden nennen, passen auf denselben Treiber.
#[test]
fn a_hardware_path_becomes_a_driver_call() {
    let p = corpus("29_heartbeat.takt");
    let src = takt_conformance::mcu::build(&p).source;

    assert!(src.contains("void takt_out_ui_led(unsigned char value);"), "der Treiber ist deklariert:\n{src}");
    assert!(src.contains("takt_out_ui_led(*(unsigned char *)(latch + 0));"), "und wird gerufen:\n{src}");
    assert!(src.contains("void takt_mcu_commit(void)"), "Schritt 10 hat einen Namen (12.1)");
}

/// **Ein Ausgang ohne Hardware-Bindung bekommt keinen Treiberaufruf.**
///
/// `sim(...)` bindet an ein Streckenmodell, nicht an ein Geraet (8.3).
/// Einen Treiber dafuer zu verlangen hiesse, den Sim-Bau unbaubar zu
/// machen — und 8.3 will gerade, dass beide Baeuche dieselbe Logik tragen.
#[test]
fn an_unbound_output_needs_no_driver() {
    let p = corpus("01_minimal.takt");
    let src = takt_conformance::mcu::build(&p).source;
    let calls = src.lines().filter(|l| l.contains("takt_out_")).count();
    let bound = p
        .channels
        .iter()
        .filter(|c| {
            c.dir != takt_mir::program::Direction::Input && matches!(c.binding, takt_mir::program::Binding::Hw(_))
        })
        .count();
    // Je gebundenem Ausgang eine Deklaration, ein schwacher Default (das
    // Board ueberschreibt, was es verdrahtet hat) und ein Aufruf.
    assert_eq!(calls, bound * 3, "nur gebundene Ausgaenge bekommen Treiber:\n{src}");
}

/// **Die Adresse wird zu einem Bezeichner, der in C gueltig ist.**
///
/// Adressen duerfen `/` und `[a:b]` enthalten (8.1); ein Symbolname darf
/// das nicht. Die Abbildung steht in `Address::ident`, damit sie einmal
/// existiert und nicht je Erzeuger neu.
#[test]
fn an_address_becomes_a_c_identifier() {
    use takt_mir::pattern::{Address, AddressSegment};
    assert_eq!(Address::simple("ui/led").ident(), "ui_led");
    assert_eq!(Address::simple("daq1/ai0").ident(), "daq1_ai0");
    let ranged = Address { segments: vec![AddressSegment { name: "tc".into(), range: Some((0, 16)) }] };
    let id = ranged.ident();
    assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'), "gueltig in C: {id}");
}

/// **Jeder ABI-Puffer ist auf acht Byte ausgerichtet.**
///
/// Der erzeugte Code sieht diese Puffer als Strukturen mit `i64`-Feldern
/// und greift darauf mit `LDRD` zu. Ein `unsigned char[]` hat in C aber
/// Ausrichtung 1, und der Linker legt es dahin, wo Platz ist: Auf einem
/// STM32F401 landete `state_blink` so auf 0x2000_0036, und das erste
/// `LDRD` loeste einen UsageFault aus, der zum HardFault eskalierte.
///
/// **Der Wirt kann diesen Fehler nicht finden**, weil x86-64
/// unausgerichtete Zugriffe traegt — darum steht die Pruefung am Text des
/// Rahmens und nicht an einem Lauf. 12.8 trennt die Zielklassen aus genau
/// diesem Grund.
#[test]
fn every_abi_buffer_is_aligned() {
    for name in ["16_timing.takt", "19_faults.takt", "29_heartbeat.takt"] {
        let p = corpus(name);
        let src = takt_conformance::mcu::build(&p).source;
        // Byte-Puffer, die der erzeugte Code als Struktur liest; Felder
        // eines Struct-Typs richtet C von sich aus aus.
        for line in src.lines().filter(|l| l.starts_with("static") && l.contains("unsigned char") && l.contains('[')) {
            assert!(line.contains("_Alignas(8)"), "{name}: unausgerichteter Puffer: {line}");
        }
    }
}

/// **Der MCU-Trace ist mit dem Interpreter vergleichbar** (9.4.4, 13.8).
///
/// Das ist die Voraussetzung des M5-Exits, und sie war lange nicht
/// erfuellt: Der Rahmen schrieb `out led 1` ohne Tickzahl, der
/// Interpreter `t=0 out led false`. Ohne `t=` laesst sich keine Zeile
/// zuordnen — `run::compare` verwirft sie —, und damit haette ein
/// Hardwarelauf *immer* „kein Unterschied" gemeldet, egal was das Board
/// rechnet. Ein Vergleich ohne gemeinsame Zeilen ist kein bestandener
/// Vergleich, sondern gar keiner.
#[test]
fn the_mcu_trace_can_be_compared_with_the_interpreter() {
    let p = corpus("29_heartbeat.takt");
    let src = takt_conformance::mcu::build(&p).source;

    assert!(src.contains(r#"takt_board_trace("t=")"#), "die Zeile beginnt mit der Tickzahl:\n{src}");
    assert!(src.contains("takt_board_trace_i64(g_tick)"), "und zwar mit dem laufenden Tick");

    // Die Gegenprobe am Vergleich selbst: Eine Zeile in der erzeugten
    // Form muss ankommen, eine ohne `t=` nicht.
    let want = "t=0 out led true\n";
    assert!(takt_conformance::run::compare(want, "t=0 out led 1\n").is_empty(), "true gegen 1 ist gleich (9.3)");
    assert!(!takt_conformance::run::compare(want, "t=0 out led 0\n").is_empty(), "true gegen 0 ist ein Unterschied");
}

/// Ein Enum-Ausgang traegt seinen Namen, nicht seine Diskriminante.
///
/// Der Interpreter schreibt den Variantennamen (9.3). `same_number`
/// vergleicht Zahlen und gliche `CLOSED` gegen `0` nicht aus — der
/// Unterschied waere einer der Schreibweise, und der Test faende ihn als
/// Wertunterschied. Der Linux-Rahmen macht es seit je so; der MCU-Rahmen
/// zog nach.
#[test]
fn an_enum_output_carries_its_name() {
    let p = corpus("19_faults.takt");
    let src = takt_conformance::mcu::build(&p).source;
    let has_enum_output = p.channels.iter().any(|c| {
        c.dir != takt_mir::program::Direction::Input
            && matches!(p.types.list.get(c.ty.index()), Some(takt_mir::types::Type::Enum(_)))
    });
    if !has_enum_output {
        eprintln!("kein Enum-Ausgang in diesem Programm; uebersprungen");
        return;
    }
    assert!(src.contains("switch (*"), "ein Enum-Ausgang wird verzweigt, nicht als Zahl geschrieben:\n{src}");
}

/// **Der Rahmen rechnet nicht mit `double`** (12.3, 4.2; FB-143).
///
/// `takt_measure` gab seinen Wert als `(long long)(v * 1000000.0)` aus.
/// Auf einem Kern ohne f64-Hardware bindet das die Software-Emulation ein
/// — `__muldf3`, `__aeabi_d2lz`, `__aeabi_dmul`, zusammen 784 Byte in
/// einem Binary von 4866. Fuer eine Funktion, die das Programm nie rief:
/// Der Linker kann sie nicht entfernen, weil der Rahmen sie exportiert.
///
/// Die Bits kosten nichts und sagen mehr. 4.2 verlangt bitgleiche
/// Ergebnisse ueber alle Targets, und `same_number` vergleicht
/// Fliesskomma ohnehin bitweise (9.4.4) — eine Multiplikation waere eine
/// zweite Rundungsquelle unmittelbar vor diesem Vergleich.
#[test]
fn the_harness_does_no_floating_point_arithmetic() {
    for name in ["19_faults.takt", "29_heartbeat.takt"] {
        let p = corpus(name);
        let src = takt_conformance::mcu::build(&p).source;
        for line in src.lines() {
            let computation = line.contains(" * ") || line.contains(" / ") || line.contains(" + ");
            assert!(
                !(computation && (line.contains("double") || line.contains(".0"))),
                "{name}: Fliesskomma-Rechnung im Rahmen: {line}"
            );
        }
        assert!(src.contains("__builtin_memcpy(&bits, &v"), "{name}: `measure` gibt die Bits heraus");
    }
}

/// Jede Beobachtungszeile traegt ihren Tick.
///
/// `grammar/trace.md` gibt `t=<tick> <art> …` vor. Der Rahmen schrieb
/// `alert 0 3` ohne Tick — dieselbe Luecke wie bei den Ausgaengen, nur
/// eine Zeile weiter, und aus demselben Grund unentdeckt: Verglichen
/// wurden bisher nur `out`-Zeilen.
#[test]
fn every_observation_line_carries_its_tick() {
    let p = corpus("19_faults.takt");
    let src = takt_conformance::mcu::build(&p).source;
    for kind in ["alert", "log", "abort", "verify", "verdict", "measure"] {
        let at = src.find(&format!("takt_board_trace(\"{kind} \")")).unwrap_or_else(|| {
            panic!("`{kind}` fehlt im Rahmen");
        });
        // Die beiden Zeilen davor muessen den Tick schreiben.
        let davor = &src[at.saturating_sub(120)..at];
        assert!(davor.contains("takt_board_trace_i64(g_tick)"), "`{kind}` ohne Tickzahl:\n{davor}");
    }
}

/// **Der Rahmen beantwortet die Schlafbedingung** (9.9).
///
/// 9.9 nennt sechs Konjunkte. Vier liefert der erzeugte Code je Maschine,
/// die anderen beiden — geplante Ausgaben und Jobs — kennt der MCU-Rahmen
/// nicht; sie sind dort trivial wahr.
#[test]
fn the_harness_answers_the_sleep_condition() {
    let p = corpus("29_heartbeat.takt");
    let src = takt_conformance::mcu::build(&p).source;
    assert!(src.contains("_Bool takt_mcu_idle(void)"), "die Bedingung hat einen Namen:\n{src}");
    assert!(src.contains("long long takt_mcu_deadline(void)"), "und die Frist auch");
    // Alle Maschinen muessen zustimmen: ein `return 0` je Maschine.
    assert!(src.contains("if (!heartbeat_idle(state_heartbeat)) return 0;"), "je Maschine eine Abfrage:\n{src}");
}

/// Ohne `idle`-Zustand schlaeft eine Maschine nie.
///
/// Der Codegen schreibt dann `ret i1 0`, und der Optimierer entfernt den
/// Aufruf — ein Programm, das nicht schlafen kann, zahlt nichts dafuer.
#[test]
fn a_machine_without_idle_never_sleeps() {
    let p = corpus("29_heartbeat.takt");
    let ir = common::ir_for(&p, Target::THUMBV7EM.triple);
    let at = ir.find("define i1 @heartbeat_idle").expect("die Funktion wird erzeugt");
    let body = &ir[at..at + 120];
    assert!(body.contains("ret i1 0"), "ohne `idle` konstant falsch:\n{body}");
}

/// **Ein `idle`-Zustand schlaeft, und die Frist stimmt** (9.9).
///
/// Die beiden Funktionen zusammen sind die Antwort auf `sleep_allowed`
/// und `next_deadline`: Ist das aktive Blatt `idle` und kein Fault
/// vorgemerkt, darf die Schleife bis zur `after`-Frist ueberspringen.
#[test]
fn an_idle_state_reports_its_deadline() {
    let p = corpus("30_idle.takt");
    let ir = common::ir_for(&p, Target::THUMBV7EM.triple);

    let at = ir.find("define i1 @m_idle").expect("Schlafabfrage");
    let idle = &ir[at..ir[at..].find("\n}").map_or(ir.len(), |e| at + e)];
    assert!(idle.contains("icmp eq i8"), "das aktive Blatt wird geprueft:\n{idle}");
    assert!(idle.contains("xor i1"), "und `pending` negiert");

    let at = ir.find("define i64 @m_deadline").expect("Fristabfrage");
    let dl = &ir[at..ir[at..].find("\n}").map_or(ir.len(), |e| at + e)];
    // `after 500 ms` bei 10 ms Tick sind 50 Ticks, `after 200 ms` 20.
    assert!(dl.contains("sub i64 50,"), "die Frist steht in Ticks:\n{dl}");
    assert!(dl.contains("sub i64 20,"), "je Blatt die eigene");
}

/// **Die Frist ist ein absoluter Zeitpunkt in Nanosekunden** (9.9).
///
/// `Program::next_deadline` verlangt es so; die Maschinen rechnen aber in
/// Ticks, weil `t_in_state` sie zaehlt. Die Umrechnung stand zuerst
/// nirgends — `Runtime::sleep` bekam Ticks, zog `now` in Nanosekunden ab
/// und erhielt eine tief negative Zahl. Ergebnis: Es wurde nie
/// geschlafen, ohne Fehler und ohne Meldung.
#[test]
fn the_deadline_is_absolute_nanoseconds() {
    let p = corpus("30_idle.takt");
    let src = takt_conformance::mcu::build(&p).source;
    assert!(
        src.contains("return takt_now() + best * 10000000LL;"),
        "Ticks mal Periode, auf `takt_now` bezogen:\n{src}"
    );
    assert!(src.contains("if (best < 0) return -1;"), "keine Frist bleibt keine Frist");
}

/// **Multirate: die Frist rechnet in Aktivierungen, nicht in Basis-Ticks.**
///
/// `after` vergleicht `t_in_state * period * T0` gegen die Dauer (7.2);
/// `t_in_state` zaehlt also Aktivierungen. Eine erste Fassung teilte nur
/// durch `T0` und war bei `period = 5` fuenfmal zu gross. Zurueck kommt
/// die Zahl in Basis-Ticks, weil die Runtime darin springt.
#[test]
fn a_multirate_deadline_counts_activations() {
    let p = corpus("31_idle_multirate.takt");
    let ir = common::ir_for(&p, Target::THUMBV7EM.triple);
    let at = ir.find("define i64 @m_deadline").expect("Fristabfrage");
    let dl = &ir[at..ir[at..].find("\n}").map_or(ir.len(), |e| at + e)];
    // `every 50 ms` bei 10 ms Tick: Periode 5. `after 500 ms` sind zehn
    // Aktivierungen, `after 200 ms` vier.
    assert!(dl.contains("sub i64 10,"), "500 ms sind zehn Aktivierungen:\n{dl}");
    assert!(dl.contains("sub i64 4,"), "200 ms sind vier");
    assert!(dl.contains("mul i64"), "und das Ergebnis geht in Basis-Ticks zurueck");
}
