//! Lowering vom Syntaxbaum in die MIR (plan/m1.md 3): Prelude, Typen,
//! Einheiten, Maschinen, Generics; der Textdump ist die Golden-Form.

use takt_diag::Policy;
use takt_mir::dump::dump_machine;
use takt_mir::{MachineId, Program};
use takt_sema::{Build, Compiled, Options};

fn options() -> Options {
    Options { policy: Policy::default(), build: Build::Sim, profile: None }
}

/// Uebersetzt und verlangt Fehlerfreiheit.
fn compile(src: &str) -> Program {
    let out = takt_sema::compile(src, &options());
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Uebersetzt und liefert die Diagnosen.
fn diagnose(src: &str) -> Compiled {
    takt_sema::compile(src, &options())
}

/// Codes der Fehler.
fn errors(out: &Compiled) -> Vec<&str> {
    out.diagnostics.iter().filter(|d| d.is_error()).map(|d| d.code).collect()
}

/// Meldungen aller Diagnosen.
fn messages(out: &Compiled) -> String {
    out.diagnostics.iter().map(|d| format!("{d}")).collect::<Vec<_>>().join("\n")
}

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

#[test]
fn prelude_lowers_without_errors() {
    let p = compile(HEAD);
    assert!(p.units.iter().any(|u| u.name == "bar"));
    assert!(p.units.iter().any(|u| u.name == "degC" && u.affine_offset.is_some()));
    assert!(p.enums.iter().any(|e| e.name == "FaultKind" && e.open));
    assert!(p.records.iter().any(|r| r.name == "LastFault"));
    assert_eq!(p.config.tick, 1_000_000);
}

#[test]
fn machine_with_states_and_transitions() {
    let src = format!(
        "{HEAD}\
input  tank_p : float[bar] in 0..100 bar @ hw(\"daq1/ai0\") with max_age = 5 ms
output valve  : bool                     @ hw(\"plc1/do0\") with safe = false
output tank_p_sim : float[bar]           @ sim(\"daq1/ai0\") with safe = 0 bar
command start
param LIMIT : float[bar] in 10..90 bar = 50 bar

machine ctrl:
    fault -> SAFE
    initial IDLE

    loop:
        check tank_p < LIMIT, \"overpressure {{tank_p}}\"

    state IDLE:
        when start: -> OPEN
    state OPEN:
        enter:
            valve = true
        after 200 ms: -> IDLE
    state SAFE:
        enter:
            valve = false
"
    );
    let p = compile(&src);
    let m = p.machines.iter().position(|m| m.name == "ctrl").expect("Maschine");
    let text = dump_machine(&p, MachineId(m as u32));
    assert!(text.contains("state IDLE:"), "{text}");
    assert!(text.contains("when start: -> OPEN"), "{text}");
    assert!(text.contains("after 200 ms: -> IDLE"), "{text}");
    assert!(text.contains("check (checked[Valid](tank_p) < LIMIT)"), "{text}");
    let machine = &p.machines[m];
    assert_eq!(machine.states.len(), 3);
    assert!(matches!(machine.fault_target, takt_mir::machine::FaultTarget::State(_)));
    assert_eq!(p.channels.iter().filter(|c| c.owner.is_some()).count(), 1, "valve hat einen Besitzer");
}

#[test]
fn sequence_becomes_states_after_desugaring() {
    let src = format!(
        "{HEAD}\
output valve : bool @ hw(\"plc1/do0\") with safe = false
output v_sim : bool @ sim(\"plc1/do0\") with safe = false

machine seq:
    initial RUN
    state RUN:
        sequence:
            valve = true
            wait 150 ms
            valve = false
            -> DONE
    state DONE:
        loop: pass
"
    );
    let p = compile(&src);
    assert!(p.is_core(), "nach dem Desugaring");
    let m = p.machines.iter().position(|m| m.name == "seq").expect("Maschine");
    let text = dump_machine(&p, MachineId(m as u32));
    assert!(text.contains("state RUN.S0:"), "{text}");
    assert!(text.contains("after 150 ms: -> RUN.S1"), "{text}");
    assert!(text.contains("valve = false"), "{text}");
    assert!(text.contains("-> DONE"), "{text}");
}

#[test]
fn units_are_nominal_and_affine_rules_hold() {
    let bad = |body: &str| {
        let src = format!("{HEAD}{body}");
        let out = diagnose(&src);
        assert!(errors(&out).contains(&"SC-3"), "erwartet SC-3 fuer:\n{body}\n{}", messages(&out));
    };
    bad("const X : float[bar] = 1 psi\n");
    bad("const X : float[bar] = 1 bar + 1 psi\n");
    bad("const X : float[degC] = 20 degC + 5 degC\n");
    bad("const X : float[bar] = 300\n");
    // erlaubt: Differenz affiner Punkte ist die Basiseinheit, Punkt + Vektor bleibt Punkt
    let p = compile(&format!(
        "{HEAD}const DT : float[K] = 30 degC - 20 degC\nconst T2 : float[degC] = 20 degC + 5 K\nconst P : float[psi] = (1 bar).to(psi)\n"
    ));
    let _ = p;
}

#[test]
fn generics_solve_units_from_arguments() {
    let src = format!(
        "{HEAD}\
input  p : float[bar] in 0..100 bar @ hw(\"daq/ai0\")
output p_sim : float[bar]           @ sim(\"daq/ai0\") with safe = 0 bar
output y : float[bar]               @ hw(\"out/y\")    with safe = 0 bar

machine m:
    initial RUN
    state RUN:
        loop:
            y = clamp(p, 0 bar, 50 bar)
"
    );
    let p = compile(&src);
    assert!(
        p.fns.iter().any(|f| f.name == "clamp[bar]"),
        "Instanz: {:?}",
        p.fns.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
    assert_eq!(p.fns.iter().filter(|f| f.name.starts_with("clamp")).count(), 1, "einmal instanziiert");
}

#[test]
fn unsolvable_unit_variable_asks_for_explicit_instantiation() {
    let src = format!(
        "{HEAD}\
fn zero[U]() -> float[U]:
    return 0 * (1 1/s).to(1/s) * 0

machine m:
    initial RUN
    state RUN:
        loop: pass
"
    );
    let out = diagnose(&src);
    assert!(!errors(&out).is_empty(), "eine offene Variable ist ein Fehler: {}", messages(&out));
}

#[test]
fn checks_report_their_numbers() {
    let cases: &[(&str, &str)] = &[
        // 2: unbekannter Name
        ("machine m:\n    initial RUN\n    state RUN:\n        loop:\n            var x = unbekannt\n", "SC-2"),
        // 3: Typfehler
        ("machine m:\n    initial RUN\n    state RUN:\n        loop:\n            var x : int = true\n", "SC-3"),
        // 8: unbekanntes Ziel
        ("machine m:\n    initial RUN\n    state RUN:\n        when true: -> WEG\n", "SC-8"),
        // 8: `check` in einem Aktionsblock (5.5)
        ("machine m:\n    initial RUN\n    state RUN:\n        enter:\n            check true, \"x\"\n", "SC-8"),
        // 19: `match` nicht erschoepfend
        (
            "enum Mode: A, B\nmachine m:\n    initial RUN\n    state RUN:\n        loop:\n            var e : Mode = A\n            match e:\n                case A: pass\n",
            "SC-19",
        ),
        // 51: offenes Enum ohne `case _`
        (
            "machine m:\n    initial RUN\n    state RUN:\n        loop:\n            match last_fault.kind:\n                case TIMEOUT: pass\n",
            "SC-51",
        ),
        // 9: Fault-Ziel ist der Zustand selbst
        ("machine m:\n    initial RUN\n    state RUN:\n        fault -> RUN\n        loop: pass\n", "SC-8"),
    ];
    for (body, code) in cases {
        let src = format!("{HEAD}{body}");
        let out = diagnose(&src);
        assert!(errors(&out).contains(code), "erwartet {code} fuer:\n{body}\ngefunden:\n{}", messages(&out));
    }
}

#[test]
fn shadowing_rules_follow_2_5() {
    // Prelude-Namen duerfen verdeckt werden (Warnung)
    let out = diagnose(&format!(
        "{HEAD}const PCT : int = 1\nmachine m:\n    initial RUN\n    state RUN:\n        loop: pass\n"
    ));
    assert!(errors(&out).is_empty(), "{}", messages(&out));
    // ein sichtbarer Name im inneren Bereich ist ein Fehler (Festlegung 5)
    let out = diagnose(&format!(
        "{HEAD}machine m:\n    var x : int = 1\n    initial RUN\n    state RUN:\n        loop:\n            var x : int = 2\n"
    ));
    assert!(errors(&out).contains(&"SC-2"), "{}", messages(&out));
}

#[test]
fn state_discriminants_stay_unique_with_an_explicit_faulted_state() {
    // 5.3 erlaubt einen ausdruecklichen `state FAULTED:`. Wurde er aus der
    // Variantenliste gefiltert, nachdem die Indizes vergeben waren, trafen
    // zwei Zustaende denselben Wert und wurden fuer Codegen, Wire-Layout und
    // Telemetrie ununterscheidbar.
    let program = compile(&format!(
        "{HEAD}\
output v : bool @ hw(\"o/v\") with safe = false
command reset

machine mm:
    initial S
    state FAULTED:
        when reset: -> S
    state S:
        enter:
            v = true
        loop: pass
"
    ));
    let e = program.enums.iter().find(|e| e.name == "mm.State").expect("Zustandstyp");
    let mut seen: Vec<i64> = e.variants.iter().map(|v| v.discriminant).collect();
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();
    assert_eq!(seen.len(), before, "doppelte Diskriminante in {:?}", e.variants);
}

#[test]
fn quality_accessors_need_an_input_channel() {
    // 3.5: Qualitaet und Alter gibt es nur an einem Channel. Ein `T?` kennt
    // nach 3.8 allein `.valid` und `.or(d)`; vorher nahm der Elaborator die
    // Zugriffe auch fuer ein Element eines gewoehnlichen Optional-Arrays an,
    // und der Interpreter meldete dann einen internen Fehler.
    let out = diagnose(&format!(
        "{HEAD}\
output d : Duration @ hw(\"o/d\") with safe = 0 ms

machine mm:
    var a : [2] bool? = [none, none]
    initial S
    state S:
        loop:
            d = a[1].age
"
    ));
    assert!(messages(&out).contains("kein Zugriff `age`"), "{}", messages(&out));
}

#[test]
fn an_overlong_period_is_an_error_not_a_wrapped_counter() {
    // 7.2: der Aktivierungszaehler ist 32 Bit breit. `as u32` wickelte den
    // Quotienten um, sodass `every 10 s` bei `tick = 1 ns` als 1410065408
    // Ticks lief, ohne jede Diagnose.
    let out = diagnose(
        "system:
    language = 1
    tick = 1 ns

output v : bool @ hw(\"o/v\") with safe = false

machine mm every 10 s:
    initial S
    state S:
        loop: pass
",
    );
    assert!(messages(&out).contains("Aktivierungszaehler"), "{}", messages(&out));
}

#[test]
fn extreme_literals_do_not_break_the_compiler() {
    // Der Compiler darf bei keinem Eingabeprogramm abstuerzen: die
    // aufrundende Division lief fuer Dauern nahe `i64::MAX` ueber, und die
    // Fortzaehlung der Enum-Diskriminanten ebenso.
    let out = diagnose(&format!(
        "{HEAD}\
output v : bool @ hw(\"o/v\") with safe = false

machine m:
    initial S
    state S:
        after 9223372036854775807 ns: -> S
"
    ));
    let _ = messages(&out);

    let out = diagnose(&format!("{HEAD}enum Ee: A = 9223372036854775807, B\n"));
    assert!(messages(&out).contains("i64::MAX"), "{}", messages(&out));
}

/// Pruefung 5 (Validitaets-Dominanz) meldet nie einen Code — sie ist
/// strukturell: Ein nicht dominiertes Lesen bekommt `checked[Valid]`, ein
/// dominiertes nicht („impliziter Check ist Default", Tabelle 10.1). Darum
/// kann sie kein Korpusverzeichnis haben; dieser Test haelt beide Haelften
/// fest.
#[test]
fn validity_dominance_shows_in_the_mir_not_in_a_code() {
    let src = format!(
        "{HEAD}input  p     : float[bar] @ hw(\"i/p\")     with max_rate = 100 Hz
output valve : bool       @ hw(\"o/valve\") with safe = false

machine ctrl:
    initial RUN
    state RUN:
        loop:
            valve = p > 1.0 bar
            if p.valid:
                valve = p > 2.0 bar
"
    );
    let p = compile(&src);
    let text = dump_machine(&p, MachineId(0));
    assert!(
        text.contains("checked[Valid](p) > 1.0"),
        "das ungeschuetzte Lesen traegt den Check:
{text}"
    );
    assert!(
        text.contains("(p > 2.0)"),
        "das dominierte Lesen steht roh da:
{text}"
    );
    assert!(
        !text.contains("checked[Valid](p) > 2.0"),
        "kein doppelter Check unter `valid`:
{text}"
    );
}
