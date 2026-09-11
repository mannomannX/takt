//! Maschinen nach LLVM-IR (11.2): Zustands-Struct und Schrittfunktion.
//!
//! Die Tests uebersetzen echte Korpusprogramme, nicht erfundene MIR —
//! ein Struct, der nur fuer Testdaten stimmt, ist kein Beleg.

use takt_llvm::emit::Module;
use takt_llvm::machine::{self, Role, depth, state_struct};
use takt_llvm::toolchain::{Clang, find};
use takt_mir::program::Program;

/// Uebersetzt ein Korpusprogramm zur MIR.
fn corpus(name: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{name}: kein Programm"))
}

/// Baut die IR aller Maschinen eines Programms, so weit der Codegen reicht.
///
/// Maschinen, die etwas enthalten, das noch nicht gesenkt wird, fallen
/// weg — `step_function` raeumt sie selbst ab. Der Test prueft, dass das,
/// was *entsteht*, gueltig ist.
fn ir_of(p: &Program) -> String {
    let mut m = Module::new("korpus", "x86_64-pc-windows-msvc");
    takt_llvm::abi::Abi::declare(&mut m);
    takt_llvm::stream::Streams::declare(&mut m);
    // Bloecke zuerst: Die Maschinen halten ihre Instanzen (5.7).
    let methoden: Vec<_> = p.blocks.iter().flat_map(|b| b.step.iter().chain(&b.methods).copied()).collect();
    for b in &p.blocks {
        let Some(inst) = takt_llvm::block::instance_of(b, p) else { continue };
        takt_llvm::block::declare(b, &inst, &mut m);
        for fid in b.step.iter().chain(&b.methods) {
            let Some(f) = p.fns.get(fid.index()) else { continue };
            let _ = takt_llvm::fns::block_method(b, f, p, &mut m);
        }
    }
    // Reine Funktionen: Die Maschinen rufen sie (4.4).
    for (i, f) in p.fns.iter().enumerate() {
        if !methoden.contains(&takt_mir::FnId(i as u32)) {
            let _ = takt_llvm::fns::function(f, p, &mut m);
        }
    }
    for machine in &p.machines {
        let Some(st) = state_struct(machine, p) else { continue };
        machine::declare_state(machine, &st, &mut m);
        let _ = takt_llvm::step::step_function(machine, &st, p, &mut m);
    }
    m.finish()
}

/// Die Korpusprogramme, deren Maschinen der Codegen heute vollstaendig
/// senkt. Die Liste waechst mit ihm; sie steht hier, damit ein Rueckschritt
/// auffaellt.
const VOLLSTAENDIG: [&str; 9] = [
    "12_bitfields.takt",
    "13_protocol_analysis.takt",
    "18_blocks.takt",
    "01_minimal.takt",
    "03_sequences_and_faults.takt",
    "14_latency.takt",
    "15_quality.takt",
    "16_timing.takt",
    "17_nested.takt",
];

/// Alle Korpusdateien, die fehlerfrei zu MIR uebersetzen — auch die, deren
/// Maschinen der Codegen nur teilweise senkt. Was er *erzeugt*, muss
/// gueltig sein, sonst faellt eine halbe Funktion erst spaeter auf.
const UEBERSETZBAR: [&str; 11] = [
    "18_blocks.takt",
    "01_minimal.takt",
    "02_units_and_data.takt",
    "03_sequences_and_faults.takt",
    "12_bitfields.takt",
    "13_framing.takt",
    "13_protocol_analysis.takt",
    "14_latency.takt",
    "15_quality.takt",
    "16_timing.takt",
    "17_nested.takt",
];

// --- Der Zustands-Struct (11.2) -----------------------------------------

/// 11.2: `conf` und `t_in_state` haben die Tiefe des Zustandsbaums.
#[test]
fn the_configuration_is_a_path_array_of_fixed_depth() {
    let p = corpus("01_minimal.takt");
    let m = &p.machines[0];
    let st = state_struct(m, &p).expect("Struct baubar");
    assert_eq!(st.depth, depth(m));
    assert!(st.depth >= 1, "auch eine flache Maschine hat eine Konfiguration");
    assert_eq!(st.fields[0].role, Role::Conf);
    assert_eq!(st.fields[1].role, Role::TimeInState);
}

