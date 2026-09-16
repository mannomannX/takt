//! Die Abnahme von M4: Interpreter ≡ nativ (13.8, Satz 9.4.4).
//!
//! Jedes Korpusprogramm wird zweimal ausgefuehrt — einmal vom
//! Referenzinterpreter, einmal als uebersetztes Binaerprogramm — und die
//! Outputs muessen Zeichen fuer Zeichen gleich sein.
//!
//! **Die Tests ueberspringen sich ohne clang**, wie die uebrigen
//! LLVM-Tests: Der Compiler baut ueberall, die Abnahme laeuft dort, wo
//! die Werkzeugkette steht.

use takt_conformance::compare;
use takt_conformance::stimulus::Stimulus;
use takt_llvm::toolchain::{Clang, find};
use takt_mir::program::Program;

mod common;

/// Die Korpusprogramme, die der Codegen vollstaendig senkt.
const KORPUS: [&str; 27] = [
    "01_minimal.takt",
    "20_native.takt",
    "19_faults.takt",
    "02_units_and_data.takt",
    "03_sequences_and_faults.takt",
    "12_bitfields.takt",
    "13_framing.takt",
    "13_protocol_analysis.takt",
    "14_latency.takt",
    "15_quality.takt",
    "16_timing.takt",
    "17_nested.takt",
    "18_blocks.takt",
    "21_fault_targets.takt",
    "22_faulted_outputs.takt",
    "23_patterns.takt",
    "24_send_has.takt",
    "25_format.takt",
    "26_samples.takt",
    "27_every.takt",
    "28_scheduled.takt",
    "32_reboot.takt",
    "33_enum_param.takt",
    "34_boot_jump.takt",
    // Hier mit leerem Speicher; das Laden einer Nutzlast auf beiden Seiten
    // prueft `persist_native.rs` (5.9).
    "35_persist.takt",
    "36_int_units.takt",
    "37_follows.takt",
];

/// Wie viele Ticks verglichen werden.
///
/// Genug, dass jede `after`-Frist des Korpus feuert (die laengste ist
/// 500 ms bei 10 ms Tick), und wenig genug, dass ein Fehlschlag noch zu
/// lesen ist.
const TICKS: u64 = 60;

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

/// Fuehrt dasselbe Programm im Interpreter aus.
fn run_interpreted(p: &Program) -> String {
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None, ..Default::default() };
    match takt_interp::run(p, &takt_interp::Trace::default(), &options) {
        Ok(r) => r.trace.render(),
        Err(e) => panic!("Interpreter: {e:?}"),
    }
}

