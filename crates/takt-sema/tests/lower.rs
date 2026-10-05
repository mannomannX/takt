//! Lowering vom Syntaxbaum in die MIR (plan/m1.md 3): Prelude, Typen,
//! Einheiten, Maschinen, Generics; der Textdump ist die Golden-Form.

use takt_diag::Policy;
use takt_mir::dump::dump_machine;
use takt_mir::{MachineId, Program};
use takt_sema::{Build, Compiled, Options};

fn options() -> Options {
    Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() }
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

/// 3.2, 3.12: Was die Argumente nicht festlegen, wird explizit
/// instanziiert. Der Rumpf ist einheitenrichtig; scheitern darf nur der
/// Aufruf `zero()`, mit SC-3 und dem Vorschlag der expliziten Form, und
/// `zero[bar]()` uebersetzt.
#[test]
fn unsolvable_unit_variable_asks_for_explicit_instantiation() {
    let program = |call: &str| {
        format!(
            "{HEAD}\
output y : float[bar] @ hw(\"o/y\") with safe = 0 bar

fn zero[U]() -> float[U]:
    return 0.0 U

machine m:
    initial RUN
    state RUN:
        loop:
            y = {call}
"
        )
    };
    let out = diagnose(&program("zero()"));
    let found: Vec<String> = out
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| format!("{} {} | {}", d.code, d.message, d.suggestion.as_deref().unwrap_or_default()))
        .collect();
    assert_eq!(
        found,
        [
            "SC-3 Variable `U` von `zero` ist nicht aus den Argumenten ableitbar | explizit instanziieren: `zero[<U>](…)` (3.12)"
        ],
        "{}",
        messages(&out)
    );
    let p = compile(&program("zero[bar]()"));
    assert!(p.fns.iter().any(|f| f.name == "zero[bar]"), "{:?}", p.fns.iter().map(|f| &f.name).collect::<Vec<_>>());
}

