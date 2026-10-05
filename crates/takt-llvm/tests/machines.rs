//! Maschinen nach LLVM-IR (11.2): Zustands-Struct und Schrittfunktion.
//!
//! Die Tests uebersetzen echte Korpusprogramme, nicht erfundene MIR —
//! ein Struct, der nur fuer Testdaten stimmt, ist kein Beleg.

use takt_llvm::machine::{self, Role, depth, state_struct};
use takt_llvm::toolchain::{Clang, find};
use takt_mir::program::Program;

/// **Im `idle` wacht der Wake-Strom, der andere wird verworfen** (5.10,
/// 9.9): `_idle` fragt das Fenster von `bell`, `_drop` rueckt nur den
/// Cursor von `data` vor.
#[test]
fn idle_watches_the_wake_stream_and_drops_the_other() {
    let p = corpus("92_idle_streams.takt");
    let ir = takt_llvm::lower::program(&p, "x86_64-pc-windows-msvc", &takt_llvm::symbols::Prefix::default()).ir;
    // Der Rumpf an seiner `define`-Zeile, nicht der Aufruf in seinem
    // Einstieg `takt_<maschine>_idle` (12.11).
    let body = |name: &str| {
        let head = ir
            .lines()
            .find(|l| l.starts_with("define") && l.contains(&format!("@{name}(")))
            .unwrap_or_else(|| panic!("kein `{name}` in der IR"));
        let start = ir.find(head).unwrap_or_default();
        ir[start..].split("\n}").next().unwrap_or_default().to_string()
    };
    let count = |channel: &str| {
        let id = p.channels.iter().position(|c| c.name == channel).expect("Channel");
        format!("@app_stream_count(ptr %arena, i32 {id},")
    };
    let (idle, dropped) = (body("sleeper_idle"), body("sleeper_drop"));
    assert!(idle.contains(&count("bell")) && !idle.contains(&count("data")), "{idle}");
    assert!(dropped.contains(&count("data")) && !dropped.contains(&count("bell")), "{dropped}");
    assert!(!ir.contains("@feeder_drop("), "ohne `idle` kein Verwurf");
}

/// Uebersetzt ein Korpusprogramm zur MIR.
fn corpus(name: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
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
    lowered(p).ir
}

/// Die Uebersetzung, wie `takt build` sie macht (`lower::program`): Ein
/// Verwurf steht in `skipped`, statt still eine Maschine zu verlieren.
fn lowered(p: &Program) -> takt_llvm::lower::Lowered {
    takt_llvm::lower::program(p, "x86_64-pc-windows-msvc", &takt_llvm::symbols::Prefix::default())
}