/// **Die Abnahme.** Interpreter und erzeugter Code liefern dieselben
/// Outputs (Satz 9.4.4).
#[test]
fn the_interpreter_and_the_generated_code_agree() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let mut failed = Vec::new();
    for name in KORPUS {
        let p = corpus(name);
        // Alle Maschinen, in Schrittordnung (7.2): Ψ-Lesevorgaenge und
        // `follows` gibt es nur zwischen Maschinen.
        let native = match common::run_native_all(&clang, &p, name, TICKS) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: laesst sich nicht bauen:\n{e}"));
                continue;
            }
        };
        let interpreted = run_interpreted(&p);
        let diffs = compare(&interpreted, &native);
        if !diffs.is_empty() {
            let list: Vec<String> = diffs.iter().take(8).map(|d| format!("  {d}")).collect();
            failed.push(format!(
                "{name}: {} Abweichungen\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
                diffs.len(),
                list.join("\n"),
                interpreted.lines().take(12).collect::<Vec<_>>().join("\n"),
                native.lines().take(12).collect::<Vec<_>>().join("\n")
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

/// Die Grenzen der Abnahme stehen im Code, nicht nur im Plan.
///
/// Der Test gibt sie aus, damit ein gruener Lauf sie mitliefert — und er
/// prueft, dass die Liste nicht leer laeuft. Eine Grenze ohne Termin
/// waere eine Ausrede; jede traegt einen.
#[test]
fn the_limits_of_the_acceptance_are_written_down() {
    let limits = takt_conformance::LIMITS;
    assert!(limits.len() >= 5, "die Liste ist zu kurz, um vollstaendig zu sein: {}", limits.len());
    for l in limits {
        assert!(!l.was.is_empty() && !l.warum.is_empty(), "eine Grenze ohne Begruendung");
        assert!(!l.wann.is_empty(), "`{}` hat keinen Termin", l.was);
    }
    eprintln!("{}", takt_conformance::limits::report());
}

/// **Die Abnahme mit Eingaben** (12.5): Beide Seiten sehen denselben
/// Stimulus, und ihre Outputs stimmen ueberein.
///
/// Das ist die Haelfte, die bis Schritt 10 fehlte: Ein Lauf ohne
/// Eingaben prueft den Anfangszustand und seine Fortschreibung, nicht die
/// *Reaktion* auf Lieferungen. Ein Command ist die einfachste Form davon
/// (ein Puls, ein Byte, 8.5) — und die, die der Korpus benutzt.
#[test]
fn the_two_implementations_agree_on_recorded_inputs() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    // `16_timing` wartet auf `go` und faellt nach 200 ms zurueck; damit
    // laeuft jeder Uebergang mindestens einmal.
    let p = corpus("16_timing.takt");
    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    let stimulus = takt_interp::Trace::parse(
        "t=3 cmd go
t=40 cmd go
",
    )
    .expect("Stimulus");
    let inputs: Vec<Stimulus> = stimulus
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            takt_interp::trace::LineKind::Command { name } => Some(Stimulus::cmd(l.tick, name)),
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 2, "der Stimulus traegt zwei Commands");

    let native =
        common::run_native_with(&clang, &p, "eingaben", &machine, TICKS, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();

    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen mit Eingaben:
{}
--- Interpreter ---
{}
--- nativ ---
{}",
        diffs.len(),
        diffs.iter().take(6).map(|d| format!("  {d}")).collect::<Vec<_>>().join(
            "
"
        ),
        interpreted,
        native
    );
}

/// **Die Abnahme mit Stromelementen** (8.6, 8.7): Ein Handler laeuft
/// nativ ueber ein Fenster mit Daten.
///
/// Bis hierher war das nicht geprueft, und zwar unbemerkt: `takt_stream_count`
/// lieferte null, also lief der Handler-Dispatch des erzeugten Codes
/// zwar, aber nie ueber ein Element (FB-115). Ein leeres Fenster ist
/// gruen wie ein richtiges Ergebnis.
///
/// `23_patterns` ist der Fall, der es zeigt: ein Handler mit `matches`
/// ueber dem Produkt-DFA (8.7, 11.2). Trifft das Muster, steht `y = 7` im
/// Trace; trifft es nicht, bleibt der Anfangswert. Beide Seiten muessen
/// dasselbe sagen — und der Unterschied zwischen „trifft" und „trifft
/// nicht" ist im Trace sichtbar, sonst prueft der Test nichts.
#[test]
fn the_two_implementations_agree_on_stream_elements() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let p = corpus("23_patterns.takt");
    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    // Vier Elemente: eines trifft `"READY"`, eines gar kein Muster, und
    // zwei tragen einen Platzhalter mit verschiedenen Werten. Ohne den
    // zweiten Fall pruefte der Test nur, dass etwas ankommt; ohne die
    // letzten beiden nicht, dass die Extraktion den *Wert* trifft.
    let stimulus = takt_interp::Trace::parse(
        "t=2 in rx_log READY
t=5 in rx_log BUSY
t=8 in rx_log Erasing sector 42
t=11 in rx_log Erasing sector 7
",
    )
    .expect("Stimulus");
    let inputs: Vec<Stimulus> = stimulus
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            takt_interp::trace::LineKind::Input { channel, sample } => {
                Some(Stimulus::element(l.tick, channel, sample.value.as_deref().unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 4, "der Stimulus traegt vier Elemente");

    let native =
        common::run_native_with(&clang, &p, "stroeme", &machine, TICKS, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();

    // Der Handler muss gelaufen sein: `y = 7` steht nur im Trace, wenn
    // das Muster getroffen hat. Ohne diese Zusicherung waere ein Lauf,
    // in dem beide Seiten nichts tun, ebenfalls gruen.
    assert!(
        interpreted.contains("out y 7"),
        "der Interpreter hat den Handler nicht ausgefuehrt; der Test pruefte sonst ein leeres Fenster:\n{interpreted}"
    );

    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen mit Stromelementen:\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
        diffs.len(),
        diffs.iter().take(6).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n"),
        interpreted,
        native
    );
}

/// **`has` und `send` mit Daten** (8.7, 8.8): Das Muster darf an jeder
/// Stelle beginnen, und der Rumpf sendet.
///
/// `has` ist der Fall, den der Automat *nicht* entscheidet: Er prueft
/// den ganzen Text, `has` sucht ein Vorkommen. Der Codegen laeuft darum
/// den Durchlauf aus 8.7 ueber jede Startposition — und dieser Test
/// misst, dass er dieselben Stellen findet wie der Interpreter.
#[test]
fn the_two_implementations_agree_on_has_and_send() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let p = corpus("24_send_has.takt");
    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    // Vier Zeilen: `ERR` am Anfang, in der Mitte, am Ende, und gar nicht.
    // Ein Muster, das nur am Anfang traefe, kaeme auf eine andere Zahl.
    let stimulus = takt_interp::Trace::parse(
        "t=2 in rx ERR init failed
t=4 in rx warn: ERR on bus
t=6 in rx sensor ERR
t=8 in rx all good
",
    )
    .expect("Stimulus");
    let inputs: Vec<Stimulus> = stimulus
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            takt_interp::trace::LineKind::Input { channel, sample } => {
                Some(Stimulus::element(l.tick, channel, sample.value.as_deref().unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 4, "der Stimulus traegt vier Zeilen");

    let native =
        common::run_native_with(&clang, &p, "has_send", &machine, TICKS, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();

    // Drei der vier Zeilen tragen `ERR`; stuende hier eine andere Zahl,
    // haette `has` nicht an jeder Stelle gesucht.
    assert!(
        interpreted.contains("out hits 3"),
        "`has` hat nicht drei Vorkommen gefunden; der Test pruefte sonst nichts:\n{interpreted}"
    );
    // Der Treiber holt das Gesendete ab und meldet es als `out tx
    // [bytes]` (8.8) — `compare` prueft die Zeile also gegen den
    // Interpreter. Die Zusicherung hier belegt, dass ueberhaupt gesendet
    // wurde: Ein Lauf ohne `send` waere sonst ebenfalls gruen.
    assert!(
        native.contains("out tx [0x61, 0x63, 0x6b, 0x20, 0x31, 0x0a]"),
        "`ack 1` fehlt im nativen Trace:\n{native}"
    );

    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen bei `has`/`send`:\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
        diffs.len(),
        diffs.iter().take(6).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n"),
        interpreted,
        native
    );
}

/// Die Formatangaben aus 3.9 im erzeugten Code: `{x}`, `{x:hex}`,
/// `{x:04}`.
///
/// Sie laufen ueber dieselbe Ziffernrechnung, unterscheiden sich aber in
/// Basis, Vorzeichen und Fuellung — und jede davon hat einen Rand: die
/// Null ohne Ziffer, das Minus vor der Zahl, die Fuellung, die kuerzer
/// ist als die Zahl.
#[test]
fn the_format_specs_produce_the_expected_text() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let p = corpus("25_format.takt");
    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    let native = common::run_native_with(&clang, &p, "format", &machine, 8, &[]).unwrap_or_else(|e| panic!("{e}"));
    // Der Treiber meldet die Bytes als `out tx [..]` (8.8), also prueft
    // `compare` sie gegen den Interpreter. Hier steht, *was* dort stehen
    // muss — sonst waere ein Lauf gruen, in dem beide Seiten dasselbe
    // Falsche schreiben.
    for (was, bytes) in [
        // `d=10 h=a p=0010`
        ("`{x:hex}` von 10 ist `a`", "0x64, 0x3d, 0x31, 0x30, 0x20, 0x68, 0x3d, 0x61"),
        // `d=-2 h=fffffffffffffffe …`: hex zaehlt das Bitmuster (3.9).
        ("`{x:hex}` von -2 ist vorzeichenlos", "0x64, 0x3d, 0x2d, 0x32, 0x20, 0x68, 0x3d, 0x66, 0x66"),
    ] {
        assert!(native.contains(bytes), "{was} — fehlt:\n{native}");
    }
}