/// Die Tabelle der affinen Einheiten (3.2) mit Werten im Trace: die
/// Differenz zweier Punkte, Punkt plus Vektor, `.to` auf eine skalierte und
/// eine affine Einheit, `degF` ueber `degR`, `min` auf Punkten. `1 bar` in
/// `psi` ist der korrekt gerundete Quotient 100000 / 6894.757293168.
#[test]
fn the_affine_table_of_3_2_holds_in_the_trace() {
    let p = compile(&format!(
        "{HEAD}\
output diff    : float[K]    @ hw(\"o/diff\")    with safe = 0 K
output moved   : float[degC] @ hw(\"o/moved\")   with safe = 0 degC
output pressed : float[psi]  @ hw(\"o/pressed\") with safe = 0 psi
output kelvin  : float[K]    @ hw(\"o/kelvin\")  with safe = 0 K
output rankine : float[degR] @ hw(\"o/rankine\") with safe = 0 degR
output fmoved  : float[degF] @ hw(\"o/fmoved\")  with safe = 0 degF
output lower   : float[degC] @ hw(\"o/lower\")   with safe = 0 degC
output warmer  : bool        @ hw(\"o/warmer\")  with safe = false

machine m:
    initial RUN
    state RUN:
        loop:
            diff = 30 degC - 20 degC
            moved = 20 degC + 5 K
            pressed = (1 bar).to(psi)
            kelvin = (20 degC).to(K)
            rankine = 50 degF - 32 degF
            fmoved = 32 degF + 9 degR
            lower = min(20 degC, 25 degC)
            warmer = 25 degC > 20 degC
"
    ));
    let t = takt_interp::run(
        &p,
        &takt_interp::Trace::default(),
        &takt_interp::RunOptions { ticks: 1, ..Default::default() },
    )
    .expect("Lauf")
    .trace
    .render();
    for line in [
        "t=0 out diff 10.0 K",
        "t=0 out moved 25.0 degC",
        "t=0 out pressed 14.503773773021681 psi",
        "t=0 out kelvin 293.15 K",
        "t=0 out rankine 18.0 degR",
        "t=0 out fmoved 41.0 degF",
        "t=0 out lower 20.0 degC",
        "t=0 out warmer true",
    ] {
        assert!(t.contains(line), "`{line}` fehlt:\n{t}");
    }
}

/// Die Fehlerzeilen der Tabelle (3.2): `degC` als Punkt verbietet Summe,
/// Skalierung, Negation und Betrag; `degF` misst seine Differenzen in
/// `degR`, `degC` in `K`, und die beiden mischen nicht. Eine vordefinierte
/// Einheit, auch mit Praefix, ist nicht neu definierbar.
#[test]
fn the_affine_error_rows_of_3_2_are_errors() {
    let mut wrong = Vec::new();
    for (body, code) in [
        ("const X : float[degC] = 20 degC + 5 degC\n", "SC-3"),
        ("const X : float[degC] = 20 degC * 2\n", "SC-3"),
        ("const X : float[degC] = 2 * (20 degC)\n", "SC-3"),
        // `-60 degC` ist ein Punkt; geklammert und als Konstante ist es das
        // Negative eines Punkts (lower/expr.rs zu FB-184).
        ("const X : float[degC] = -(20 degC)\n", "SC-3"),
        ("const T : float[degC] = 20 degC\nconst X : float[degC] = -T\n", "SC-3"),
        ("const X : float[degC] = abs(20 degC)\n", "SC-3"),
        ("const X : float[degF] = 50 degF + 32 degF\n", "SC-3"),
        ("const X : float[degF] = 50 degF * 2\n", "SC-3"),
        ("const X : float[degF] = 50 degF + 1 K\n", "SC-3"),
        ("const X : float[degC] = 20 degC + 1 degR\n", "SC-3"),
        ("const X : float[K] = 50 degF - 32 degF\n", "SC-3"),
        ("unit mV = 0.001 V\n", "SC-2"),
    ] {
        let out = diagnose(&format!("{HEAD}{body}"));
        if errors(&out) != [code] {
            wrong.push(format!("{body}  {:?} statt {code}: {}", errors(&out), messages(&out)));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
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
        // 9: Fault-Ziel ist der Zustand selbst; der Fall steht in
        // corpus-try/checks/SC-9/bad_self_fault_target.takt.
    ];
    for (body, code) in cases {
        let src = format!("{HEAD}{body}");
        let out = diagnose(&src);
        assert!(errors(&out).contains(code), "erwartet {code} fuer:\n{body}\ngefunden:\n{}", messages(&out));
    }
}

#[test]
fn shadowing_rules_follow_2_5() {
    // Prelude-Namen duerfen verdeckt werden (Warnung SC-2 am verdeckenden
    // Namen); `FAST_PI` und `clamp` stehen im Prelude.
    for (decl, line) in
        [("const FAST_PI : float = 3.0\n", 5), ("fn clamp(x: int, lo: int, hi: int) -> int:\n    return x\n", 5)]
    {
        let src = format!("{HEAD}{decl}machine m:\n    initial RUN\n    state RUN:\n        loop: pass\n");
        let out = diagnose(&src);
        assert!(errors(&out).is_empty(), "{}", messages(&out));
        let map = takt_diag::SourceMap::single("t.takt", src.as_str());
        let warned: Vec<(u32, &str)> = out
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("verdeckt einen Namen der Standardbibliothek"))
            .map(|d| (map.line_col(d.span).0, d.code))
            .collect();
        assert_eq!(warned, [(line, "SC-2")], "{decl}{}", messages(&out));
    }
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
    // Ticks lief, ohne jede Diagnose. Beim kleinsten Tick aus 1.3 sind
    // 12 h 4320000000 Ticks.
    let out = diagnose(
        "system:
    language = 1
    tick = 10 us

output v : bool @ hw(\"o/v\") with safe = false

machine mm every 12 h:
    initial S
    state S:
        loop: pass
",
    );
    assert!(messages(&out).contains("Aktivierungszaehler"), "{}", messages(&out));
}

/// 7.2: Periode und Phase passen in 32 Bit Ticks. Beim kleinsten Tick aus
/// 1.3 (`10 us`) sind 2^32 - 1 Ticks die laengste Periode und Phase,
/// 2^32 Ticks sind SC-3.
#[test]
fn period_and_phase_end_at_two_to_the_32_ticks() {
    let machine = |every: u64, phase: u64| {
        format!(
            "system:
    language = 1
    tick = 10 us

output v : bool @ hw(\"o/v\") with safe = false

machine mm every {} us phase {} us:
    initial S
    state S:
        loop: pass
",
            every * 10,
            phase * 10
        )
    };
    const MAX: u64 = u32::MAX as u64;
    let out = diagnose(&machine(MAX, MAX - 1));
    assert!(errors(&out).is_empty(), "{}", messages(&out));
    for (every, phase, what) in [(MAX + 1, 0, "Periode"), (MAX, MAX + 1, "Phase")] {
        let out = diagnose(&machine(every, phase));
        let found: Vec<String> =
            out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{} {}", d.code, d.message)).collect();
        assert_eq!(
            found,
            [format!(
                "SC-3 {what} {} us sind {} Ticks, mehr als ein Aktivierungszaehler fasst",
                (MAX + 1) * 10,
                MAX + 1
            )],
            "{}",
            messages(&out)
        );
    }
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
    // Festgeschrieben (SEM2-021): Die Frist uebersetzt; aufgerundet auf die
    // Periode laege sie ueber `i64::MAX` und bleibt dort stehen.
    assert!(errors(&out).is_empty(), "{}", messages(&out));
    let rounded: Vec<&str> = out.diagnostics.iter().filter(|d| d.code == "SC-14").map(|d| d.message.as_str()).collect();
    assert_eq!(
        rounded,
        ["9223372036854775807 ns ist kein Vielfaches der Periode; wirkt als 9223372036854775807 ns"],
        "{}",
        messages(&out)
    );

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

/// 1.3: `tick` liegt in 10 us bis 1 s. Die Grenzen gelten, je ein Schritt
/// daneben ist SC-3 mit Vorschlag.
#[test]
fn the_tick_lies_between_ten_microseconds_and_one_second() {
    let program = |tick: &str| {
        format!(
            "system:\n    language = 1\n    tick = {tick}\n\noutput v : bool @ hw(\"o/v\") with safe = false\n\n\
             machine m:\n    initial S\n    state S:\n        loop:\n            v = true\n"
        )
    };
    for tick in ["10 us", "1 s", "1000 ms"] {
        let out = diagnose(&program(tick));
        assert!(errors(&out).is_empty(), "{tick}: {}", messages(&out));
    }
    for (tick, nearest) in [("9 us", "10 us"), ("1 ns", "10 us"), ("1001 ms", "1 s"), ("2 s", "1 s")] {
        let out = diagnose(&program(tick));
        assert_eq!(errors(&out), ["SC-3"], "{tick}: {}", messages(&out));
        let d = out.diagnostics.iter().find(|d| d.is_error()).expect("Fehler");
        assert!(d.message.contains("10 us bis 1 s"), "{tick}: {d}");
        assert!(d.suggestion.as_deref().is_some_and(|s| s.contains(nearest)), "{tick}: {d}");
    }
}

/// 3.4: Ein `safe`-Wert ausserhalb der Range ist genau ein Fehler. Der
/// abgelehnte Wert laesst den Output nicht als `ohne safe` zurueck — das
/// waere eine zweite Meldung fuer dieselbe Ursache (FB-407).
#[test]
fn a_safe_value_outside_its_range_is_one_error() {
    let out = diagnose(&format!(
        "{HEAD}output o : int in 0..9 @ hw(\"o/o\") with safe = 12\nmachine m:\n    initial RUN\n    state RUN:\n        loop:\n            o = 1\n"
    ));
    let found: Vec<String> =
        out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{} {}", d.code, d.message)).collect();
    assert_eq!(found, ["SC-3 Literal ausserhalb der Range"], "{}", messages(&out));
}

/// Ein Programm mit `tick = 10 ms` und dem Systemeintrag `entry`.
fn with_system(entry: &str) -> Compiled {
    diagnose(&format!(
        "system:\n    language = 1\n    tick = 10 ms\n{entry}\noutput v : bool @ hw(\"o/v\") with safe = false\n\n\
         machine m:\n    initial S\n    state S:\n        loop:\n            v = true\n"
    ))
}

/// 7.1: `tick_tolerance = 5 pct for 3 ticks` erlaubt 5 % der Periode ueber
/// drei Ticks; ohne Eintrag gilt `2 pct for 10 ticks`.
#[test]
fn tick_tolerance_reaches_the_configuration() {
    let set = with_system("    tick_tolerance = 5 pct for 3 ticks\n").program.expect("Programm");
    assert_eq!(set.config.tolerance(), (500_000, 3));
    let default = with_system("").program.expect("Programm");
    assert_eq!(default.config.tolerance(), (200_000, 10));
}

/// 3.6, 7.1: `tick_tolerance` verlangt eine Zahl in `pct`. Ohne Einheit
/// oder in einer fremden ist das ein Fehler, nicht still 5 pct.
#[test]
fn tick_tolerance_needs_pct() {
    for value in ["5", "5 bar"] {
        let out = with_system(&format!("    tick_tolerance = {value} for 3 ticks\n"));
        assert_eq!(errors(&out), ["SC-3"], "`{value}`: {}", messages(&out));
    }
}