/// Die Schrittfunktionen eines Moduls samt ihrer `loop:`-Rumpfe:
/// `define ... @<m>_step(` bzw. `@<m>_loop_<i>(` bis zur schliessenden
/// Klammer. Eintritt und Initialisierung bleiben aussen vor.
fn step_ir(ir: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in ir.lines() {
        if line.starts_with("define ") {
            inside = line.contains("_step(") || line.contains("_loop_");
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
        if line == "}" {
            inside = false;
        }
    }
    out
}

/// Die definierten Funktionen eines Moduls: Name und Rumpf bis zur
/// schliessenden Klammer. Eine Pruefung am Rumpf sieht nur, was dort
/// steht, nicht, was irgendwo im Modul vorkommt (GEN-007).
fn bodies(ir: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in ir.lines() {
        if line.starts_with("define ") {
            let name = line.split('@').nth(1).and_then(|r| r.split('(').next()).unwrap_or_default();
            current = Some((name.to_string(), String::new()));
        }
        if let Some((_, body)) = &mut current {
            body.push_str(line);
            body.push('\n');
        }
        if line == "}"
            && let Some(done) = current.take()
        {
            out.push(done);
        }
    }
    out
}

/// Die Korpusprogramme, die der Codegen vollstaendig senken muss. Die
/// Liste waechst mit ihm; sie steht hier, damit ein Rueckschritt auffaellt.
const KORPUS: [&str; 11] = [
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

/// Eine Maschine mit geschachtelten Zustaenden hat Tiefe > 1:
/// `RUNNING.WARMUP` in `17_nested` liegt eine Ebene unter der Wurzel.
#[test]
fn nested_states_increase_the_depth() {
    let p = corpus("17_nested.takt");
    let m = p.machines.iter().find(|m| m.name == "plant").expect("plant");
    assert_eq!(depth(m), 2);
    assert_eq!(state_struct(m, &p).expect("Struct baubar").depth, 2);
    for name in ["03_sequences_and_faults.takt", "17_nested.takt"] {
        let p = corpus(name);
        for m in &p.machines {
            let st = state_struct(m, &p).expect("Struct baubar");
            assert_eq!(st.depth, depth(m), "{}", m.name);
        }
    }
}

/// 11.2 nennt die Felder in einer Reihenfolge; sie ist die einzige
/// Quelle fuer die Indizes im erzeugten Code. `last_fault` fuehrt nur, wer
/// es liest (5.3).
#[test]
fn every_machine_has_the_fields_the_reference_names() {
    for name in ["03_sequences_and_faults.takt", "98_last_fault.takt"] {
        let p = corpus(name);
        for m in &p.machines {
            let st = state_struct(m, &p).expect("Struct baubar");
            for role in [Role::Conf, Role::TimeInState, Role::Deliver, Role::Pc] {
                assert!(st.index_of(role, 0).is_some(), "{}: {role:?} fehlt", m.name);
            }
            let reads = takt_mir::visit::reads_last_fault(m);
            assert_eq!(st.index_of(Role::LastFault, 0).is_some(), reads, "{name}: {}", m.name);
            assert_eq!(reads, name.starts_with("98"), "{name}: {}", m.name);
        }
    }
}

/// Die Variablen der Maschine stehen im Struct — als Feld oder im Overlay (11.2).
#[test]
fn the_variables_of_a_machine_are_fields_of_its_state() {
    let p = corpus("13_protocol_analysis.takt");
    for m in &p.machines {
        let Some(st) = state_struct(m, &p) else { continue };
        let vars = st.fields.iter().filter(|f| f.role == Role::Var).count();
        let overlaid = st.overlay.iter().flatten().count();
        assert_eq!(vars + overlaid, m.vars.len(), "{}", m.name);
    }
}

/// 11.2: Zustandslokale Variablen von Geschwistern teilen den Platz; die
/// einer Maschine nicht.
#[test]
fn sibling_states_overlay_their_variables() {
    let p = corpus("13_protocol_analysis.takt");
    for m in &p.machines {
        let Some(st) = state_struct(m, &p) else { continue };
        for (i, v) in m.vars.iter().enumerate() {
            let local =
                matches!(v.scope, takt_mir::machine::VarScope::State(_) | takt_mir::machine::VarScope::Lifted(_))
                    && !v.public;
            assert_eq!(st.overlay[i].is_some(), local && st.index_of(Role::Var, i).is_none(), "{}.{}", m.name, v.name);
        }
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
    for name in ["03_sequences_and_faults.takt", "17_nested.takt"] {
        let p = corpus(name);
        for m in &p.machines {
            let d = depth(m) as usize;
            for leaf in machine::leaves(m) {
                let path = machine::path_to(m, leaf);
                assert!(path.len() <= d, "{}: Pfad {} laenger als Tiefe {d}", m.name, path.len());
                // Wurzel vorn, Blatt hinten, dazwischen je Schritt ein Kind.
                assert!(m.states[path[0].index()].parent.is_none(), "{}: Pfad beginnt nicht an der Wurzel", m.name);
                assert_eq!(path.last(), Some(&leaf), "{}: Pfad endet nicht am Blatt", m.name);
                for pair in path.windows(2) {
                    assert_eq!(m.states[pair[1].index()].parent, Some(pair[0]), "{}: {path:?}", m.name);
                }
            }
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
    let Some(path) =
        takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")
    else {
        return;
    };
    let clang = Clang::At(path);
    for name in KORPUS {
        let p = corpus(name);
        let out = lowered(&p);
        assert!(out.complete(), "{name}: {:?}", out.skipped.iter().map(|s| &s.reason).collect::<Vec<_>>());
        assert!(out.without_persist.is_empty(), "{name}: ohne Lesepfad {:?}", out.without_persist);
        let ir = out.ir;
        assert!(ir.contains("_state = type"), "{name}: kein Zustands-Struct");
        // Je Maschine mit eigenem Schritt ihre Definition (11.2), nicht nur
        // irgendein `_step(`.
        for m in p.machines.iter().filter(|m| m.kind != takt_mir::machine::MachineKind::Template) {
            let step = format!("@{}_step(", takt_llvm::fns::sanitized(&m.name));
            assert!(
                ir.lines().any(|l| l.starts_with("define ") && l.contains(&step)),
                "{name}: `{step}` ist nicht definiert"
            );
        }
        assert!(ir.contains("switch i8"), "{name}: kein `switch` ueber die Blaetter (11.2)");
        let dir =
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-llvm-m-{}", name.replace('.', "_")));
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
    // Der Fault-Pfad meldet den Fault der Runtime (5.3).
    assert!(
        ir.contains("call void @app_fault("),
        "der Fault wird nicht gemeldet:
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

/// GEN-006: Eine Maschine, die der Codegen nicht vollstaendig senkt, nennt
/// ihren Grund und hinterlaesst keine halbe Funktion — sie waere gueltige
/// IR mit falschem Inhalt. `{d:float}` im Handlermuster senkt er nicht
/// (die Umwandlung haette eine zweite Rundungsquelle, `captures.rs`).
#[test]
fn an_incomplete_machine_leaves_no_half_function() {
    let src = "system:
    language = 1
    tick = 10 ms

input rx : stream<line<32>> @ hw(\"uart0/rx\") with max_rate = 100 Hz, framing = lines, capacity = 4

output o : float @ hw(\"o/o\") with safe = 0.0

machine m:
    initial RUN
    state RUN:
        on rx matches \"d={d:float}\" as ev:
            o = ev.d
";
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let p = takt_sema::compile(src, &options).program.expect("Programm");
    let out = lowered(&p);
    assert!(!out.complete(), "`{{d:float}}` wird heute nicht gesenkt; der Test braucht ein anderes Beispiel");
    assert!(out.skipped.iter().any(|s| s.machine == "m"), "{:?}", out.skipped);
    let defines = out.ir.lines().filter(|l| l.starts_with("define ")).count();
    let ends = out.ir.lines().filter(|l| *l == "}").count();
    assert_eq!(defines, ends, "offene oder halbe Funktion");
    assert!(!out.ir.lines().any(|l| l.starts_with("define ") && l.contains("@m_step(")), "halber Schritt");
}

/// GEN-011: Die IR jedes Korpusprogramms traegt auf jedem Ziel weder
/// Fast-Math-Flags noch `llvm.fmuladd` (4.2): LLVM darf nicht
/// kontrahieren, was der Interpreter getrennt rundet.
#[test]
fn machine_ir_carries_no_fast_math_flags() {
    let extra = ["46_matrices.takt", "97_fast_math.takt", "101_correct_math.takt", "102_correct_math_f32.takt"];
    for name in KORPUS.iter().chain(&extra) {
        let p = corpus(name);
        for target in takt_llvm::target::Target::ALL {
            let ir = takt_llvm::lower::program(&p, target.triple, &takt_llvm::symbols::Prefix::default()).ir;
            for (i, line) in ir.lines().enumerate() {
                let code = line.split(';').next().unwrap_or("");
                for flag in ["fast", "nnan", "ninf", "nsz", "arcp", "contract", "reassoc", "afn"] {
                    assert!(
                        !code.split_whitespace().any(|w| w == flag),
                        "{name} ({}) Zeile {}: `{flag}`",
                        target.name,
                        i + 1
                    );
                }
                assert!(!code.contains("fmuladd"), "{name} ({}) Zeile {}: `fmuladd`", target.name, i + 1);
            }
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

/// 5.2: Beim Eintritt in einen Zustand beginnt `t_in_state` bei null.
///
/// Bei *null*, nicht bei -1: Der Entry-Modus (5.2 Regel 4) laeuft noch im
/// selben Tick und liest den Wert. Ein `-1` haette jede `after`-Frist um
/// einen Tick verschoben; der Interpreter setzt in `enter_state`
/// ebenfalls 0 (FB-89).
#[test]
fn a_transition_resets_the_time_in_state() {
    let p = corpus("01_minimal.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("store i64 0, ptr"),
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
    // Der `loop:` steht als Funktion und wird vor den Uebergaengen gerufen.
    let check = ir[watch..].find("call i8 @tank_guard_loop_").expect("der loop: von WATCH");
    let trans = ir[watch..].find("uebergang").expect("der Uebergang aus WATCH");
    assert!(
        ir.contains("fcmp"),
        "der check in WATCH fehlt:
{ir}"
    );
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
    for name in KORPUS {
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

/// 7.1: `after` feuert „nie im Entry-Tick" — die Frist steht in
/// Aktivierungen im Vergleich, und sie ist mindestens eins.
#[test]
fn after_never_fires_in_the_entry_tick() {
    let p = corpus("16_timing.takt");
    let ir = step_ir(&ir_of(&p));
    let deadlines: Vec<i64> = ir
        .lines()
        .filter_map(|l| l.trim().strip_prefix("%").and_then(|l| l.split_once(" = icmp sge i64 %")))
        .filter_map(|(_, rest)| rest.split_once(", ").and_then(|(_, k)| k.trim().parse().ok()))
        .collect();
    assert!(
        !deadlines.is_empty() && deadlines.iter().all(|k| *k >= 1),
        "die Frist in Aktivierungen fehlt oder ist null:
{ir}"
    );
}

// --- Geschachtelte Zustaende (5.2) --------------------------------------

/// 5.2: Aktiv ist ein *Pfad*, nicht ein Zustand. Der `loop:` einer
/// Zwischenebene ist die Invariante aller Zustaende darunter: Er steht
/// einmal in der IR, und ein `switch` ueber die Blaetter fuehrt von ihm
/// in jedes Blatt darunter (FB-222).
#[test]
fn an_ancestor_loop_runs_in_every_leaf_below_it() {
    let p = corpus("17_nested.takt");
    let ir = step_ir(&ir_of(&p));
    // Der `check p < 90 bar` steht einmal im Programm, unter `RUNNING`
    // mit zwei Blaettern — und genau einmal in der IR.
    let checks = ir.matches("fcmp olt").count();
    assert_eq!(
        checks, 1,
        "der `check` von RUNNING steht nicht genau einmal:
{ir}"
    );
    assert!(
        ir.contains("switch i8 ") && ir.matches("label %ebene").count() >= 2,
        "die Verzweigung von RUNNING auf seine Blaetter fehlt:
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
    let (raus, rein) = takt_llvm::machine::crossing(m, Some(warmup), Some((active, active)));
    assert_eq!(raus, vec![warmup], "nur das Geschwister wird verlassen");
    assert_eq!(rein, vec![active], "nur das Geschwister wird betreten");
}

/// Ein Uebergang aus der Tiefe nach aussen verlaesst die ganze Kette.
#[test]
fn a_transition_out_of_a_hierarchy_exits_the_whole_chain() {
    let p = corpus("17_nested.takt");
    let m = &p.machines[0];
    let find = |n: &str| takt_mir::StateId(m.states.iter().position(|s| s.name == n).expect(n) as u32);
    let (raus, _) = takt_llvm::machine::crossing(m, Some(find("WARMUP")), Some((find("SAFE"), find("SAFE"))));
    assert_eq!(raus, vec![find("WARMUP"), find("RUNNING")], "erst das Blatt, dann sein Elternteil");
}

/// 9.3: Der gemeinsame Vorfahr liegt echt oberhalb des *Zielzustands*, nicht
/// nur oberhalb des Blatts, in dem das Betreten endet. Ein Uebergang auf
/// den eigenen Elternzustand verlaesst und betritt ihn neu; der eines
/// Blatts auf sich selbst laesst den Elternzustand stehen.
#[test]
fn the_common_ancestor_lies_strictly_above_the_target_state() {
    let p = corpus("17_nested.takt");
    let m = &p.machines[0];
    let find = |n: &str| takt_mir::StateId(m.states.iter().position(|s| s.name == n).expect(n) as u32);
    let (running, warmup, active) = (find("RUNNING"), find("WARMUP"), find("ACTIVE"));
    let (raus, rein) = takt_llvm::machine::crossing(m, Some(active), Some((running, warmup)));
    assert_eq!(raus, vec![active, running], "der Elternzustand wird verlassen");
    assert_eq!(rein, vec![running, warmup], "und neu betreten, bis zu seinem `initial`");
    let (raus, rein) = takt_llvm::machine::crossing(m, Some(warmup), Some((warmup, warmup)));
    assert_eq!((raus, rein), (vec![warmup], vec![warmup]), "nur das Blatt selbst");
}

/// Marken muessen je erzeugter Verzweigung eindeutig sein: Ein Blatt
/// fuehrt auch die Uebergaenge seiner Vorfahren aus, und zwei Ebenen
/// haetten sonst dieselbe Marke — ungueltige IR, die nur LLVM findet.
#[test]
fn every_label_in_the_generated_ir_is_unique() {
    for name in KORPUS {
        let p = corpus(name);
        let ir = ir_of(&p);
        let mut seen = std::collections::BTreeSet::new();
        for line in ir.lines() {
            let t = line.trim_end();
            // Marken gelten je Funktion.
            if t.starts_with("define ") {
                seen.clear();
            }
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
    let Some(path) =
        takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")
    else {
        return;
    };
    let clang = Clang::At(path);
    for name in KORPUS {
        let p = corpus(name);
        let ir = ir_of(&p);
        let dir =
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-llvm-all-{}", name.replace('.', "_")));
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
    for name in KORPUS {
        let p = corpus(name);
        let ir = ir_of(&p);
        let (mut terminated, mut inside) = (false, false);
        for line in ir.lines() {
            let t = line.trim();
            // Nur Rumpfe; Metadaten und Deklarationen stehen ausserhalb.
            if t.starts_with("define") {
                (terminated, inside) = (false, true);
                continue;
            }
            if t == "}" {
                inside = false;
                continue;
            }
            if !inside || t.is_empty() || t.starts_with(';') {
                continue;
            }
            if t.ends_with(':') {
                terminated = false;
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
    for name in KORPUS {
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

/// Ein monomorphisierter Name traegt seine Einheiten (`clamp[bar]`,
/// 3.12) — lesbar in Diagnosen, in einem LLVM-Bezeichner aber nicht
/// erlaubt. Die Bereinigung ersetzt zeichenweise, damit `clamp[bar]` und
/// `clamp[psi]` zwei Symbole bleiben.
#[test]
fn a_monomorphised_name_becomes_a_valid_symbol() {
    use takt_llvm::fns::sanitized;
    assert_eq!(sanitized("clamp[bar]"), "clamp.bar.");
    assert_ne!(sanitized("clamp[bar]"), sanitized("clamp[psi]"), "zwei Instanzen, zwei Symbole");
    assert_eq!(sanitized("zaehler.reset"), "zaehler.reset", "Blockmethoden bleiben unveraendert");
    assert_eq!(sanitized("schlicht"), "schlicht", "ein gewoehnlicher Name bleibt, wie er ist");
    // Jedes Zeichen des Ergebnisses ist in einem LLVM-Bezeichner erlaubt.
    for name in ["clamp[bar]", "lim[1/s, m]", "b.step"] {
        assert!(
            sanitized(name).chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.')),
            "`{name}` ergibt keinen gueltigen Bezeichner"
        );
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
    // Der Rumpf, der `c.step(…)` ruft: Dort wird das Flag gefragt, gesetzt
    // und um den Aufruf verzweigt.
    let is_call = |l: &str| l.contains("call ") && l.contains("@takt_block_counter_step(");
    let calls: Vec<(String, String)> =
        bodies(&ir).into_iter().filter(|(n, b)| n.starts_with("gauge_") && b.lines().any(is_call)).collect();
    assert!(!calls.is_empty(), "kein Rumpf von `gauge` ruft `step`:\n{ir}");
    for (name, body) in &calls {
        let lines: Vec<&str> = body.lines().collect();
        let call = lines.iter().position(|l| is_call(l)).unwrap_or_default();
        let before = &lines[..call];
        assert!(before.iter().any(|l| l.contains("br i1")), "`{name}`: kein Zweig vor dem Aufruf:\n{body}");
        assert!(
            before.iter().any(|l| l.contains("store i1 true")),
            "`{name}`: das Flag steht nicht vor dem Aufruf:\n{body}"
        );
    }
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
        ir.contains("@app_stream_count"),
        "die Fenstergroesse wird nicht erfragt:
{ir}"
    );
    assert!(
        ir.contains("@app_stream_at"),
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
        ir.contains("@app_stream_examined"),
        "`examined` wird nicht gemeldet:
{ir}"
    );
    // Der Aufruf steht *vor* dem Rumpf des Handlers: Er gilt fuer jedes
    // Element, nicht nur fuer die getroffenen.
    let at = ir.find("@app_stream_at").expect("at");
    let examined = ir[at..].find("@app_stream_examined").expect("examined");
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
    // Im Rumpf mit dem `match` von `parser`: erst die `case`-Marken, dann
    // das Weiterreichen an den naechsten.
    let (name, body) = bodies(&ir)
        .into_iter()
        .find(|(n, b)| n.starts_with("parser_") && b.contains("case"))
        .unwrap_or_else(|| panic!("kein Rumpf von `parser` mit `case`-Marken:\n{ir}"));
    let first = body.find("case").unwrap_or_default();
    assert!(body[first..].contains("_sonst_"), "`{name}`: kein Weiterreichen an den naechsten `case`:\n{body}");
}

/// 3.8: `match` auf `T!E` entscheidet am Flag im Feld 2, nicht an den
/// Diskriminanten des Fehlers — bei `OK` ist das Fehlerfeld unbestimmt,
/// und ein Fehler-Enum mit einer Variante hat keine zweite (FB-318).
#[test]
fn a_result_matches_on_its_flag() {
    let p = corpus("13_protocol_analysis.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains(", 2\n") && ir.contains("icmp eq i1"),
        "kein Vergleich des Flags:
{ir}"
    );
}

// --- Kennlinien (3.9) ----------------------------------------------------

/// 3.9: `interp` ist stueckweise linear und an den Raendern geklemmt.
/// Geklemmt heisst: Links vom ersten und rechts vom letzten Stuetzpunkt
/// steht dessen Wert, nicht eine Extrapolation — das haelt die Funktion
/// total (4.1).
#[test]
fn interp_clamps_at_both_ends() {
    let p = corpus("02_units_and_data.takt");
    let ir = ir_of(&p);
    assert!(
        ir.contains("fcmp ole"),
        "kein Vergleich gegen die Stuetzstellen:
{ir}"
    );
    assert!(
        ir.contains("select i1"),
        "die Segmente werden nicht ausgewaehlt:
{ir}"
    );
}

/// Die Stuetzstellen stehen als Literal am Aufruf, also ist die Suche
/// abgerollt — keine Schleife, keine Schranke zu pruefen (4.1).
#[test]
fn interp_unrolls_the_search() {
    let p = corpus("02_units_and_data.takt");
    let ir = ir_of(&p);
    // Vier Stuetzstellen ergeben drei Segmente, jedes mit seiner
    // Steigung: `fsub`, `fsub`, `fsub`, `fmul`, `fdiv`, `fadd`.
    assert!(
        ir.matches("fdiv").count() >= 3,
        "die Segmente sind nicht abgerollt:
{ir}"
    );
}

/// Die Rechnung ist die des Interpreters, Operation fuer Operation:
/// `y0 + (y1 - y0) * (x - x0) / (x1 - x0)`. Eine andere Klammerung waere
/// mathematisch gleich und in Fliesskomma eine andere Zahl (9.4.4).
#[test]
fn interp_keeps_the_operation_order_of_the_interpreter() {
    let p = corpus("02_units_and_data.takt");
    let ir = ir_of(&p);
    // Im Rumpf von `oven`, der `interp` rechnet: die Multiplikation vor der
    // Division, nicht irgendwo davor im Modul.
    let (name, body) = bodies(&ir)
        .into_iter()
        .find(|(n, b)| n.starts_with("oven_") && b.contains("fdiv"))
        .unwrap_or_else(|| panic!("kein Rumpf von `oven` mit einer Division:\n{ir}"));
    let at = body.find("fdiv").unwrap_or_default();
    assert!(body[..at].contains("fmul"), "`{name}`: die Multiplikation steht nicht vor der Division:\n{body}");
}

/// **Der virtuelle Schlaf rueckt jeden Zeitzaehler vor** (9.9, FB-268).
///
/// `after` liest den Zaehler seines Zustands, der Schritt erhoeht alle;
/// `_advance` muss es ebenso halten, sonst verschlaeft jede Frist
/// ausserhalb des ersten Zustands — in `59_persist_idle` die von `SLEEP`.
#[test]
fn virtual_sleep_advances_every_timer() {
    let p = corpus("59_persist_idle.takt");
    let m = &p.machines[0];
    let ir = takt_llvm::lower::program(&p, "x86_64-pc-windows-msvc", &takt_llvm::symbols::Prefix::default()).ir;
    let head = format!("@{}_advance(", m.name);
    let start = ir.find(&head).unwrap_or_else(|| panic!("kein `{head}` in der IR"));
    let call = ir[start..].lines().find(|l| l.contains("@takt_advance_timers(")).unwrap_or_else(|| panic!("{ir}"));
    assert!(call.contains(&format!("i32 {}, i64", machine::timers(m))), "{call}");
}

/// Wie viele Indexpruefungen eine Maschine noch traegt: was die Analyse
/// beweist, faellt aus der MIR (3.4).
fn index_checks(p: &Program) -> usize {
    let mut n = 0;
    for m in &p.machines {
        takt_mir::visit::for_each_expr_machine(m, &mut |e| {
            n += usize::from(matches!(
                e.kind,
                takt_mir::expr::ExprKind::Checked { kind: takt_mir::expr::CheckedKind::Index { .. }, .. }
            ));
        });
    }
    n
}

/// SYN-024: `b.clear()` im Schleifenkoerper nimmt die Schranke `k < b.len`
/// zurueck, auch wenn `written_vars` den Empfaenger nicht nennt: Der
/// Methodenaufruf vergisst sie, und die Indexpruefung von `b[k]` bleibt —
/// geweitet (300 Durchlaeufe), abgerollt (100) und an der Grenze
/// `UNROLL_LIMIT` (128 und 129 Durchlaeufe zu je zwei Anweisungen).
#[test]
fn clearing_the_vector_in_a_loop_keeps_the_index_check() {
    assert_eq!(takt_mir::analysis::walk::UNROLL_LIMIT, 256, "die Grenzfaelle unten rechnen mit 256");
    for (n, clear) in [(300, true), (100, true), (128, true), (129, true), (100, false)] {
        let clear = if clear { "b.clear()" } else { "pass" };
        let src = format!(
            "system:
    language = 1
    tick = 10 ms

output o : int @ hw(\"o/o\") with safe = 0

machine m:
    var b : vec<int, 8> = default
    var k : int in 0..7 = 0
    var x : int = 0
    initial RUN
    state RUN:
        loop:
            b.push(1)
            if k < b.len:
                for i in range({n}):
                    x = b[k]
                    {clear}
            o = x
"
        );
        let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
        let out = takt_sema::compile(&src, &options);
        let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
        assert!(errors.is_empty(), "{n}: {}", errors.join("\n"));
        let p = out.program.expect("Programm");
        // Ohne `clear` beweist die Schranke den Index; die Gegenprobe zeigt,
        // dass der Test etwas misst.
        let want = usize::from(clear != "pass");
        assert_eq!(index_checks(&p), want, "{n} Durchlaeufe mit `{clear}`");
    }
}

/// SEM1-049: Im Modus ENTRY ist das Fenster eines Stroms leer (9.6) — in
/// `enter:` und `exit:` immer, auch wo ein Wechsel sie im Schritt ausfuehrt,
/// in einem `loop:` im Eintritts-Tick. Jede Zaehlung des Fensters fuer
/// `for x in s` und `s.count` (8.6) geht darum durch ein `select` auf null:
/// in der `loop:`-Funktion ueber ihren Entry-Parameter, sonst fest.
#[test]
fn a_window_is_empty_in_entry_mode() {
    let src = "system:
    language = 1
    tick = 1 ms

input rx : stream<u8> @ hw(\"u/rx\") with max_rate = 200 kHz, capacity = 256
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    var count : int in 0..99 = 0
    initial RUN
    state RUN:
        enter:
            for x in rx:
                count = (count + 1) % 100
            count = min(rx.count, 99)
        loop:
            for x in rx:
                count = (count + 1) % 100
            n = min(rx.count, 99)
        when count > 50: -> DONE
        exit:
            for x in rx:
                count = (count + 1) % 100
    state DONE:
        loop:
            n = count
";
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    assert!(!out.has_errors(), "{:?}", out.diagnostics);
    let p = out.program.expect("Programm");
    let low = lowered(&p);
    assert!(low.skipped.is_empty(), "{:?}", low.skipped);
    let mut function = String::new();
    let mut counted = 0;
    for line in low.ir.lines() {
        if line.starts_with("define ") {
            function = line.to_string();
        }
        let Some((reg, _)) = line.trim().split_once(" = call i32 @app_stream_count(") else { continue };
        counted += 1;
        let gated = low
            .ir
            .lines()
            .find_map(|l| l.trim().split_once(&format!(", i32 0, i32 {reg}")).map(|(g, _)| g.to_string()));
        let gate = gated.unwrap_or_else(|| panic!("{reg} in `{function}` geht ungeprueft in die Schleife"));
        let want = if function.contains("_loop_") { "select i1 %5" } else { "select i1 true" };
        assert!(gate.ends_with(want), "`{function}`: {gate}");
    }
    assert!(counted >= 5, "Fenster in `enter:`, `exit:` und `loop:`:\n{}", low.ir);
}

/// INT-025 (5.9): Der erzeugte Restore verwirft einen `persist`-Eintrag mit
/// NaN- oder Inf-Bitmuster auch ohne Range, wie der Interpreter
/// (`Nvm::load`): Vor der Range steht die Pruefung des Exponenten.
#[test]
fn the_restore_refuses_a_non_finite_float() {
    let src = "system:
    language = 1
    tick = 1 ms
    float = f32

output n : float @ hw(\"o/n\") with safe = 0.0

machine m:
    persist var level : float = 1.5
    persist var wide : f64 = 2.5
    initial RUN
    state RUN:
        loop:
            n = level
";
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let p = takt_sema::compile(src, &options).program.expect("Programm");
    let low = lowered(&p);
    assert!(low.skipped.is_empty(), "{:?}", low.skipped);
    let head = low.ir.lines().find(|l| l.starts_with("define") && l.contains(" @m_persist_restore(")).expect("Restore");
    let start = low.ir.find(head).unwrap_or_default();
    let body = low.ir[start..].split("\n}").next().unwrap_or_default();
    assert!(body.contains(", 2139095040") && body.contains(", 9218868437227405312"), "Exponent je Breite:\n{body}");
}

/// SEM1-030: `vec<T, N>` in `persist` senkt der Codegen — Laenge, dann die
/// Elemente in einer Schleife zur Laufzeit; der Lauf gegen den Interpreter
/// steht in `takt-conformance/tests/persist_native.rs`
/// (`random_vectors_are_encoded_alike`). Im Pruefdurchlauf liegt jedes
/// gelesene Byte vor dem Ende des Eintrags: Eine Laenge, die mehr Elemente
/// verspricht, als der Eintrag traegt, liest nicht hinter die Nutzlast.
#[test]
fn a_persisted_vector_is_lowered_and_read_within_its_entry() {
    let src = "system:
    language = 1
    tick = 1 ms

output n : int @ hw(\"o/n\") with safe = 0

machine m:
    persist var list : vec<int, 4> = default
    initial RUN
    state RUN:
        loop:
            n = list.len as int
";
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let p = takt_sema::compile(src, &options).program.expect("Programm");
    let low = lowered(&p);
    assert!(low.skipped.is_empty(), "{:?}", low.skipped);
    let body = |name: &str| {
        let head = low.ir.lines().find(|l| l.starts_with("define") && l.contains(&format!(" @{name}("))).expect(name);
        let start = low.ir.find(head).unwrap_or_default();
        low.ir[start..].split("\n}").next().unwrap_or_default().to_string()
    };
    let (restore, snapshot) = (body("m_persist_restore"), body("m_persist_snapshot"));
    assert!(restore.contains("vec") && snapshot.contains("vec"), "eine Schleife je Richtung");
    let loads = restore.matches("load i64, ptr").count();
    let bounds = restore.matches("icmp ule i64").count();
    assert!(bounds >= 2, "Grenzen je Lesen im Pruefdurchlauf ({bounds} bei {loads} Ladevorgaengen):\n{restore}");
}

/// KON1-017 (Lemma 3.4): Eine Schiebung rechnet nur dann in `i32`, wenn ihr
/// Betrag bewiesen in `0..31` liegt. `m >> 50` passt mit `m in 0..1000` in
/// `i32`, aber `ashr i32 _, 50` ist undefiniert (LLVM: poison); ebenso ein
/// Betrag, der selbst nur bis 8191 reicht.
#[test]
fn a_shift_by_more_than_31_bits_stays_wide() {
    let src = "system:
    language = 1
    tick = 1 ms

input k : int in 0..100000000 @ hw(\"i/k\")
output y : int @ hw(\"o/y\") with safe = 0
output z : int @ hw(\"o/z\") with safe = 0
output w : int @ hw(\"o/w\") with safe = 0

machine m:
    var n : int in 0..1000 = 7
    initial RUN
    state RUN:
        loop:
            n = (n + 1) % 1000
            y = n >> 50
            z = 3 >> (k.or(0) >> 15)
            w = n >> 3
";
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let p = takt_sema::compile(src, &options).program.expect("Programm");
    let ir = ir_of(&p);
    for (name, body) in bodies(&ir).into_iter().filter(|(n, _)| n.starts_with("m_")) {
        for line in body.lines().filter(|l| l.contains("ashr i32") || l.contains("shl i32") || l.contains("lshr i32")) {
            let amount = line.rsplit(", ").next().unwrap_or_default();
            let bounded = amount.parse::<u32>().is_ok_and(|a| a < 32);
            assert!(bounded, "`{name}`: Schiebung in i32 um einen unbeschraenkten Betrag: {line}");
        }
    }
}

/// `for (k, v) in m` ueber eine `map` (3.9, Korpus 114): Der Codegen senkt
/// es — eine Schleife ueber die Slots in ihrer Reihenfolge, die belegten
/// (Marke 1) mit Schluessel und Wert aus der kanonischen Form; der Lauf
/// gegen den Interpreter steht im Vergleich von `takt-conformance`.
#[test]
fn a_map_is_iterated_slot_by_slot() {
    let p = corpus("114_for_pairs.takt");
    let low = lowered(&p);
    assert!(low.skipped.is_empty(), "{:?}", low.skipped);
    let pairs: Vec<(String, String)> = bodies(&low.ir).into_iter().filter(|(_, b)| b.contains("paare")).collect();
    assert_eq!(pairs.len(), 1, "eine Funktion mit der Schleife");
    let body = &pairs[0].1;
    assert!(body.contains("icmp eq i8") && body.contains(", 1\n"), "die Marke eines belegten Slots:\n{body}");
}