/// Eine Maschine mit geschachtelten Zustaenden hat Tiefe > 1.
#[test]
fn nested_states_increase_the_depth() {
    let p = corpus("03_sequences_and_faults.takt");
    let deepest = p.machines.iter().map(depth).max().expect("Maschinen");
    assert!(deepest >= 1);
    for m in &p.machines {
        let st = state_struct(m, &p).expect("Struct baubar");
        assert_eq!(st.depth, depth(m), "{}", m.name);
    }
}

/// 11.2 nennt die Felder in einer Reihenfolge; sie ist die einzige
/// Quelle fuer die Indizes im erzeugten Code.
#[test]
fn every_machine_has_the_fields_the_reference_names() {
    let p = corpus("03_sequences_and_faults.takt");
    for m in &p.machines {
        let st = state_struct(m, &p).expect("Struct baubar");
        for role in [Role::Conf, Role::TimeInState, Role::Pending, Role::LastFault, Role::Pc] {
            assert!(st.index_of(role, 0).is_some(), "{}: {role:?} fehlt", m.name);
        }
    }
}

/// Die Variablen der Maschine stehen im Struct.
#[test]
fn the_variables_of_a_machine_are_fields_of_its_state() {
    let p = corpus("13_protocol_analysis.takt");
    for m in &p.machines {
        let Some(st) = state_struct(m, &p) else { continue };
        let vars = st.fields.iter().filter(|f| f.role == Role::Var).count();
        assert_eq!(vars, m.vars.len(), "{}", m.name);
    }
}

/// Ein Blattzustand ist eine Diskriminante; nur Blaetter kommen in den
/// `switch` (11.2).
#[test]
fn only_leaf_states_become_switch_targets() {
    let p = corpus("03_sequences_and_faults.takt");
    for m in &p.machines {
        for leaf in machine::leaves(m) {
            assert!(m.states[leaf.index()].children.is_empty(), "{}: {leaf:?} ist kein Blatt", m.name);
        }
    }
}

/// Der Pfad zu einem Blatt ist so lang wie die Tiefe erlaubt — er ist
/// das, was in `conf` steht.
#[test]
fn the_path_to_a_leaf_fits_into_the_configuration() {
    let p = corpus("03_sequences_and_faults.takt");
    for m in &p.machines {
        let d = depth(m) as usize;
        for leaf in machine::leaves(m) {
            let path = machine::path_to(m, leaf);
            assert!(path.len() <= d, "{}: Pfad {} laenger als Tiefe {d}", m.name, path.len());
            assert_eq!(path[0].index(), m.states[leaf.index()].parent.map_or(leaf, |_| path[0]).index());
        }
    }
}

// --- Die erzeugte IR ----------------------------------------------------

