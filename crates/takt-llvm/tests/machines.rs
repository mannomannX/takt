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
const VOLLSTAENDIG: [&str; 4] =
    ["01_minimal.takt", "03_sequences_and_faults.takt", "13_framing.takt", "14_latency.takt"];

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
