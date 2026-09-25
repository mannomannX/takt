//! Das statische Gate (Referenz 3.4, 9.4.3, 11.5; plan/m3.md Abschnitt 6).
//!
//! Geprueft wird an der MIR, nicht am Trace: Eine bewiesene Zuweisung laesst
//! keine Pruefung uebrig, eine unbewiesene schon, und gewarnt wird nur, wo
//! 3.4 es verlangt. Lemma 3.4 prueft `theorems.rs`.

use takt_diag::Policy;
use takt_mir::Program;
use takt_mir::analysis::Report;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";
const OUT: &str = "output n : int in 0..99 @ hw(\"o/n\") with safe = 0\n\n";

/// Uebersetzt und liefert Programm samt Kennzahlen.
fn compile(body: &str) -> (Program, Report, Vec<String>) {
    let src = format!("{HEAD}{OUT}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    let warnings: Vec<String> = out.diagnostics.iter().filter(|d| !d.is_error()).map(|d| format!("{d}")).collect();
    (out.program.expect("Programm"), out.report, warnings)
}

/// Zahl der impliziten Pruefungen einer Ursache.
fn count(r: &Report, cause: &str) -> u32 {
    r.checks.get(cause).copied().unwrap_or(0)
}

#[test]
fn a_provable_assignment_needs_no_check() {
    // 3.4: „Ist das Intervall des Ausdrucks enthalten → keine Pruefung."
    let (_, r, _) = compile(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            var a : int in 0..9 = 5
            var b : int in 0..99 = a + 1
            n = b
",
    );
    assert_eq!(count(&r, "Declared"), 0, "0..9 plus 1 liegt in 0..99: {:?}", r.checks);
}

#[test]
fn an_unprovable_assignment_keeps_its_check() {
    let (_, r, _) = compile(
        "\
machine m:
    var big : int in 0..999 = 500
    initial RUN
    state RUN:
        loop:
            n = big
",
    );
    assert_eq!(count(&r, "Declared"), 1, "0..999 passt nicht in 0..99: {:?}", r.checks);
}

#[test]
fn a_comparison_refines_the_branch() {
    // 3.4: „Vergleiche verfeinern Intervalle in Zweigen."
    let (_, r, _) = compile(
        "\
machine m:
    var big : int in 0..999 = 5
    initial RUN
    state RUN:
        loop:
            if big < 100:
                n = big
",
    );
    assert_eq!(count(&r, "Declared"), 0, "im Zweig ist `big` unter 100: {:?}", r.checks);
}

#[test]
fn a_passed_check_refines_what_follows() {
    // 5.6: ein bestandener `check` gilt fuer das Folgende.
    let (_, r, _) = compile(
        "\
machine m:
    var big : int in 0..999 = 5
    initial RUN
    state RUN:
        loop:
            check big < 100
            n = big
",
    );
    assert_eq!(count(&r, "Declared"), 0, "nach dem `check` ist `big` unter 100: {:?}", r.checks);
}

#[test]
fn only_a_check_in_a_loop_warns() {
    // 3.4, Warnpolitik: „Warnungen im engeren Sinn entstehen nur bei
    // impliziten Pruefungen in `for`-Schleifen und in Aktionsbloecken."
    let (_, outside, w_outside) = compile(
        "\
machine m:
    var big : int in 0..999 = 500
    initial RUN
    state RUN:
        loop:
            n = big
",
    );
    assert_eq!(outside.warned, 0, "ausserhalb einer Schleife wird nicht gewarnt");
    assert!(!w_outside.iter().any(|w| w.contains("SC-24")), "{w_outside:?}");

    let (_, inside, w_inside) = compile(
        "\
machine m:
    var big : int in 0..999 = 500
    initial RUN
    state RUN:
        loop:
            for i in range(3):
                n = big
",
    );
    assert_eq!(inside.warned, 1, "in der Schleife wird gewarnt");
    assert!(w_inside.iter().any(|w| w.contains("SC-24")), "{w_inside:?}");
}

#[test]
fn the_metric_names_the_cause() {
    // 3.4: die Kennzahl ist nach Ursache aufgeschluesselt (FB-19).
    let (_, r, _) = compile(
        "\
machine m:
    var big : int in 0..999 = 500
    initial RUN
    state RUN:
        loop:
            n = big
",
    );
    for cause in ["Declared", "Index", "Convert", "Arith"] {
        assert!(r.checks.contains_key(cause), "`{cause}` fehlt in der Kennzahl: {:?}", r.checks);
    }
    let text = r.lines().join("\n");
    assert!(text.contains("Declared 1"), "die Ursache steht im Report: {text}");
}

#[test]
fn a_proven_expression_is_narrowed_to_i32() {
    // 3.4, Lemma 3.4: „32 Bit, wo moeglich".
    let (_, r, _) = compile(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            var a : int in 0..9 = 5
            n = a
",
    );
    assert!(r.narrowed > 0, "etwas wurde verengt: {} von {}", r.narrowed, r.integer_exprs);
    assert_eq!(r.narrowed, r.integer_exprs, "hier ist alles beweisbar schmal");
}

/// 3.4: `|` und `^` nicht negativer Operanden haben hoechstens so viele
/// Bits wie der groessere, und der Ausdruck bleibt in 32 Bit. Ohne die
/// Regel galt ihr Ergebnis als unbeschraenkt, und `takt bench` zaehlte die
/// Operation als `i64` (FB-291).
#[test]
fn bitwise_or_and_xor_keep_the_bits_of_their_operands() {
    let (_, r, _) = compile(
        "\
machine m:
    var a : int in 0..65535 = 1
    var b : int in 0..65535 = 7
    initial RUN
    state RUN:
        loop:
            a = ((a * 181) ^ (b >> (a & 7))) & 65535
            b = (b | (a * 3)) & 65535
",
    );
    assert_eq!(r.narrowed, r.integer_exprs, "alles beweisbar schmal: {} von {}", r.narrowed, r.integer_exprs);
}

#[test]
fn every_machine_gets_a_cost_budget() {
    // 9.4.3: `B_m` und `F_m` als Vektoren ueber sieben Klassen.
    let (p, _, _) = compile(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            var a : int in 0..9 = 5
            n = a + 1
",
    );
    let m = &p.machines[0];
    let b = m.budget.expect("Budget aus M3");
    assert!(b.activation.i32 > 0 || b.activation.i64 > 0, "die Addition zaehlt: {:?}", b.activation);
}

/// **Division hat eigene Gewichte** (7.2): Eine Division und ein Rest
/// zaehlen in ihrer Klasse und dazu als Division; die LU-Zerlegung einer
/// 2×2-Inversen teilt einmal, das Einsetzen je Spalte zweimal.
#[test]
fn a_division_counts_in_its_class_and_as_a_division() {
    let (p, _, _) = compile(
        "\
machine m:
    var x : float = 1.5
    initial RUN
    state RUN:
        loop:
            var a : int in 0..9 = 5
            n = a / 3 + a % 4
            x = x / 2.0
",
    );
    let b = p.machines[0].budget.expect("Budget").activation;
    assert_eq!(b.i32_div + b.i64_div, 2, "eine Division, ein Rest: {b:?}");
    assert_eq!(b.f64_div, 1, "{b:?}");
    assert!(b.i32 + b.i64 >= 3, "die Addition dazu: {b:?}");

    let (p, _, _) = compile(
        "\
machine m:
    var s : mat<2, 2> = [[2.0, 1.0], [1.0, 3.0]]
    initial RUN
    state RUN:
        loop:
            s = s.inv()
",
    );
    let b = p.machines[0].budget.expect("Budget").activation;
    assert_eq!(b.f64_div, 1 + 2 * 2, "LU n(n-1)/2, Einsetzen n je Spalte: {b:?}");
    assert_eq!(b.f64_fma, 1 + 2 * 2, "LU eine Elimination, Einsetzen n(n-1) je Spalte: {b:?}");
    assert!(b.f64 >= b.f64_div + b.f64_fma, "{b:?}");
}

/// **Ein Aufruf kostet seinen Rumpf** (9.4.3: `N(f(args)) = Σ cost(args) +
/// N(body f)`). Zwei Aufrufe einer Funktion mit einer Schleife ueber vier
/// Runden zu je drei Operationen und dem Schleifenzaehler (zwei), dazu die
/// Eins der Schleife: `2 · (1 + 4 · (3 + 2)) = 42` Operationen und zwei
/// Aufrufe — nicht nur die zwei Aufrufe, wie bis FB-278. Dazu kommt die
/// Maschinerie der Aktivierung: der Aufruf des `loop:` (eine Operation
/// und ein Aufruf) und das Verteilen ueber `conf` (zwei Operationen).
#[test]
fn a_call_costs_its_body() {
    let (p, _, _) = compile(
        "\
fn mix(x: int in 0..65535, y: int in 0..65535) -> int in 0..65535:
    var r : int in 0..65535 = x
    for i in range(4):
        r = (r * 181 + y) & 65535
    return r

machine m:
    var a : int in 0..65535 = 1
    initial RUN
    state RUN:
        loop:
            a = mix(a, 7)
            a = mix(a, 9)
",
    );
    let b = p.machines[0].budget.expect("Budget").activation;
    assert_eq!(b.call, 3, "zwei Aufrufe von `mix`, einer des `loop:`: {b:?}");
    assert_eq!(b.i32, 45, "vier Runden zu drei Operationen und dem Zaehler, zweimal, dazu drei: {b:?}");
    let f = p.fns.iter().find(|f| f.name == "mix").expect("mix");
    assert_eq!(f.cost.map(|c| c.i32), Some(21), "der Rumpf steht in `Fn::cost`");
}

/// Die Kosten eines Zustands im Bericht (FB-281).
fn state_cost(p: &Program, name: &str) -> takt_mir::fns::CostVec {
    let r = takt_mir::analysis::budget::report(p);
    r.machines[0].states.iter().find(|s| s.name == name).map(|s| s.cost).expect("Zustand")
}

/// **Ein Wechsel kostet im Tick, in dem er geschieht** (9.3, 5.2 Regel 3
/// und 4): Der Tick, der `BUSY` betritt, fuehrt dessen `enter:` und
/// `loop:` im Modus ENTRY aus. `IDLE` rechnet selbst kein Gleitkomma —
/// sein teuerster Tick schon.
#[test]
fn a_transition_tick_costs_the_entry_of_its_target() {
    let (p, _, _) = compile(
        "\
machine m:
    var a : int in 0..99 = 0
    var x : float = 0.0
    initial IDLE
    state IDLE:
        loop:
            n = 0
        when a < 99: -> BUSY
    state BUSY:
        enter:
            x = x * 1.5 + 0.25
        loop:
            x = x * 0.5
            n = 1
        when a > 50: -> IDLE
        exit:
            x = x + 1.0
",
    );
    let (idle, busy) = (state_cost(&p, "IDLE"), state_cost(&p, "BUSY"));
    assert!(idle.f64 > 0, "der Tick hinein traegt `enter:` und `loop:` von BUSY: {idle:?}");
    assert!(busy.f64 > 0, "der Tick hinaus traegt `loop:` und `exit:`: {busy:?}");
    let b = p.machines[0].budget.expect("Budget").activation;
    assert!(b.f64 >= idle.f64.max(busy.f64), "{b:?}");
}

/// **Eine Kette kostet alle ihre Ebenen** (9.3 `exec_chain`): Der innere
/// Zustand zahlt den `loop:` des aeusseren mit.
#[test]
fn a_nested_state_costs_its_whole_chain() {
    let (p, _, _) = compile(
        "\
machine m:
    var a : int in 0..99 = 0
    var x : float = 0.0
    initial OUTER
    state OUTER:
        initial INNER
        loop:
            x = x * 0.5
        state INNER:
            loop:
                a = (a + 1) % 50
                n = 0
",
    );
    let inner = state_cost(&p, "INNER");
    assert!(inner.f64 > 0 && inner.i32 + inner.i64 > 0, "beide Ebenen: {inner:?}");
}

/// **Der Fault-Pfad betritt das Fault-Ziel** (9.4.3 `F_m`, 5.2 Regel 5):
/// `enter:` von SAFE laeuft im Tick des Faults und steht darum in `F_m`.
#[test]
fn the_fault_path_enters_the_fault_target() {
    let (p, _, _) = compile(
        "\
machine m:
    fault -> SAFE
    var a : int in 0..9 = 1
    var x : float = 0.0
    initial RUN
    state RUN:
        loop:
            check a < 9, \"zu gross\"
            n = a
    state SAFE:
        enter:
            x = x * 0.5
        loop:
            n = 0
",
    );
    let b = p.machines[0].budget.expect("Budget");
    assert!(b.fault_path.f64 > 0, "SAFE.enter steht im Fault-Pfad: {:?}", b.fault_path);
}

/// **Ein Handler laeuft je Element des Fensters** (9.4.3 `N(dispatch(st))
/// = CAP · (…)`): Die Kosten wachsen linear mit der Kapazitaet.
#[test]
fn a_handler_runs_once_per_element_of_the_window() {
    let with = |capacity: u32| {
        let (p, _, _) = compile(&format!(
            "\
input rx : stream<line<16>> @ hw(\"rx\") with max_rate = 1000 Hz, capacity = {capacity}

machine m:
    var k : int in 0..999 = 0
    initial RUN
    state RUN:
        loop:
            n = 0
        on rx as l:
            k = (k + 1) % 1000
"
        ));
        p.machines[0].budget.expect("Budget").activation
    };
    let (a, b, c) = (with(8), with(16), with(24));
    assert!(b.mem > a.mem && b.i32 > a.i32, "mehr Fenster, mehr Arbeit: {a:?} {b:?}");
    assert_eq!(b.mem - a.mem, c.mem - b.mem, "linear in CAP");
    assert_eq!(b.i32 - a.i32, c.i32 - b.i32, "linear in CAP");
}

/// **`send` kostet seinen Text** (9.4.3 `N(send o, e) = cost(e) +
/// len_max(e)`): acht Zeichen mehr, mindestens acht Bytes mehr.
#[test]
fn a_send_costs_its_text() {
    let with = |text: &str| {
        let (p, _, _) = compile(&format!(
            "\
output tx : stream<line<32>> @ hw(\"tx\") with max_rate = 1000 Hz, capacity = 64

machine m:
    var a : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            send tx, \"{text}\"
            n = 0
"
        ));
        p.machines[0].budget.expect("Budget").activation
    };
    let (short, long) = (with("T{a}"), with("T{a}abcdefgh"));
    assert!(long.mem >= short.mem + 8, "{short:?} {long:?}");
}

/// **`has` sucht an jeder Stelle** (8.7, `step::text_has`): teurer als
/// `matches`, das den Text einmal liest.
#[test]
fn has_costs_more_than_matches() {
    let with = |kind: &str| {
        let (p, _, _) = compile(&format!(
            "\
input rx : stream<line<32>> @ hw(\"rx\") with max_rate = 1000 Hz, capacity = 4

machine m:
    var k : int in 0..999 = 0
    initial RUN
    state RUN:
        loop:
            n = 0
        on rx {kind} \"OK\":
            k = (k + 1) % 1000
"
        ));
        p.machines[0].budget.expect("Budget").activation
    };
    let (matches, has) = (with("matches"), with("has"));
    assert!(has.mem > matches.mem, "{matches:?} {has:?}");
}

/// **Jeder Waechter der Kette zaehlt** (9.3: `trans(C)` wird ausgewertet,
/// bis einer zutrifft — im schlimmsten Fall keiner).
#[test]
fn every_guard_of_the_chain_counts() {
    let with = |guards: &str| {
        let (p, _, _) = compile(&format!(
            "\
machine m:
    var x : float = 0.5
    initial RUN
    state RUN:
        loop:
            n = 0
{guards}
    state DONE:
        loop:
            n = 1
"
        ));
        state_cost(&p, "RUN")
    };
    let one = with("        when x * 2.0 > 1.0: -> DONE");
    let two = with("        when x * 2.0 > 1.0: -> DONE\n        when x * 3.0 > 2.0: -> DONE");
    assert!(two.f64 > one.f64, "der zweite Waechter rechnet mit: {one:?} {two:?}");
}

/// Der Schritt einer Blockinstanz kostet seinen Rumpf (5.7, 9.4.3).
#[test]
fn a_block_step_costs_its_body() {
    let (p, _, _) = compile(
        "\
machine m:
    var lp = lowpass[bar](tau = 100 ms)
    var y : float[bar] = 0 bar
    initial RUN
    state RUN:
        loop:
            y = lp.step(1 bar, tick)
",
    );
    let b = p.machines[0].budget.expect("Budget").activation;
    let step = p.blocks.iter().find(|d| d.name.starts_with("lowpass")).and_then(|d| d.step).expect("step");
    let body = p.fns[step.index()].cost.expect("Kosten des Schritts");
    assert!(body.f64 > 0, "der Filter rechnet: {body:?}");
    assert!(b.f64 >= body.f64 && b.call >= 1, "die Aktivierung traegt den Schritt: {b:?}");
}

#[test]
fn the_memory_report_separates_reliable_from_open() {
    // 11.5: jeder Posten traegt seine Herkunft, summiert wird nur
    // Belastbares.
    let (p, _, _) = compile(
        "\
machine m:
    var x : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            n = x
",
    );
    let s = takt_mir::analysis::size::size(&p);
    assert!(s.total() > 0, "der Maschinenzustand zaehlt");
    assert!(s.has_open(), "Profilreserven fehlen noch (8.10, 13.8)");
    let text = s.lines().join("\n");
    assert!(text.contains("exakt") && text.contains("offen"), "beide Herkuenfte stehen da: {text}");
}

#[test]
fn the_cost_budget_separates_activation_from_fault_path() {
    // Pruefung 12 (9.4.3): `B_m` je Aktivierung und `F_m` je Fault-Pfad sind
    // getrennte Vektoren — die Abort-Phase (5.4) summiert nur die zweiten.
    let (p, _, _) = compile(
        "\
machine m:
    fault -> SAFE
    var a : int in 0..9 = 1
    initial RUN
    state RUN:
        loop:
            a = a
            check a < 9, \"zu gross\"
            n = a
    state SAFE:
        loop:
            n = 0
",
    );
    let b = p.machines[0].budget.expect("Budget aus M3");
    assert!(b.activation.i32 + b.activation.i64 > 0, "die Aktivierung kostet: {:?}", b.activation);
    // Der Fault-Pfad fuehrt seine eigenen Aufrufe: je Fault den Hook der
    // Runtime (RUN nach SAFE, SAFE nach `FAULTED`) und den Entry-Tick in
    // SAFE — nicht die der Aktivierung.
    assert_eq!(b.fault_path.call, 3, "der Fault-Pfad ist eigenstaendig: {:?}", b.fault_path);
}

#[test]
fn a_loop_multiplies_the_budget() {
    // 9.4.3: die statische Schranke einer `for`-Schleife geht in das Budget
    // ein — sonst waere es keine obere Schranke.
    let (one, _, _) = compile(
        "\
machine m:
    var a : int in 0..9 = 1
    initial RUN
    state RUN:
        loop:
            a = a + 1
            n = a
",
    );
    let (many, _, _) = compile(
        "\
machine m:
    var a : int in 0..9 = 1
    initial RUN
    state RUN:
        loop:
            for i in range(4):
                a = a + 1
            n = a
",
    );
    let (b1, b4) = (one.machines[0].budget.expect("B"), many.machines[0].budget.expect("B"));
    let sum = |c: takt_mir::fns::CostVec| c.i32 + c.i64;
    assert!(sum(b4.activation) > sum(b1.activation), "vier Durchlaeufe kosten mehr: {:?} vs {:?}", b4, b1);
}

#[test]
fn a_sequence_var_always_carries_an_initialiser() {
    // Pruefung 25 (Definite Assignment je Eintritt) hat heute keinen eigenen
    // Fall: Die Grammatik verlangt an jedem `var` ein `=` (2.3, `var_decl`),
    // also ist eine gehobene Variable nie uninitialisiert. Faellt diese
    // Schranke, muss 25 einen Korpus bekommen — dann schlaegt dieser Test
    // fehl und erinnert daran.
    let src = format!(
        "{HEAD}{OUT}{}",
        "machine m:
    initial RUN
    state RUN:
        sequence:
            var k : int in 0..9
            step \"setzen\":
                k = 3
            step \"lesen\":
                n = k
            -> RUN
"
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let codes: Vec<&str> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| d.code).collect();
    assert!(codes.contains(&"P"), "ein `var` ohne Initialisierer ist ein Syntaxfehler, kein SC-25: {codes:?}");
}

/// SC-25 (6.2): Eine Zuweisung in einem verschachtelten Block zaehlt.
///
/// **Der Fall, der falsch meldete.** `if c: var k = 7; y = k` steckt
/// Deklaration und Nutzung in *eine* Anweisung. Die Pruefung suchte alle
/// Lesungen einer Anweisung rekursiv, merkte aber nur Zuweisungen der
/// obersten Ebene vor — und meldete damit die eigene Initialisierung als
/// fehlend (FB-123, gefunden am Radio-Treiber in `feedback/`).
///
/// Sie laeuft jetzt verschraenkt: je Anweisung erst ihre eigenen
/// Ausdruecke, dann ihre Zuweisung, dann die Bloecke darunter.
#[test]
fn an_assignment_inside_a_branch_counts_as_assignment() {
    let src = format!(
        "{HEAD}{OUT}{}",
        "machine m:
    var flag : bool = true

    initial RUN

    state RUN:
        sequence:
            wait 10 ms
            if flag:
                var k = 7
                n = k
            -> RUN
"
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let codes: Vec<&str> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| d.code).collect();
    assert!(
        !codes.contains(&"SC-25"),
        "die Zuweisung steht unmittelbar ueber der Lesung; SC-25 darf nicht melden: {codes:?}"
    );
    assert!(codes.is_empty(), "unerwartete Fehler: {codes:?}");
}

#[test]
fn an_error_inside_a_format_string_points_at_the_placeholder() {
    // FB-31: Die Spans eines Platzhalter-Ausdrucks zaehlen ab dem Platzhalter,
    // nicht ab dem Dateianfang. Ohne Verschiebung meldete jeder Namensfehler
    // in `log "… {x}"` die Stelle 1:1 — in einer langen Datei unbrauchbar.
    let src = format!(
        "{HEAD}{OUT}{}",
        "machine m:
    var zaehler : int in 0..99 = 0
    initial RUN
    state RUN:
        enter:
            log \"start {zaehlr}\"
        loop:
            n = zaehler
"
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let d = out.diagnostics.iter().find(|d| d.code == "SC-2").expect("der Tippfehler wird gemeldet");
    let at = src[d.span.start as usize..d.span.end as usize].to_string();
    assert_eq!(at, "zaehlr", "der Span zeigt genau auf den Namen im Platzhalter");
}

#[test]
fn append_costs_the_capacity_of_its_source() {
    // 9.4.3: `append` kopiert, die obere Schranke ist die Kapazitaet der
    // Quelle — nicht ihre aktuelle Laenge, die statisch niemand kennt.
    let small = compile(
        "\
machine m:
    var a : bytes<4> = default
    var b : bytes<512> = default
    initial RUN
    state RUN:
        loop:
            b.append(a)
            n = 0
",
    )
    .0;
    let large = compile(
        "\
machine m:
    var a : bytes<256> = default
    var b : bytes<512> = default
    initial RUN
    state RUN:
        loop:
            b.append(a)
            n = 0
",
    )
    .0;
    let (s, l) = (small.machines[0].budget.expect("B"), large.machines[0].budget.expect("B"));
    assert!(
        l.activation.mem > s.activation.mem,
        "die groessere Quelle kostet mehr: {} vs {}",
        l.activation.mem,
        s.activation.mem
    );
    assert_eq!(l.activation.mem - s.activation.mem, 252, "genau die Differenz der Kapazitaeten");
}

#[test]
fn size_lists_the_dfa_tables_as_open_until_codegen() {
    // 11.5 zaehlt die DFA-Tabellen der Muster zu den Posten. Die Rechnung
    // steht, aber gefuellt werden die Tabellen erst vom Codegen — der
    // Interpreter gleicht direkt ab (plan/m2.md 1.1). Der Posten steht
    // darum auf `offen`: eine Null, die noch niemand gerechnet hat, ist
    // kein Messwert. Schlaegt der Test fehl, weil er nun `exakt` ist, hat
    // der Codegen die Tabellen gebaut — dann ist das die richtige Meldung.
    let (p, _, _) = compile(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            n = 0
",
    );
    let text = takt_mir::analysis::size::size(&p).lines().join("\n");
    let line = text.lines().find(|l| l.contains("DFA")).expect("der Posten steht in der Liste");
    assert!(line.contains("offen"), "ohne Codegen ist er offen: {line}");
}

#[test]
fn the_alert_polarity_lint_only_fires_on_the_same_comparison() {
    // Pruefung 63a (5.6): Ein `alert` nennt das zu meldende Ereignis, ein
    // `check` die einzuhaltende Invariante. Dieselbe Vergleichsrichtung in
    // beiden ist der beweisbare Fehlerfall; die entgegengesetzte ist der
    // dokumentierte Normalfall und muss schweigen.
    let same = warnings_of(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            check n < 50, \"zu gross\"
            alert n < 50, \"waechst\"
            n = 0
",
    );
    assert!(same.iter().any(|w| w.contains("SC-63")), "gleiche Richtung warnt: {same:?}");

    let opposite = warnings_of(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            check n < 50, \"zu gross\"
            alert n > 50, \"waechst\"
            n = 0
",
    );
    assert!(!opposite.iter().any(|w| w.contains("SC-63")), "die richtige Polaritaet schweigt: {opposite:?}");
}

/// Die Warnungen eines Maschinenrumpfs als Text.
fn warnings_of(body: &str) -> Vec<String> {
    let src = format!("{HEAD}{OUT}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    takt_sema::compile(&src, &options).diagnostics.iter().map(|d| format!("{d}")).collect()
}

#[test]
fn the_protocol_case_keeps_its_implicit_checks_low() {
    // FB-24 und plan/m3.md 1.9: Die Kennzahl (3.4) wurde bis M3 nur an
    // steuerungsnahen Dateien gemessen — die Haelfte, die laut beiden
    // Praxisberichten keine Probleme macht. `13_protocol_analysis.takt` ist
    // der Messfall der Protokollhaelfte: Dekodierung, Slice ueber eine
    // dekodierte Laenge, CRC-Schleife und Bitfelder.
    //
    // Der Befund ist das eigentliche Ergebnis: Der UART-Bericht befuerchtet
    // eine hohe Zahl, die Analyse beweist das Gegenteil. Der Slice ueber
    // `h.len` braucht keine Pruefung, weil das Laengenfeld `u8 in 0..64`
    // deklariert und die Schranke vorher geprueft ist.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/13_protocol_analysis.takt");
    let src = std::fs::read_to_string(path).expect("Messfall lesbar");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<&str> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| d.code).collect();
    assert!(errors.is_empty(), "der Messfall uebersetzt: {errors:?}");

    let r = out.report;
    assert!(r.total_checks() <= 4, "Protokollcode bleibt unter vier impliziten Pruefungen: {:?}", r.checks);
    // `b[from + i]` in der CRC-Schleife ist relational (`from + i < len`);
    // das beweist erst ein Oktagon (plan/m6.md 2.12).
    assert_eq!(r.warned, 1, "nur der Ring-Index der CRC-Schleife steht in einer Schleife: {:?}", r.checks);
    assert!(r.narrowed > 0, "die Verengung greift auch hier: {} von {}", r.narrowed, r.integer_exprs);
}

#[test]
fn an_uncurated_math_function_is_rejected_before_the_run() {
    // 13.8: Eine Funktion kommt in die kuratierte Menge, *nachdem* ihre
    // Bit-Gleichheit belegt ist. `sin` ist es noch nicht — und eine Zahl
    // aus der Plattformbibliothek waere auf einem anderen Target eine
    // andere, also behauptete sie eine Zusage, die sie nicht haelt
    // (Satz 9.4.4). Der Compiler sagt es vor dem Lauf.
    let src = format!(
        "{HEAD}{OUT}{}",
        "\
machine m:
    var x : float = 0.0
    initial RUN
    state RUN:
        loop:
            x = sin(1.0)
            n = 0
"
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let text: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(text.iter().any(|t| t.contains("korrekt gerundet")), "`sin` wird abgelehnt: {text:?}");

    // `sqrt` und `fma` sind kuratiert (Stufe 1) und laufen.
    let src = format!(
        "{HEAD}{OUT}{}",
        "\
machine m:
    var x : float = 0.0
    initial RUN
    state RUN:
        loop:
            x = sqrt(2.0) + fma(1.0, 2.0, 3.0)
            n = 0
"
    );
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<&str> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| d.code).collect();
    assert!(errors.is_empty(), "`sqrt` und `fma` sind kuratiert: {errors:?}");
}

/// 12.3: Die Stacktiefe ist der laengste Pfad im Aufrufgraphen.
///
/// Nicht die Summe aller Rahmen und nicht der tiefste einzelne: Zwei
/// Funktionen, die sich nicht rufen, liegen nie gleichzeitig auf dem
/// Stack. Hier ruft `outer` beide Zweige, und nur der teurere zaehlt.
#[test]
fn the_stack_depth_is_the_longest_path_not_the_sum() {
    let (p, _, _) = compile(
        "\
fn leaf_small(x: int) -> int:
    return x + 1

fn leaf_big(x: int) -> int:
    return x * 2

fn outer(x: int) -> int:
    return leaf_small(x) + leaf_big(x)

machine m:
    initial RUN
    state RUN:
        loop:
            n = outer(1)
",
    );
    let by_name = |name: &str| p.fns.iter().position(|f| f.name == name).expect(name);
    let mut frames = vec![Some(0u32); p.fns.len()];
    frames[by_name("outer")] = Some(32);
    frames[by_name("leaf_small")] = Some(8);
    frames[by_name("leaf_big")] = Some(48);

    let d = takt_mir::analysis::stack::depth(&p, &frames, &[]).expect("Tiefe");
    assert_eq!(d.bytes, 32 + 48, "der teurere Zweig zaehlt, nicht beide");
    assert_eq!(d.path.first().map(String::as_str), Some("outer"));
    assert!(d.path.iter().any(|f| f == "leaf_big"), "der Pfad nennt den teuren Zweig: {:?}", d.path);

    // 12.3: Der Schritt der Maschine und ihre Schleifenfunktion liegen
    // unter dem Pfad; der Bericht nennt die ganze Kette.
    let machines = [takt_mir::analysis::stack::MachineFrames { step: Some(100), inner: Some(("m_loop_0".into(), 16)) }];
    let d = takt_mir::analysis::stack::depth(&p, &frames, &machines).expect("Tiefe");
    assert_eq!(d.bytes, 100 + 16 + 32 + 48);
    assert_eq!(d.path, ["m_step", "m_loop_0", "outer", "leaf_big"]);
    let unmeasured = [takt_mir::analysis::stack::MachineFrames::default()];
    assert!(takt_mir::analysis::stack::depth(&p, &frames, &unmeasured).is_none(), "ohne Schrittrahmen keine Schranke");
}

/// Eine erreichbare Funktion ohne gemessenen Rahmen macht die Rechnung
/// unbekannt.
///
/// Eine Schranke mit einer Luecke waere keine — lieber kein Wert als
/// einer, dem man nicht ansieht, dass ihm etwas fehlt (11.5).
#[test]
fn an_unmeasured_frame_leaves_the_depth_unknown() {
    let (p, _, _) = compile(
        "\
fn helper(x: int) -> int:
    return x + 1

machine m:
    initial RUN
    state RUN:
        loop:
            n = helper(1)
",
    );
    let frames = vec![None; p.fns.len()];
    assert!(takt_mir::analysis::stack::depth(&p, &frames, &[]).is_none(), "ohne Messung keine Schranke");
}

/// 9.4.3: `B_m` ist der zustandsfreie Anteil plus das komponentenweise
/// Maximum ueber die Zustaende — nicht ueber die Summe.
///
/// Die Feinheit: Das Maximum ist *je Klasse*. Zwei Zustaende, von denen
/// einer `i32` und der andere `f32` treibt, liefern beide ihren Wert an
/// `B_m` — sie schliessen einander aus, also ist jede Klasse einzeln
/// erreichbar.
#[test]
fn the_budget_takes_the_peak_per_class_not_per_state() {
    let (p, _, _) = compile(
        "\
machine m:
    var a : int in 0..99 = 0
    var x : float = 0.0
    initial ONE

    state ONE:
        loop:
            a = a + 1
            n = 0
        when a > 50: -> TWO

    state TWO:
        loop:
            x = x + 1.5
            n = 1
        when a > 50: -> ONE
",
    );
    let r = takt_mir::analysis::budget::report(&p);
    let m = r.machines.iter().find(|m| m.name == "m").expect("Maschine");

    let one = m.states.iter().find(|s| s.name == "ONE").expect("ONE");
    let two = m.states.iter().find(|s| s.name == "TWO").expect("TWO");
    assert!(one.cost.i32 > 0, "ONE rechnet mit Integern");
    assert!(two.cost.f64 > 0 || two.cost.f32 > 0, "TWO rechnet mit Gleitkomma");

    // Beide Klassen stehen in B_m — das Maximum ist komponentenweise.
    assert!(m.activation.i32 >= one.cost.i32, "die i32-Spitze aus ONE steht in B_m");
    assert!(m.activation.f64 >= two.cost.f64, "die f64-Spitze aus TWO steht in B_m");

    // Und jede Klasse nennt ihren Zustand.
    assert!(one.drives.iter().any(|c| c.name() == "i32"), "ONE treibt i32: {:?}", one.drives);
}

/// Der Bericht nennt, was ihm fehlt.
///
/// Operationen sind keine Zeit; die Umrechnung braucht `c_target` aus der
/// Messung (13.8). Ein Bericht, der das verschweigt, laedt dazu ein, die
/// Zahlen fuer Mikrosekunden zu halten.
#[test]
fn the_cost_report_says_what_it_cannot_tell() {
    let (p, _, _) = compile(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            n = 1
",
    );
    let lines = takt_mir::analysis::budget::report(&p).lines().join("\n");
    assert!(lines.contains("c_target"), "der Bericht nennt die fehlende Kalibrierung:\n{lines}");
    assert!(lines.contains("13.8"), "mit Fundstelle:\n{lines}");
}

/// **SC-12 sagt, warum es nicht urteilen kann** (9.4.3, 7.2; FB-136).
///
/// Pruefung 12 verlangt Kostenbudget und Schedulability. Beide brauchen
/// `c_target` aus der Kalibrierung (13.8), und bis die vorliegt, kann
/// niemand entscheiden. Bis hierher schwieg die Pruefung darum ganz — und
/// ein Nutzer konnte nicht unterscheiden, ob sein Budget geprueft wurde
/// oder ob es die Pruefung gar nicht gibt.
///
/// Gemeldet wird nur bei `wcet`: `ram` prueft SC-62 ohne Kalibrierung,
/// und ein Hinweis dort waere falsch. Liegt eine Kalibrierung vor,
/// urteilt SC-12 statt zu melden — das prueft `tests/calibrated.rs`.
#[test]
fn a_declared_wcet_learns_why_it_cannot_be_judged() {
    let (_, _, warnings) = compile(
        "\
machine m with budget = {wcet = 1 ms}:
    initial S

    state S:
        enter:
            n = 1
",
    );
    let hint = warnings.iter().find(|w| w.contains("SC-12")).unwrap_or_else(|| {
        panic!("SC-12 meldet sich nicht; gefunden:\n{}", warnings.join("\n"));
    });
    assert!(hint.contains("c_target"), "die Meldung nennt die fehlende Eingabe: {hint}");
}

/// Ohne deklariertes `wcet` schweigt SC-12.
///
/// Wer keine Rechenzeit nennt, hat nichts erwartet; ein Hinweis auf eine
/// fehlende Pruefung waere dort Rauschen. Ein `ram`-Budget prueft SC-62
/// ohne Kalibrierung und braucht den Hinweis darum nicht. 3.4 haelt
/// dieselbe Regel fuer die Performance-Lints fest — gewarnt wird, wo
/// jemand eine Zusage gemacht hat.
#[test]
fn without_a_declared_wcet_check_twelve_stays_quiet() {
    let (_, _, warnings) = compile(
        "\
machine m with budget = {ram = 256}:
    initial S

    state S:
        enter:
            n = 1
",
    );
    assert!(!warnings.iter().any(|w| w.contains("SC-12")), "SC-12 meldet sich ungefragt:\n{}", warnings.join("\n"));
}

#[test]
fn a_break_under_or_refines_the_rest_of_the_loop() {
    // De Morgan: hinter `if a or b: break` gilt `not a and not b`.
    let (_, r, _) = compile(
        "machine m:
    var room : int in 0..128 = 100
    initial RUN
    state RUN:
        loop:
            var sent : int in 0..128 = 0
            for i in range(200):
                if sent >= room or i > 150:
                    break
                sent = sent + 1
            n = sent % 100
",
    );
    assert_eq!(count(&r, "Declared"), 0, "sent < room <= 128, also passt sent + 1: {:?}", r.checks);
}

#[test]
fn the_length_of_a_collection_lies_within_its_capacity() {
    let (_, r, _) = compile(
        "machine m:
    var s : bytes<16> = default
    initial RUN
    state RUN:
        loop:
            var l : int in 0..16 = s.len
            n = l
",
    );
    assert_eq!(count(&r, "Declared"), 0, "`len` liegt in 0..16: {:?}", r.checks);
}

#[test]
fn a_conversion_is_checked_only_where_the_source_can_exceed_the_target() {
    let (_, fits, _) = compile(
        "machine m:
    var c : int in 1..255 = 7
    initial RUN
    state RUN:
        loop:
            var b : u8 = c as u8
            n = (b as int) % 100
",
    );
    assert_eq!(count(&fits, "Convert"), 0, "1..255 passt in u8: {:?}", fits.checks);
    let (_, wide, _) = compile(
        "machine m:
    var w : int in 0..1000 = 7
    initial RUN
    state RUN:
        loop:
            var b : u8 = w as u8
            n = (b as int) % 100
",
    );
    assert_eq!(count(&wide, "Convert"), 1, "0..1000 passt nicht: {:?}", wide.checks);
}

#[test]
fn a_shift_amount_is_checked_only_outside_the_width() {
    let body = |range: &str| {
        format!(
            "machine m:
    var k : int in {range} = 3
    var s : u16 = 1
    initial RUN
    state RUN:
        loop:
            s = s << k
            n = (s >> 9) as int
"
        )
    };
    let (_, fits, _) = compile(&body("0..15"));
    assert_eq!(count(&fits, "Arith"), 0, "0..15 ist ein gueltiger Betrag: {:?}", fits.checks);
    let (_, wide, _) = compile(&body("0..20"));
    assert_eq!(count(&wide, "Arith"), 1, "20 ist keiner: {:?}", wide.checks);
}

#[test]
fn a_divisor_is_checked_only_where_it_can_be_zero() {
    let body = |range: &str| {
        format!(
            "machine m:
    var d : int in {range} = 2
    initial RUN
    state RUN:
        loop:
            n = 50 / d
"
        )
    };
    let (_, safe, _) = compile(&body("1..10"));
    assert_eq!(count(&safe, "Arith"), 0, "1..10 enthaelt keine Null: {:?}", safe.checks);
    let (_, risky, _) = compile(&body("0..10"));
    assert_eq!(count(&risky, "Arith"), 1, "0..10 enthaelt sie: {:?}", risky.checks);
}

#[test]
fn an_overflow_is_checked_in_narrow_types_and_never_warned_in_wide_ones() {
    let (_, narrow, w_narrow) = compile(
        "machine m:
    var a : u8 = 200
    initial RUN
    state RUN:
        loop:
            for i in range(3):
                a = a + 1
            n = (a as int) % 100
",
    );
    assert_eq!(count(&narrow, "Arith"), 1, "u8 + 1 kann ueberlaufen: {:?}", narrow.checks);
    assert!(w_narrow.iter().any(|w| w.contains("SC-24")), "in der Schleife warnt es: {w_narrow:?}");
    let (_, wide, w_wide) = compile(
        "machine m:
    var t : Duration = 0 s
    initial RUN
    state RUN:
        loop:
            for i in range(3):
                t = t + tick
            n = 1
",
    );
    assert_eq!(count(&wide, "Arith"), 1, "eine Dauer ist i64 und kann ueberlaufen: {:?}", wide.checks);
    assert!(!w_wide.iter().any(|w| w.contains("SC-24")), "i64-Ueberlauf warnt nicht (Pruefung 4): {w_wide:?}");
}

#[test]
fn a_proven_range_check_stays_in_the_mir_as_proven() {
    use takt_mir::expr::{CheckedKind, ExprKind};
    use takt_mir::stmt::StmtKind;
    use takt_mir::types::RangeOrigin;
    let (p, r, _) = compile(
        "machine m:
    var a : int in 0..9 = 5
    initial RUN
    state RUN:
        loop:
            n = a + 1
",
    );
    assert_eq!(count(&r, "Declared"), 0, "0..9 plus 1 liegt in 0..99: {:?}", r.checks);
    let m = p.machines.iter().find(|m| m.name == "m").expect("Maschine");
    let stmt = &m.states[0].loop_block.stmts[0];
    let StmtKind::Assign { value, .. } = &stmt.kind else { panic!("Zuweisung erwartet: {stmt:?}") };
    let ExprKind::Checked { kind: CheckedKind::Range(range), .. } = &value.kind else {
        panic!("die bewiesene Range bleibt als Knoten stehen: {value:?}")
    };
    assert_eq!(range.origin, RangeOrigin::Proven);
}

#[test]
fn a_check_is_proven_only_when_every_unrolled_round_proves_it() {
    use takt_mir::expr::{CheckedKind, ExprKind};
    use takt_mir::stmt::StmtKind;
    use takt_mir::types::RangeOrigin;
    // Runde 0 und 1 passen in 0..5, Runde 2 (6) nicht: die Pruefung bleibt.
    let (p, r, _) = compile(
        "machine m:
    initial RUN
    state RUN:
        loop:
            for i in range(3):
                var y : int in 0..5 = i * 3
                n = y
",
    );
    assert_eq!(count(&r, "Declared"), 1, "die dritte Runde verletzt die Range: {:?}", r.checks);
    let m = p.machines.iter().find(|m| m.name == "m").expect("Maschine");
    let StmtKind::ForRange { body, .. } = &m.states[0].loop_block.stmts[0].kind else { panic!("Schleife") };
    let StmtKind::Assign { value, .. } = &body.stmts[0].kind else { panic!("Zuweisung") };
    let ExprKind::Checked { kind: CheckedKind::Range(range), .. } = &value.kind else {
        panic!("die Pruefung steht noch: {value:?}")
    };
    assert_eq!(range.origin, RangeOrigin::Declared, "kein Besuch allein beweist sie");
}

#[test]
fn an_external_proof_drops_the_check_and_a_stale_one_is_refused() {
    use takt_mir::analysis::proof::{Proof, Site};
    use takt_mir::expr::{CheckedKind, ExprKind};
    use takt_mir::stmt::StmtKind;
    use takt_mir::types::RangeOrigin;
    let body = "\
machine m:
    var a : int in 0..200 = 100
    initial RUN
    state RUN:
        loop:
            a = a + 60
            n = a % 100
";
    let (_, r, _) = compile(body);
    assert_eq!(count(&r, "Declared"), 1, "a + 60 kann die Range verlassen: {:?}", r.checks);
    let site = r.sites.iter().find(|s| s.cause.name() == "Declared").expect("die Stelle");

    let src = format!("{HEAD}{OUT}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let hash = takt_mir::review::hash_of(src.as_bytes());
    let proof = Proof { program: hash, sites: vec![Site { start: site.span.start, kind: "range".into(), k: 3 }] };
    let out = takt_sema::compile_with(&src, &options, Some(&proof));
    assert!(!out.has_errors(), "{:?}", out.diagnostics);
    assert_eq!(count(&out.report, "Declared"), 0, "die bewiesene Stelle zaehlt nicht mehr: {:?}", out.report.checks);
    let p = out.program.expect("Programm");
    let m = p.machines.iter().find(|m| m.name == "m").expect("Maschine");
    let StmtKind::Assign { value, .. } = &m.states[0].loop_block.stmts[0].kind else { panic!("Zuweisung") };
    let ExprKind::Checked { kind: CheckedKind::Range(range), .. } = &value.kind else { panic!("Knoten: {value:?}") };
    assert_eq!(range.origin, RangeOrigin::Proven, "der Interpreter prueft weiter, der Codegen nicht");

    let stale = Proof { program: "0000".into(), ..proof };
    let out = takt_sema::compile_with(&src, &options, Some(&stale));
    assert!(out.diagnostics.iter().any(|d| d.code == "SC-65"), "{:?}", out.diagnostics);
    assert!(out.program.is_none(), "eine fremde Beweisdatei uebersetzt nicht");
}

#[test]
fn a_float_operation_is_a_non_finite_check_that_never_warns() {
    let (_, r, w) = compile(
        "\
machine m:
    var x : float = 1.5
    initial RUN
    state RUN:
        loop:
            for i in range(3):
                x = x * 2.0
            n = 1
",
    );
    assert_eq!(count(&r, "NonFinite"), 1, "die Multiplikation kann unendlich werden: {:?}", r.checks);
    assert_eq!(count(&r, "Arith"), 0, "kein Ganzzahlfall: {:?}", r.checks);
    assert!(!w.iter().any(|w| w.contains("SC-24")), "Gleitkomma warnt nicht (Pruefung 4): {w:?}");
}

/// 3.4 (Differenzschranken): `if i >= b.len: break` traegt `i < b.len` bis
/// zur naechsten Zuweisung an `i` oder `b`.
#[test]
fn a_length_guard_before_break_proves_the_index() {
    let (_, r, _) = compile(
        "\
machine m:
    var b : bytes<64> = default
    initial RUN
    state RUN:
        loop:
            var s : int in 0..99 = 0
            for i in range(1024):
                if i >= b.len:
                    break
                s = (s + (b[i] as int)) % 100
            n = s
",
    );
    assert_eq!(count(&r, "Index"), 0, "{:?}", r.checks);
    assert_eq!(r.relational, 0, "{:?}", r.checks);
}

#[test]
fn a_length_that_changed_after_the_guard_keeps_the_check() {
    let (_, r, _) = compile(
        "\
machine m:
    var b : bytes<64> = default
    initial RUN
    state RUN:
        loop:
            var s : int in 0..99 = 0
            for i in range(1024):
                if i >= b.len:
                    break
                b.clear()
                s = (s + (b[i] as int)) % 100
            n = s
",
    );
    assert_eq!(count(&r, "Index"), 1, "{:?}", r.checks);
}

/// Der Vergleich traegt eine Konstante: `len >= off + k + 2` beweist
/// `off + k` und `off + k + 1`.
#[test]
fn a_guard_with_an_offset_proves_the_offset_index() {
    let (_, r, _) = compile(
        "\
const HDR : int = 4

record Hdr:
    len : u16

machine m:
    var b : bytes<64> = default
    var k : u16 = 0
    initial RUN
    state RUN:
        loop:
            var hh = Hdr(len = k)
            if (b.len as int) < HDR + (hh.len as int) + 2:
                n = 1
            else:
                var lo = b[HDR + (hh.len as int)] as int
                var hi = b[HDR + (hh.len as int) + 1] as int
                n = (lo + hi) % 100
",
    );
    assert_eq!(count(&r, "Index"), 0, "{:?}", r.checks);
}

/// 3.4: `code` bleibt am Schleifenkopf unter 255, weil der Koerper es bei
/// 255 zuruecksetzt — das findet erst der absteigende Durchlauf nach der
/// Weitung.
#[test]
fn narrowing_after_widening_finds_the_loop_invariant() {
    let (_, r, _) = compile(
        "\
machine m:
    var b : bytes<64> = default
    initial RUN
    state RUN:
        loop:
            var code : int in 1..255 = 1
            for i in range(1024):
                if i >= b.len:
                    break
                if b[i] != 0:
                    code += 1
                if b[i] == 0 or code == 255:
                    code = 1
            n = code % 100
",
    );
    assert_eq!(count(&r, "Declared"), 0, "{:?}", r.checks);
}