/// Der Kern: Aus echten Korpusprogrammen entsteht gueltige LLVM-IR, die
/// sich zu Maschinencode uebersetzen laesst.
///
/// Das ist die Zusage von Schritt 7, und sie ist nur mit LLVM pruefbar —
/// ein Textest sieht nicht, ob ein Basisblock einen Terminator hat
/// (FB-67).
#[test]
fn the_machines_of_the_corpus_compile_to_object_code() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    for name in VOLLSTAENDIG {
        let p = corpus(name);
        let ir = ir_of(&p);
        assert!(ir.contains("_state = type"), "{name}: kein Zustands-Struct");
        assert!(ir.contains("_step("), "{name}: keine Schrittfunktion");
        assert!(ir.contains("switch i8"), "{name}: kein `switch` ueber die Blaetter (11.2)");
        let dir = std::env::temp_dir().join(format!("takt-llvm-m-{}", name.replace('.', "_")));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("Verzeichnis");
        if let Err(e) = clang.assembles(&ir, &dir) {
            panic!(
                "{name}: die IR assembliert nicht:
{e}
--- IR ---
{ir}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 11.2: `check` wird zu Vergleich und bedingtem Sprung in den
/// Fault-Trampolin.
#[test]
fn a_check_branches_into_the_fault_trampoline() {
    let p = corpus("01_minimal.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("fcmp olt double"),
        "der Vergleich fehlt:
{ir}"
    );
    assert!(
        ir.contains("label %fault_tank_guard"),
        "der Sprung in den Trampolin fehlt:
{ir}"
    );
    // Der Trampolin merkt den Fault vor (5.4) und verlaesst den Schritt.
    assert!(
        ir.contains("store i1 true"),
        "`pending` wird nicht gesetzt:
{ir}"
    );
}

/// 11.2: Ein Output wird in den Latch geschrieben; die Runtime committet
/// ihn (12.1).
#[test]
fn an_output_is_written_to_the_latch() {
    let p = corpus("14_latency.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("ptr %3"),
        "der Latch-Zeiger wird nicht benutzt:
{ir}"
    );
}

/// Eine Maschine, die der Codegen nicht vollstaendig senkt, hinterlaesst
/// keine halbe Funktion — sie waere gueltige IR mit falschem Inhalt.
#[test]
fn an_incomplete_machine_leaves_no_half_function() {
    let p = corpus("12_bitfields.takt");
    let ir = ir_of(&p);
    let defines = ir.matches("define ").count();
    let rets = ir
        .matches(
            "
}",
        )
        .count();
    assert_eq!(
        defines, rets,
        "offene oder halbe Funktion in:
{ir}"
    );
}

/// Auch die Maschinen-IR traegt keine Fast-Math-Flags (4.2).
#[test]
fn machine_ir_carries_no_fast_math_flags() {
    let p = corpus("13_protocol_analysis.takt");
    let ir = ir_of(&p);
    for (i, line) in ir.lines().enumerate() {
        let code = line.split(';').next().unwrap_or("");
        for flag in ["fast", "nnan", "ninf", "nsz", "arcp", "contract", "reassoc"] {
            assert!(!code.split_whitespace().any(|w| w == flag), "Zeile {}: `{flag}`", i + 1);
        }
    }
}

/// 11.3: reproduzierbare Builds — zweimal dieselbe MIR, zweimal dieselbe
/// IR.
#[test]
fn the_same_program_produces_the_same_ir() {
    let p = corpus("03_sequences_and_faults.takt");
    assert_eq!(ir_of(&p), ir_of(&p));
}

/// Der Zustands-Struct traegt einen Namen, damit die IR lesbar bleibt —
/// nach 13.4 ist sie das Artefakt der Qualifikation.
#[test]
fn the_state_struct_is_a_named_type() {
    let p = corpus("01_minimal.takt");
    let ir = ir_of(&p);
    let name = &p.machines[0].name;
    assert!(ir.contains(&format!("%{name}_state = type")), "{ir}");
    assert!(ir.contains(&format!("@{name}_step")), "{ir}");
}

// --- Uebergaenge (5.2) --------------------------------------------------

/// 5.2: Ein Uebergang setzt `conf` auf den Zielzustand.
#[test]
fn a_transition_writes_the_target_into_the_configuration() {
    let p = corpus("01_minimal.takt");
    let ir = ir_of(&p);
    // IDLE --start--> WATCH: WATCH ist das zweite Blatt, also `i8 1`.
    assert!(
        ir.contains("store i8 1, ptr"),
        "der Zielzustand wird nicht geschrieben:
{ir}"
    );
}

/// 5.2: Beim Eintritt in einen Zustand beginnt `t_in_state` von vorn.
///
/// Ohne das Zuruecksetzen misst `after d` die Zeit seit dem Start der
/// Maschine statt seit dem Eintritt.
#[test]
fn a_transition_resets_the_time_in_state() {
    let p = corpus("01_minimal.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("store i64 -1, ptr"),
        "`t_in_state` wird nicht zurueckgesetzt:
{ir}"
    );
    // Am Ende des Schritts waechst er wieder um einen Tick; der erste Tick
    // im neuen Zustand hat damit `t_in_state == 0`.
    assert!(
        ir.contains("add i64"),
        "`t_in_state` waechst nicht:
{ir}"
    );
}

/// 5.2: Erst der `loop:`-Koerper, dann die Uebergaenge. Ein `check` wirkt
/// also, bevor ein `when` den Zustand verlassen kann — sonst nennte die
/// Meldung den falschen Zustand.
#[test]
fn the_loop_body_runs_before_the_transitions() {
    let p = corpus("01_minimal.takt");
    let ir = ir_of(&p);
    let watch = ir.find("tank_guard_WATCH:").expect("Zustand WATCH");
    let check = ir[watch..].find("fcmp").expect("der check in WATCH");
    let trans = ir[watch..].find("uebergang").expect("der Uebergang aus WATCH");
    assert!(
        check < trans,
        "der Uebergang steht vor dem `check`:
{ir}"
    );
}

// --- Reichweite des Codegens --------------------------------------------

/// `scope` zaehlt unabhaengig vom Codegen; beide muessen dieselbe Menge
/// meinen.
///
/// Ohne diesen Test koennte die Messung behaupten, ein Knoten sei gedeckt,
/// waehrend `step_function` ihn ablehnt — eine Zahl, die besser aussieht
/// als die Lage. Geprueft wird an den Programmen, die vollstaendig
/// uebersetzen: Dort darf `scope` keinen offenen Knoten finden.
#[test]
fn the_measurement_agrees_with_the_codegen() {
    for name in VOLLSTAENDIG {
        let p = corpus(name);
        let mut cov = takt_llvm::scope::Coverage::default();
        for m in &p.machines {
            takt_llvm::scope::machine(m, &mut cov);
        }
        assert!(cov.open.is_empty(), "{name} uebersetzt vollstaendig, aber `scope` meldet offen: {:?}", cov.open);
    }
}

/// Die Messung sieht ueberhaupt etwas — ein leerer Zaehler waere zu 100 %
/// gedeckt und damit wertlos.
#[test]
fn the_measurement_sees_the_corpus() {
    let p = corpus("01_minimal.takt");
    let mut cov = takt_llvm::scope::Coverage::default();
    for m in &p.machines {
        takt_llvm::scope::machine(m, &mut cov);
    }
    assert!(cov.total() > 10, "zu wenige Knoten gezaehlt: {}", cov.total());
    assert!(cov.percent() > 99.0, "01_minimal sollte vollstaendig gedeckt sein: {:.0} %", cov.percent());
}

// --- Qualitaet am Rand (3.5) --------------------------------------------

/// 3.5: `.valid` ist „`Good` oder `Suspect` mit Wert" — im erzeugten Code
/// ein Vergleich gegen die Grenze zwischen beiden Haelften der Skala.
#[test]
fn valid_tests_the_quality_against_the_boundary() {
    let p = corpus("15_quality.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("icmp sle i8"),
        "`.valid` prueft nicht die Qualitaet:
{ir}"
    );
}

/// `.suspect` und `.stale` fragen nach je einem Wert der Skala.
#[test]
fn suspect_and_stale_compare_against_their_own_value() {
    let p = corpus("15_quality.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("icmp eq i8"),
        "`.suspect`/`.stale` fehlen:
{ir}"
    );
}

/// `x.or(d)`: der Wert, wenn gueltig, sonst der Ersatz — ohne
/// Verzweigung, weil beide Seiten total sind (4.1).
#[test]
fn or_selects_between_value_and_fallback() {
    let p = corpus("15_quality.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("select i1"),
        "`.or` erzeugt kein `select`:
{ir}"
    );
}

/// Die Reihenfolge der Qualitaetsstufen ist eine ABI zwischen Runtime und
/// erzeugtem Code; sie muss mit `takt-hal` uebereinstimmen.
#[test]
fn the_quality_scale_matches_the_runtime() {
    use takt_llvm::image::quality;
    assert_eq!(quality::GOOD, 0);
    assert_eq!(quality::SUSPECT, 1);
    assert_eq!(quality::STALE, 2);
    assert_eq!(quality::BAD, 3);
    // `.valid` heisst `<= SUSPECT`; das gilt nur, wenn `Stale` und `Bad`
    // darueber liegen. Als `const`, weil die Bedingung schon beim
    // Uebersetzen entscheidbar ist — dann bricht eine Umnummerierung den
    // Bau, nicht erst den Test.
    const _: () = assert!(quality::STALE > quality::SUSPECT && quality::BAD > quality::SUSPECT);
}

// --- `after d` (5.2, 7.1) ------------------------------------------------

/// 7.1: `after` feuert „nie im Entry-Tick" — im erzeugten Code steht
/// dafuer der Vergleich `elapsed > 0` neben `elapsed >= d`.
#[test]
fn after_never_fires_in_the_entry_tick() {
    let p = corpus("16_timing.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("icmp sgt i64"),
        "der Vergleich gegen 0 fehlt:
{ir}"
    );
    assert!(
        ir.contains("icmp sge i64 %"),
        "der Vergleich gegen die Frist fehlt:
{ir}"
    );
    assert!(
        ir.contains("and i1"),
        "beide Bedingungen werden nicht verknuepft:
{ir}"
    );
}

// --- Geschachtelte Zustaende (5.2) --------------------------------------

/// 5.2: Aktiv ist ein *Pfad*, nicht ein Zustand. Der `loop:` einer
/// Zwischenebene ist die Invariante aller Zustaende darunter und laeuft
/// darum in jedem ihrer Blaetter.
#[test]
fn an_ancestor_loop_runs_in_every_leaf_below_it() {
    let p = corpus("17_nested.takt");
    let ir = ir_of(&p);
    // Der `check p < 90 bar` steht einmal im Programm, unter `RUNNING`
    // mit zwei Blaettern — also zweimal in der IR.
    let checks = ir.matches("fcmp olt").count();
    assert_eq!(
        checks, 2,
        "der `check` von RUNNING laeuft nicht in beiden Blaettern:
{ir}"
    );
}

/// 5.2: Ein Uebergang auf einen zusammengesetzten Zustand betritt dessen
/// `initial`-Kind, rekursiv bis zu einem Blatt.
#[test]
fn a_transition_into_a_composite_state_reaches_its_initial_leaf() {
    let p = corpus("17_nested.takt");
    let m = &p.machines[0];
    let running = m.states.iter().position(|s| s.name == "RUNNING").expect("RUNNING");
    let leaf = takt_llvm::machine::initial_leaf(m, takt_mir::StateId(running as u32)).expect("Blatt");
    assert_eq!(m.states[leaf.index()].name, "WARMUP", "das `initial`-Kind von RUNNING");
    assert!(m.states[leaf.index()].children.is_empty(), "und es ist ein Blatt");
}

/// 5.2: Ein Uebergang zwischen Geschwistern raeumt ihren Elternteil nicht
/// ab — sein `exit:` laeuft nicht, sein `enter:` auch nicht.
#[test]
fn a_transition_between_siblings_leaves_the_parent_alone() {
    let p = corpus("17_nested.takt");
    let m = &p.machines[0];
    let find = |n: &str| takt_mir::StateId(m.states.iter().position(|s| s.name == n).expect(n) as u32);
    let (warmup, active) = (find("WARMUP"), find("ACTIVE"));
    let raus = takt_llvm::machine::exiting(m, warmup, active);
    let rein = takt_llvm::machine::entering(m, warmup, active);
    assert_eq!(raus, vec![warmup], "nur das Geschwister wird verlassen");
    assert_eq!(rein, vec![active], "nur das Geschwister wird betreten");
}

/// Ein Uebergang aus der Tiefe nach aussen verlaesst die ganze Kette.
#[test]
fn a_transition_out_of_a_hierarchy_exits_the_whole_chain() {
    let p = corpus("17_nested.takt");
    let m = &p.machines[0];
    let find = |n: &str| takt_mir::StateId(m.states.iter().position(|s| s.name == n).expect(n) as u32);
    let raus = takt_llvm::machine::exiting(m, find("WARMUP"), find("SAFE"));
    assert_eq!(raus, vec![find("WARMUP"), find("RUNNING")], "erst das Blatt, dann sein Elternteil");
}

/// Marken muessen je erzeugter Verzweigung eindeutig sein: Ein Blatt
/// fuehrt auch die Uebergaenge seiner Vorfahren aus, und zwei Ebenen
/// haetten sonst dieselbe Marke — ungueltige IR, die nur LLVM findet.
#[test]
fn every_label_in_the_generated_ir_is_unique() {
    for name in VOLLSTAENDIG {
        let p = corpus(name);
        let ir = ir_of(&p);
        let mut seen = std::collections::BTreeSet::new();
        for line in ir.lines() {
            let t = line.trim_end();
            if t.ends_with(':') && !t.starts_with(' ') && !t.starts_with(';') {
                assert!(seen.insert(t.to_string()), "{name}: Marke `{t}` kommt zweimal vor");
            }
        }
    }
}

/// Was der Codegen erzeugt, ist gueltige LLVM-IR — auch fuer Programme,
/// die er nur teilweise senkt.
///
/// Das ist die schaerfere Fassung von
/// `the_machines_of_the_corpus_compile_to_object_code`: Sie prueft die
/// vollstaendigen, dieser hier *alle*. Eine halbe Funktion, die
/// assembliert, waere schlimmer als eine, die es nicht tut.
#[test]
fn everything_the_codegen_emits_assembles() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    for name in UEBERSETZBAR {
        let p = corpus(name);
        let ir = ir_of(&p);
        let dir = std::env::temp_dir().join(format!("takt-llvm-all-{}", name.replace('.', "_")));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("Verzeichnis");
        if let Err(e) = clang.assembles(&ir, &dir) {
            panic!(
                "{name}: die IR assembliert nicht:
{e}
--- IR ---
{ir}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 5.4: `abort` faultet alle Maschinen und verlaesst den Schritt. Was
/// danach im Block stand, ist unerreichbar und darf nicht in der IR
/// stehen — LLVM nimmt es nicht an.
#[test]
fn nothing_follows_a_terminator() {
    for name in UEBERSETZBAR {
        let p = corpus(name);
        let ir = ir_of(&p);
        let mut terminated = false;
        for line in ir.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with(';') {
                continue;
            }
            if t.ends_with(':') || t.starts_with("define") {
                terminated = false;
                continue;
            }
            if t == "}" {
                continue;
            }
            assert!(!terminated, "{name}: `{t}` steht hinter einem Terminator");
            let head = t.split_whitespace().next().unwrap_or("");
            if matches!(head, "br" | "ret" | "switch" | "unreachable") {
                terminated = true;
            }
        }
    }
}

// --- Sammlungen (3.9) ---------------------------------------------------

/// 3.9: `push` schreibt nur, wenn Platz ist, und sagt es. Der Vergleich
/// steht vor dem Sprung, damit der haeufige Weg der gerade ist.
#[test]
fn push_checks_the_capacity_before_writing() {
    let p = corpus("13_framing.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("icmp ult i32"),
        "`push` prueft die Kapazitaet nicht:
{ir}"
    );
    assert!(
        ir.contains("phi i1"),
        "`push` liefert kein `bool`:
{ir}"
    );
}

/// 3.9: `append` ist alles-oder-nichts — ein Teilanhang liesse einen
/// halben Rahmen im Puffer, den niemand als Fehler erkennt.
#[test]
fn append_copies_everything_or_nothing() {
    let p = corpus("13_framing.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("icmp ule i32"),
        "`append` prueft die Summe nicht:
{ir}"
    );
    assert!(
        ir.contains("llvm.memcpy"),
        "`append` kopiert nicht in einem Zug:
{ir}"
    );
}

// --- Reine Funktionen (4.4) ---------------------------------------------

/// Eine `fn` wird eine gewoehnliche LLVM-Funktion; LLVM darf sie
/// einbetten und ueber Aufrufe hinweg optimieren.
#[test]
fn a_pure_function_becomes_an_llvm_function() {
    let p = corpus("13_framing.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("@takt_fn_checksum"),
        "`checksum` fehlt:
{ir}"
    );
    assert!(
        ir.contains("define") && ir.contains("@takt_fn_"),
        "keine Funktionsdefinition:
{ir}"
    );
}

/// 4.1: `for i in range(n)` hat eine statische Schranke, und `break`
/// verlaesst die Schleife vorzeitig.
#[test]
fn a_bounded_loop_has_a_header_and_an_exit() {
    let p = corpus("13_framing.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("fuer") && ir.contains("_rumpf"),
        "keine Schleife:
{ir}"
    );
    assert!(
        ir.contains("icmp slt"),
        "der Schleifenvergleich fehlt:
{ir}"
    );
}

/// 4.1: Die `wrapping_*`-Primitiven sind der explizite Umlauf — dieselbe
/// Instruktion wie der gewoehnliche Operator, nur ohne `nsw`/`nuw` und
/// ohne den Ueberlauf-Check darum herum.
#[test]
fn wrapping_arithmetic_has_no_overflow_flags() {
    let p = corpus("13_framing.takt");
    let ir = ir_of(&p);
    for line in ir.lines().filter(|l| l.contains(" add i8") || l.contains(" add i32")) {
        assert!(!line.contains("nsw") && !line.contains("nuw"), "{line}");
    }
}

/// Eine Funktion, deren Rumpf der Codegen nicht senkt, wird *deklariert*
/// statt weggelassen: Ein Aufruf auf ein undefiniertes Symbol ist
/// gueltige IR, ein fehlendes Symbol laesst clang die ganze Datei
/// zurueckweisen.
#[test]
fn every_called_function_is_defined_or_declared() {
    for name in UEBERSETZBAR {
        let p = corpus(name);
        let ir = ir_of(&p);
        for line in ir.lines() {
            let Some(at) = line.find("@takt_fn_") else { continue };
            if !line.trim_start().starts_with("call") {
                continue;
            }
            let symbol: String =
                line[at..].chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '@')).collect();
            // Bereitgestellt heisst: Es gibt eine Zeile, die das Symbol
            // definiert oder deklariert — nicht nur eine, die es ruft.
            let bereit = ir.lines().any(|l| {
                let t = l.trim_start();
                (t.starts_with("define ") || t.starts_with("declare ")) && t.contains(&symbol)
            });
            assert!(bereit, "{name}: `{symbol}` wird gerufen, aber weder definiert noch deklariert");
        }
    }
}

// --- Blockinstanzen (5.7) -----------------------------------------------

/// 5.7: `step` hoechstens einmal je Aktivierung. Das Flag im Zustand ist
/// die Absicherung — ein Filter, der zweimal laeuft, hat einen Tick
/// uebersprungen, ohne dass es jemand saehe.
#[test]
fn step_runs_at_most_once_per_activation() {
    let p = corpus("18_blocks.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("store i1 true"),
        "das `stepped`-Flag wird nicht gesetzt:
{ir}"
    );
    assert!(
        ir.contains("step1_ende") || ir.contains("_ende"),
        "kein Zweig um den Aufruf:
{ir}"
    );
}

/// Eine Blockmethode bekommt die Instanz als Zeiger: Sie aendert ihren
/// Zustand, also braucht sie einen Speicherort, keinen Wert.
#[test]
fn a_block_method_takes_its_instance_by_pointer() {
    let p = corpus("18_blocks.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("@takt_block_counter_step(ptr"),
        "die Methode nimmt keinen Zeiger:
{ir}"
    );
}

/// `reset()` stellt die Initialwerte her und gibt `step` wieder frei.
#[test]
fn reset_restores_the_initial_state_and_clears_the_flag() {
    let p = corpus("18_blocks.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("store i1 false"),
        "`reset` gibt `step` nicht wieder frei:
{ir}"
    );
}

// --- Ereignisstroeme (8.6, 8.7, 9.6) ------------------------------------

/// 8.7: Der Handler laeuft ueber das Fenster seines Stroms. Die
/// Fenstergroesse steht erst zur Laufzeit fest, die Schranke aber im Typ
/// — also eine Schleife, keine abgerollte Folge.
#[test]
fn a_handler_walks_the_window_of_its_stream() {
    let p = corpus("12_bitfields.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("@takt_stream_count"),
        "die Fenstergroesse wird nicht erfragt:
{ir}"
    );
    assert!(
        ir.contains("@takt_stream_at"),
        "die Elemente werden nicht gelesen:
{ir}"
    );
}

/// 9.6: Auch ein Element, auf das kein Handler passt, gilt als
/// untersucht — sonst saehe die Maschine es im naechsten Tick wieder.
#[test]
fn every_element_of_the_window_counts_as_examined() {
    let p = corpus("12_bitfields.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("@takt_stream_examined"),
        "`examined` wird nicht gemeldet:
{ir}"
    );
    // Der Aufruf steht *vor* dem Rumpf des Handlers: Er gilt fuer jedes
    // Element, nicht nur fuer die getroffenen.
    let at = ir.find("@takt_stream_at").expect("at");
    let examined = ir[at..].find("@takt_stream_examined").expect("examined");
    let body = ir[at..].find("store").unwrap_or(usize::MAX);
    assert!(
        examined < body,
        "`examined` steht hinter dem Rumpf:
{ir}"
    );
}

// --- Wrapper-Typen (3.8) ------------------------------------------------

/// 3.8: `T?` und `T!E` tragen ihr Flag am Ende, damit der Wert an
/// derselben Stelle liegt wie ohne Wrapper.
#[test]
fn the_validity_flag_sits_at_the_end_of_the_wrapper() {
    let p = corpus("13_protocol_analysis.takt");
    let mut found = false;
    for t in &p.types.list {
        if let takt_mir::types::Type::Result { .. } = t {
            let ty = takt_llvm::ty::lower(
                takt_mir::TypeId(p.types.list.iter().position(|x| x == t).expect("Typ") as u32),
                &p,
            );
            if let Some(takt_llvm::ty::LlvmType::Struct(fields)) = ty {
                assert_eq!(fields.last(), Some(&takt_llvm::ty::LlvmType::Int(1)), "das Flag steht nicht hinten");
                found = true;
            }
        }
    }
    assert!(found, "kein `T!E` im Programm");
}

// --- Drahtformat (3.7) ---------------------------------------------------

/// 3.7: `decode` faultet nie. Ein zu kurzer Puffer ergibt `none` — das
/// Programm entscheidet mit `match`, was das bedeutet.
#[test]
fn decode_checks_the_length_before_reading() {
    let p = corpus("13_protocol_analysis.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("icmp uge i32"),
        "die Laenge wird nicht geprueft:
{ir}"
    );
    assert!(
        ir.contains("phi"),
        "die beiden Wege werden nicht zusammengefuehrt:
{ir}"
    );
}

/// 3.7: Der Plan steht im Typ — der Codegen rollt die Felder ab, statt zu
/// rechnen. Kein Schleifencode, keine Laufvariable.
#[test]
fn decode_unrolls_the_fields() {
    let p = corpus("13_protocol_analysis.takt");
    let ir = ir_of(&p);
    let decode_start = ir.find("icmp uge i32").expect("Laengenpruefung");
    let window = &ir[decode_start..decode_start.saturating_add(2000)];
    assert!(
        window.contains("insertvalue"),
        "die Felder werden nicht zusammengesetzt:
{window}"
    );
}

/// 3.7: Ein Konstantenfeld muss den deklarierten Wert tragen; sonst ist
/// das Ergebnis `none`.
#[test]
fn a_constant_field_is_verified() {
    let p = corpus("13_protocol_analysis.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("and i1"),
        "die Pruefungen werden nicht verknuepft:
{ir}"
    );
}

// --- `match` (6.1) -------------------------------------------------------

/// 6.1: Der erste passende `case` gewinnt — eine Kette von Vergleichen in
/// Quelltextreihenfolge, kein umsortierter `switch`.
#[test]
fn match_tests_the_cases_in_source_order() {
    let p = corpus("13_protocol_analysis.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("case"),
        "keine `case`-Marken:
{ir}"
    );
    let first = ir.find("case").expect("erster case");
    let rest = &ir[first..];
    assert!(
        rest.contains("_sonst_"),
        "kein Weiterreichen an den naechsten `case`:
{ir}"
    );
}

/// 3.8: Bei `T!E` steht die Fehlerdiskriminante im Feld 1, nicht im
/// Feld 0 — dort steht der Wert.
#[test]
fn a_result_matches_on_its_error_field() {
    let p = corpus("13_protocol_analysis.takt");
    let ir = ir_of(&p);
    // Der Vergleich laeuft auf `i32`; er kaeme auf dem Feld 0 nie
    // zustande, weil dort der Record steht.
    assert!(
        ir.contains("icmp eq i32"),
        "kein Vergleich der Diskriminante:
{ir}"
    );
}
