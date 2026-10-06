//! Der Korpus auf einem Board gegen den Interpreter (13.8, Satz 9.4.4).

use std::collections::BTreeSet;

use takt_conformance::board::{self, Board, Options};
use takt_conformance::compare;
use takt_mir::program::Program;

/// Ticks eines Konformitaetslaufs auf dem Board.
pub const TICKS: u64 = 60;

/// Ein Programm des Korpus, fuer die Simulation uebersetzt.
pub fn corpus(name: &str) -> Program {
    program(&board::corpus_path(name))
}

/// Ein Programm, fuer die Simulation uebersetzt; die Konfigurationen aus
/// `import channels` liegen neben ihm (8.2).
pub fn program(path: &std::path::Path) -> Program {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let channel_imports = takt_sema::channel_imports(&src)
        .into_iter()
        .map(|file| {
            let text = std::fs::read_to_string(dir.join(&file)).unwrap_or_else(|e| panic!("{file}: {e}"));
            (file, text)
        })
        .collect();
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        channel_imports,
        core: None,
    };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}:\n{}", path.display(), errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{}: kein Programm", path.display()))
}

/// **Der Treiberrand urteilt auf dem Board wie im Interpreter** (12.6, M10
/// Schritt 29c). Das Pruefgeraet des Bring-ups (`edge_probe`) liefert jeden
/// Verstoss gegen den Treibervertrag einmal, bestaetigt einen
/// Schreibvorgang nicht und laesst einmal den Heartbeat aus; der
/// Interpreter rechnet denselben Lauf mit der Folge des Pruefgeraets als
/// Stimulus. Verglichen werden die Ausgaben, die Zeilen `driver` und die
/// `Runtime(Driver)`, die das Board erhebt.
pub fn driver_edge_agrees(board: &mut dyn Board) -> Vec<String> {
    const EDGE_TICKS: u64 = 16;
    let path = board::root().join("crates/takt-conformance/tests/programs/driver_edge.takt");
    let options = Options::fresh(EDGE_TICKS);
    let text = match board.build(&path, &options).and_then(|elf| board.run(&elf, &options)) {
        Ok(t) => t,
        Err(e) => return vec![format!("kein Lauf: {e}")],
    };
    let stimulus = takt_interp::Trace::parse(&probe_stimulus(EDGE_TICKS)).expect("Stimulus");
    let run = takt_interp::RunOptions { ticks: EDGE_TICKS, ..Default::default() };
    let interpreted = takt_interp::run(&program(&path), &stimulus, &run).expect("Lauf").trace.render();
    let mut failed = Vec::new();
    let diffs = compare(&interpreted, &text);
    if !diffs.is_empty() {
        let list: Vec<String> = diffs.iter().take(8).map(|d| format!("  {d}")).collect();
        failed.push(format!("{} Abweichungen:\n{}\n--- Board ---\n{text}", diffs.len(), list.join("\n")));
    }
    let lines = |t: &str, what: &str| -> Vec<String> {
        t.lines().filter(|l| l.starts_with("t=") && l.contains(what)).map(|l| l.trim_end().to_string()).collect()
    };
    // Je Fall des Pruefgeraets seine Zeile (KON1-012): Gleichheit allein
    // bestuende auch, wenn beide Seiten keinen Verstoss mehr saehen.
    let edge = |t: &str| -> Vec<String> {
        let mut out = lines(t, " driver ");
        out.extend(lines(t, " stream pairs "));
        out.sort_by_key(|l| l.trim_start_matches("t=").split(' ').next().and_then(|k| k.parse::<u64>().ok()));
        out
    };
    let want: Vec<String> = EDGE_LINES.iter().map(|l| l.to_string()).collect();
    for (side, trace) in [("Interpreter", &interpreted), (board.name(), &text)] {
        if edge(trace) != want {
            failed.push(format!("{side}: andere Zeilen des Treiberrands:\n{:#?}\nerwartet:\n{want:#?}", edge(trace)));
        }
    }
    // Die Folge des Pruefgeraets bestimmt, welche Ausgaenge faulten; die
    // Reihenfolge innerhalb eines Ticks ist die der Ausgaenge im Rahmen.
    let by_tick = |mut v: Vec<String>| {
        v.sort_by_key(|l| {
            (l.trim_start_matches("t=").split(' ').next().and_then(|k| k.parse::<u64>().ok()), l.clone())
        });
        v
    };
    let raised = by_tick(raised_driver_faults(EDGE_TICKS));
    if by_tick(lines(&text, " runtime Driver ")) != raised {
        failed.push(format!("das Board erhob {:?}, erwartet {raised:?}", lines(&text, " runtime Driver ")));
    }
    // Die Besitzer reagieren: `actor` und `talker` nehmen ihren Fault-Pfad,
    // `failing` hat keinen und steht ab dem ersten Fault in FAULTED (5.3).
    if lines(&text, " fault ") != FAULT_LINES {
        failed.push(format!("{}: Faults {:?}, erwartet {FAULT_LINES:?}", board.name(), lines(&text, " fault ")));
    }
    if !interpreted.contains("t=1 state failing FAULTED") {
        failed.push(format!("Interpreter: `failing` steht in Tick 1 nicht in FAULTED:\n{interpreted}"));
    }
    // FAULTED setzt den Ausgang auf `safe` (5.3), im selben Tick.
    if !text.lines().any(|l| l.trim_end() == "t=1 out f 0") {
        failed.push(format!("{}: `f` geht in Tick 1 nicht auf `safe`:\n{text}", board.name()));
    }
    eprintln!("{} Treiberrand: {} Abweichungen", board.name(), failed.len());
    failed
}

/// Die Zeilen des Treiberrands im Lauf des Pruefgeraets (12.6), je Fall der
/// Folge aus `edge_probe` eine, in Tickordnung.
const EDGE_LINES: [&str; 15] = [
    // Zeile 1: hinter dem Fenster in der Toleranz, dann jenseits.
    "t=1 driver edge_a warped p",
    "t=2 driver edge_a degraded window",
    "t=3 driver edge_a recovered",
    // Zeile 2: Luecke in `seq`, mehr als MAXPT.
    "t=5 driver edge_u degraded seq",
    "t=6 driver edge_u recovered",
    "t=7 driver edge_u degraded maxpt",
    "t=8 driver edge_u recovered",
    // Zeile 5: ein Byte zu viel fuer `Pair`.
    "t=9 stream pairs dropped=0 overflowed=0 malformed=1",
    // Zeile 2: fallender Zeitstempel.
    "t=11 driver edge_b degraded timestamp",
    "t=12 driver edge_b recovered",
    // KON1-012: `seq` doppelt, ein Byte zu wenig fuer `Pair`; `t = t_k` gilt.
    "t=13 driver edge_u degraded seq",
    "t=13 stream pairs dropped=0 overflowed=0 malformed=2",
    // Die Zeile ueber `line<16>` ist gekuerzt und kein Verstoss; `t =
    // t_(k-1)` liegt knapp vor dem Fenster.
    "t=14 driver edge_u recovered",
    "t=14 driver edge_a warped q",
    // `seq` rueckwaerts.
    "t=15 driver edge_u degraded seq",
];

/// Die Faults des Laufs, wie der Rahmen sie schreibt: je `Runtime(Driver)`
/// eines Besitzers einer, bei `failing` nur der erste — danach steht es in
/// FAULTED, und ein weiterer Fault dort hat kein Ziel.
const FAULT_LINES: [&str; 4] = [
    "t=1 fault failing Runtime(Driver)",
    "t=4 fault actor Runtime(Driver)",
    "t=9 fault actor Runtime(Driver)",
    "t=13 fault talker Runtime(Driver)",
];

/// Die Folge des Pruefgeraets als Stimulus des Interpreters: Lieferungen mit
/// Zeitstempel und Folgenummer, die Faults der Ausgabeseite als `runtime`.
fn probe_stimulus(ticks: u64) -> String {
    use std::fmt::Write as _;
    use takt_board_support::edge_probe::{self, Scalar, Stream};
    let mut s = String::new();
    for tick in 0..ticks {
        for ch in Scalar::ALL {
            let Some((v, at)) = edge_probe::reading(ch, tick) else { continue };
            let _ = write!(s, "t={tick} in {} {v}", ch.name());
            if let Some(at) = at {
                let _ = write!(s, " t={at}");
            }
            s.push('\n');
        }
        for st in Stream::ALL {
            for i in 0.. {
                let Some((bytes, seq)) = edge_probe::element(st, tick, i) else { break };
                let text = match st {
                    Stream::Lines => format!("{:?}", String::from_utf8_lossy(bytes)),
                    Stream::Pairs => format!("0x{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()),
                };
                let _ = writeln!(s, "t={tick} in {} {text} seq={seq}", st.name());
            }
        }
    }
    for line in raised_driver_faults(ticks) {
        let _ = writeln!(s, "{line}");
    }
    s
}

/// Die `Runtime(Driver)`, die der Rahmen nach der Folge des Pruefgeraets
/// erhebt (12.6 Zeile 6), je einer im Tick nach dem gescheiterten Commit:
/// `o` nach dem unbestaetigten Schreiben und dem stillen Heartbeat, `tx`
/// nach dem ueberfahrenen Sendepuffer, `f` nach jedem Commit, den es nie
/// bestaetigt. Auch Tick 0 committet (1.5, 9.4): Das erste Urteil ueber `f`
/// faellt in Tick 1.
fn raised_driver_faults(ticks: u64) -> Vec<String> {
    use takt_board_support::edge_probe;
    let mut out: Vec<String> = edge_probe::DRIVER_FAULTS.iter().map(|k| format!("t={k} runtime Driver o")).collect();
    out.push(format!("t={} runtime Driver tx", edge_probe::TX_OVER_AT + 1));
    out.extend(
        (0..ticks).filter(|k| !edge_probe::failing_confirms(*k)).map(|k| format!("t={} runtime Driver f", k + 1)),
    );
    out
}

/// **Ein Zeitgeber, der seine Periode verfehlt, ist `Runtime(Hardware)`**
/// (12.6 Zeile 7, 7.1). `tick_stretch.takt` laesst das Pruefgeraet des
/// Bring-ups ab Tick 5 jede Periode um 5 Prozent strecken, ueber der
/// Toleranz von 2 Prozent. Die Schleife erhebt den Fault erst mit der
/// zehnten Verletzung in Folge, also fruehestens neun Ticks nach der
/// ersten gestreckten Periode, und hoert auf, sobald der `safe`-Wert des
/// gefaulteten Besitzers den Zeitgeber zurueckstellt — die Periode, die
/// dann schon laeuft, ist noch gestreckt. Welcher Tick es genau ist, weiss
/// nur der Zeitgeber: Der Interpreter bekommt die `runtime`-Zeilen des
/// Boards als Stimulus und muss dieselben Ausgaben rechnen (12.5).
pub fn a_stretched_tick_is_runtime_hardware(board: &mut dyn Board) -> Vec<String> {
    const STRETCH_TICKS: u64 = 30;
    let path = board::root().join("crates/takt-conformance/tests/programs/tick_stretch.takt");
    let options = Options::timed(STRETCH_TICKS);
    let text = match board.build(&path, &options).and_then(|elf| board.run(&elf, &options)) {
        Ok(t) => t,
        Err(e) => return vec![format!("kein Lauf: {e}")],
    };
    let raised: Vec<u64> =
        text.lines().filter_map(|l| l.strip_prefix("t=")?.strip_suffix(" runtime Hardware")?.parse().ok()).collect();
    let mut failed = Vec::new();
    // Gestreckt ab der Periode nach dem Commit von Tick 5: die zehnte
    // Verletzung in Tick 16, zwei Ticks Spiel fuer die Phase des Zeitgebers.
    match (raised.first(), raised.last()) {
        (Some(&first), Some(&last)) if (14..=18).contains(&first) && last <= first + 2 => {}
        _ => failed.push(format!("`runtime Hardware` in {raised:?}, erwartet ab Tick 14 bis 18:\n{text}")),
    }
    let stimulus: String = raised.iter().map(|k| format!("t={k} runtime Hardware\n")).collect();
    let stimulus = takt_interp::Trace::parse(&stimulus).expect("Stimulus");
    let run = takt_interp::RunOptions { ticks: STRETCH_TICKS, ..Default::default() };
    let interpreted = takt_interp::run(&program(&path), &stimulus, &run).expect("Lauf").trace.render();
    let diffs = compare(&interpreted, &text);
    if !diffs.is_empty() {
        failed.push(format!("{} Abweichungen: {diffs:?}\n{text}", diffs.len()));
    }
    eprintln!("{} Tick-Periode: Runtime(Hardware) in {raised:?}", board.name());
    failed
}

/// **Eine Wake-Quelle weckt an der Grenze nach ihrem Ereignis** (5.10, 9.9,
/// Satz 9.9.1; FB-388). `wake.takt` schlaeft in `idle`-Zustaenden mit
/// Fristen von 2 s; das Pruefgeraet hebt `level` und laeutet `bell` zu
/// Zeiten, die das Programm nicht kennt (`wake_probe`). Der Rahmen tastet
/// die Wake-Quellen an jeder geschlafenen Grenze ab: `up` geht im Tick des
/// Pegels an, `rung` im Tick nach der Klingel, wie im Interpreter, der die
/// Lieferungen des Pruefgeraets als Stimulus bekommt. Dazwischen schlaeft
/// das Board wirklich; ohne Weckereignis stuende es bis zur Frist.
pub fn a_wake_source_ends_the_sleep(board: &mut dyn Board) -> Vec<String> {
    use takt_board_support::wake_probe::{BELL, BELL_AT_NS, LEVEL_AT_NS, first_tick};
    const WAKE_TICKS: u64 = 100;
    let path = board::root().join("crates/takt-conformance/tests/programs/wake.takt");
    let options = Options::fresh(WAKE_TICKS);
    let text = match board.build(&path, &options).and_then(|elf| board.run(&elf, &options)) {
        Ok(t) => t,
        Err(e) => return vec![format!("kein Lauf: {e}")],
    };
    let (level, bell) = (first_tick(LEVEL_AT_NS), first_tick(BELL_AT_NS));
    let mut stimulus: String = (0..WAKE_TICKS).map(|k| format!("t={k} in level {}\n", k >= level)).collect();
    stimulus.push_str(&format!("t={bell} in bell {BELL}\n"));
    let stimulus = takt_interp::Trace::parse(&stimulus).expect("Stimulus");
    let run = takt_interp::RunOptions { ticks: WAKE_TICKS, ..Default::default() };
    let interpreted = takt_interp::run(&program(&path), &stimulus, &run).expect("Lauf").trace.render();
    let mut failed = Vec::new();
    let diffs = compare(&interpreted, &text);
    if !diffs.is_empty() {
        failed.push(format!("{} Abweichungen: {diffs:?}\n{text}", diffs.len()));
    }
    // Der Rahmen schreibt `bool` als Zahl; die Zeile steht im Tick des Ereignisses.
    for want in [format!("t={level} out up "), format!("t={} out rung {BELL}", bell + 1)] {
        if !text.lines().any(|l| l.starts_with(&want)) {
            failed.push(format!("`{want}` fehlt:\n{text}"));
        }
    }
    // Vor dem Pegel und vor der Klingel: zusammen gut dreissig Ticks.
    let slept: u64 =
        text.lines().filter_map(|l| l.split_once(" slept=")?.1.split_whitespace().next()?.parse::<u64>().ok()).sum();
    if slept < 30 {
        failed.push(format!("nur {slept} Ticks geschlafen:\n{text}"));
    }
    eprintln!("{} Weckereignisse: {} Abweichungen, {slept} Ticks geschlafen", board.name(), failed.len());
    failed
}

/// **Ein Tune von der Konsole gilt ab seiner Grenze und weckt das Board**
/// (8.4, 9.9, FB-389). `tune.takt` schlaeft in `idle` mit einer Frist von
/// 2 s; der Host schickt nach 300 ms `GAIN = 200` und nach 600 ms
/// `GAIN = 7` an die Konsole. Das Board traegt jeden an der naechsten
/// Grenze ein und schreibt seine Zeile `tune`, den ersten als verworfen;
/// der zweite weckt (`when GAIN > 5`). Welcher Tick es ist, weiss nur das
/// Board: Der Interpreter bekommt seine `tune`-Zeilen als Stimulus und muss
/// dieselben Ausgaben rechnen (12.5).
pub fn a_tune_from_the_console_wakes_the_board(board: &mut dyn Board) -> Vec<String> {
    const TUNE_TICKS: u64 = 150;
    let path = board::root().join("crates/takt-conformance/tests/programs/tune.takt");
    let p = program(&path);
    let line = |text: &str| board::tune_line(&p, "GAIN", text).expect("GAIN ist ein Tunable");
    let options = Options::timed(TUNE_TICKS)
        .with_console(std::time::Duration::from_millis(300), &line("200"))
        .with_console(std::time::Duration::from_millis(600), &line("7"));
    let text = match board.build(&path, &options).and_then(|elf| board.run(&elf, &options)) {
        Ok(t) => t,
        Err(e) => return vec![format!("kein Lauf: {e}")],
    };
    let tunes: Vec<String> = text
        .lines()
        .filter(|l| l.starts_with("t=") && l.contains(" tune "))
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let tick = |l: &str| l.strip_prefix("t=")?.split(' ').next()?.parse::<u64>().ok();
    let mut failed = Vec::new();
    match tunes.as_slice() {
        [rejected, accepted] if rejected.ends_with(" tune GAIN 200 rejected") && accepted.ends_with(" tune GAIN 7") => {
            let (a, b) = (tick(rejected).unwrap_or(0), tick(accepted).unwrap_or(0));
            if !(10..b).contains(&a) || b >= TUNE_TICKS - 10 {
                failed.push(format!("Tunes in den Ticks {a} und {b}, erwartet um 30 und 60:\n{text}"));
            }
            for want in [format!("t={b} out up "), format!("t={b} out level 14")] {
                if !text.lines().any(|l| l.starts_with(&want)) {
                    failed.push(format!("`{want}` fehlt: Der Tune weckt nicht an seiner Grenze:\n{text}"));
                }
            }
        }
        _ => failed.push(format!("`tune`-Zeilen {tunes:?}, erwartet `GAIN 200 rejected`, dann `GAIN 7`:\n{text}")),
    }
    let stimulus: String = tunes.iter().map(|l| format!("{l}\n")).collect();
    let stimulus = takt_interp::Trace::parse(&stimulus).expect("Stimulus aus den Zeilen des Boards");
    let run = takt_interp::RunOptions { ticks: TUNE_TICKS, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &run).expect("Lauf").trace.render();
    let diffs = compare(&interpreted, &text);
    if !diffs.is_empty() {
        failed.push(format!("{} Abweichungen: {diffs:?}\n{text}", diffs.len()));
    }
    let slept: u64 =
        text.lines().filter_map(|l| l.split_once(" slept=")?.1.split_whitespace().next()?.parse::<u64>().ok()).sum();
    if slept < 30 {
        failed.push(format!("nur {slept} Ticks geschlafen:\n{text}"));
    }
    eprintln!("{} Tunes: {tunes:?}, {slept} Ticks geschlafen", board.name());
    failed
}

/// **`guard` aus der Konfiguration wirkt auf dem Board** (7.5, FB-331):
/// `at now + 100 ns` liegt unter der gemessenen Treiberlatenz der Bruecke
/// in `corpus-try/hw/<board>.hw` und ist ein `TimingFault`, `at now + 1 ms`
/// geht durch. Derselbe Bau ohne `guard` am Kanal rechnet wie die
/// Simulation und laesst beides durch. Ohne Konfiguration baut ein Board
/// nicht mehr: Speicher, Schutzregion und NVM stehen in ihr (M11 Schritt 10).
pub fn a_schedule_inside_the_guard_is_a_timing_fault(board: &mut dyn Board) -> Vec<String> {
    let program = board::root().join("crates/takt-conformance/tests/programs/guard.takt");
    let hardware = board::root().join(format!("corpus-try/hw/{}.hw", board.name()));
    // Dieselbe Konfiguration ohne die gemessenen `guard_ns`.
    let text = std::fs::read_to_string(&hardware).expect("Konfiguration lesbar");
    let unguarded = takt_conformance::target_dir().join(format!("takt-{}-ohne-guard.hw", board.name()));
    let lines: String =
        text.lines().filter(|l| !l.trim_start().starts_with("guard_ns")).map(|l| format!("{l}\n")).collect();
    std::fs::write(&unguarded, lines).expect("Konfiguration schreibbar");
    let mut run = |options: Options| board.build(&program, &options).and_then(|elf| board.run(&elf, &options));
    let mut failed = Vec::new();
    match run(Options::fresh(20).with_hardware(hardware)) {
        Ok(with) => {
            if !with.lines().any(|l| l.contains("out probe 1")) {
                failed.push(format!("1 ms voraus geht nicht durch:\n{with}"));
            }
            if !with.lines().any(|l| l.contains("fault m") && l.contains("Timing")) {
                failed.push(format!("100 ns voraus ist kein `TimingFault`:\n{with}"));
            }
        }
        Err(e) => failed.push(format!("kein Lauf mit Konfiguration: {e}")),
    }
    match run(Options::fresh(20).with_hardware(unguarded)) {
        Ok(without) if without.contains("fault m") => {
            failed.push(format!("ohne `guard` am Kanal ist guard null:\n{without}"));
        }
        Ok(_) => {}
        Err(e) => failed.push(format!("kein Lauf ohne `guard`: {e}")),
    }
    failed
}

/// **Was das Programm nicht liest, zeichnet der Rahmen auf** (8.2, 12.5,
/// M10 Schritte 29d und 29e). `recorded.takt` importiert `recorded.hw` und
/// nennt keinen der Kanaele `edge_r/*`; das Pruefgeraet liefert vier der
/// Skalare und drei Stroeme. Der Rahmen schreibt sie als Metazeile `rec`:
/// einen Skalar in Tick 0 und bei jeder Aenderung — ein eigener
/// Zeitstempel ist eine —, `Bad` mit Grund `Driver`, eine Diskriminante,
/// die das Enum nicht kennt, mit Grund `OutOfRange`; einen Strom je
/// Element mit abweichendem `t=` und `seq=`. Der Kanal ohne Treiber und der
/// Output erscheinen nie. Die Ausgaben stimmen mit dem Interpreter
/// ueberein, der keine `rec`-Zeile schreibt.
pub fn unread_channels_are_recorded(board: &mut dyn Board) -> Vec<String> {
    const RECORD_TICKS: u64 = 10;
    const RECORDED: [&str; 23] = [
        // Stroeme (29e): je Element eine Zeile, ein `u8` als Zahl, sonst
        // die Drahtform; ein `u8` aus zwei Bytes ist keines und fehlt.
        "t=0 rec edge_r_frames 0x0102",
        "t=0 rec edge_r_raw 65",
        "t=1 rec edge_r_text 0x6869",
        "t=2 rec edge_r_frames 0x0304",
        "t=2 rec edge_r_frames 0x0506 t=25000000",
        "t=3 rec edge_r_frames 0x070809",
        "t=3 rec edge_r_raw 66 seq=7",
        "t=4 rec edge_r_text 0x610962",
        "t=5 rec edge_r_text 0x313233343536373839",
        "t=6 rec edge_r_text 0xff",
        "t=0 rec edge_r_level 5",
        "t=0 rec edge_r_mode IDLE",
        "t=0 rec edge_r_on true",
        "t=0 rec edge_r_temp 21.5 degC",
        "t=2 rec edge_r_level 7",
        "t=2 rec edge_r_mode RUN",
        "t=2 rec edge_r_temp 22.0 degC t=15000000",
        "t=3 rec edge_r_on false",
        "t=4 rec edge_r_level bad reason=Driver",
        "t=5 rec edge_r_level 7",
        "t=5 rec edge_r_mode ERROR",
        "t=6 rec edge_r_mode bad reason=OutOfRange",
        "t=7 rec edge_r_mode ERROR",
    ];
    let path = board::root().join("crates/takt-conformance/tests/programs/recorded.takt");
    let options = Options::fresh(RECORD_TICKS);
    let text = match board.build(&path, &options).and_then(|elf| board.run(&elf, &options)) {
        Ok(t) => t,
        Err(e) => return vec![format!("kein Lauf: {e}")],
    };
    let mut failed = Vec::new();
    let run = takt_interp::RunOptions { ticks: RECORD_TICKS, ..Default::default() };
    let none = takt_interp::Trace::parse("").expect("leerer Stimulus");
    let interpreted = takt_interp::run(&program(&path), &none, &run).expect("Lauf").trace.render();
    let diffs = compare(&interpreted, &text);
    if !diffs.is_empty() {
        failed.push(format!("{} Abweichungen: {diffs:?}\n{text}", diffs.len()));
    }
    if interpreted.contains(" rec ") {
        failed.push(format!("der Interpreter schreibt `rec`:\n{interpreted}"));
    }
    let lines: String =
        text.lines().filter(|l| l.starts_with("t=") && l.contains(" rec ")).map(|l| format!("{l}\n")).collect();
    match takt_interp::Trace::parse(&lines) {
        Ok(trace) => {
            let mut got: Vec<String> = trace.render().lines().map(str::to_string).collect();
            let mut want: Vec<String> = RECORDED.iter().map(|l| l.to_string()).collect();
            got.sort();
            want.sort();
            if got != want {
                failed.push(format!("aufgezeichnet:\n{}\nerwartet:\n{}", got.join("\n"), want.join("\n")));
            }
        }
        Err(e) => failed.push(format!("`rec`-Zeilen nicht lesbar: {e:?}\n{lines}")),
    }
    eprintln!("{} Aufzeichnung: {} Abweichungen", board.name(), failed.len());
    failed
}

/// **Ein Job, der laenger rechnet als ein Tick, verspaetet keinen** (4.5,
/// 12.3). `long_job.takt` rechnet SHA-256 ueber 4096 Byte bei 1 ms Tick in
/// Echtzeit: Das Ergebnis stimmt mit dem Interpreter ueberein und erscheint
/// nach seiner Dauer, und kein Tick beginnt um mehr als eine Zehntelperiode
/// spaeter als im Mittel. Liefe der Job im Schritt oder ohne Unterbrechung,
/// begaenne der Tick danach um die Dauer seiner Rechnung zu spaet.
///
/// Gemessen wird die Verspaetung gegen den Median, nicht der Sprung zum
/// vorigen Tick; die ersten beiden Ticks laufen noch an. Ein Tick, der eine
/// Periode zu spaet kommt, faellt hier auf (FB-352); dass er nach dem Job
/// frueher begann als nach `wfi` (FB-317), war dieselbe Ursache.
pub fn long_job_keeps_the_tick(board: &mut dyn Board) -> Vec<String> {
    let path = board::root().join("crates/takt-conformance/tests/programs/long_job.takt");
    let options = Options::timed(TICKS);
    let text = match board.build(&path, &options).and_then(|elf| board.run(&elf, &options)) {
        Ok(t) => t,
        Err(e) => return vec![format!("kein Lauf: {e}")],
    };
    let mut failed = Vec::new();
    let diffs = compare(&run_interpreted(&program(&path)), &text);
    if !diffs.is_empty() {
        failed.push(format!("{} Abweichungen: {diffs:?}\n{text}", diffs.len()));
    }
    let drift = Drift::of(&text);
    if drift.ticks < 50 || drift.late > 100_000 {
        failed.push(format!("{} Zeitzeilen, ein Tick {} ns spaeter als im Mittel:\n{text}", drift.ticks, drift.late));
    }
    failed
}

/// Wie spaet die Ticks eines Laufs nach ihrer Grenze begannen (`drift`
/// der Zeitzeilen, 7.3); die ersten beiden laufen noch an.
#[derive(Clone, Copy, Debug)]
pub struct Drift {
    /// Zeitzeilen.
    pub ticks: usize,
    /// Median in ns.
    pub median: i64,
    /// Der spaeteste Tick, in ns ueber dem Median.
    pub late: i64,
}

impl Drift {
    /// Aus dem Trace eines Laufs in Echtzeit.
    pub fn of(text: &str) -> Drift {
        let mut drifts: Vec<i64> = text
            .lines()
            .filter_map(|l| l.split_whitespace().find_map(|w| w.strip_prefix("drift="))?.parse().ok())
            .skip(2)
            .collect();
        drifts.sort_unstable();
        let median = drifts.get(drifts.len() / 2).copied().unwrap_or(0);
        Drift { ticks: drifts.len(), median, late: drifts.last().map_or(0, |d| d - median) }
    }
}

/// **Ein Tick ueber seiner Periode faultet im naechsten jede Maschine**
/// (7.3, 5.4, FB-332). `overrun.takt` laesst die Last wachsen, bis ein Tick
/// laenger rechnet als seine Periode, aber weit unter der Frist des
/// Watchdogs. Die Schleife meldet den Ueberlauf, der Rahmen schreibt
/// `runtime Overrun` und stellt `Runtime(Overrun)` im naechsten Tick zu:
/// `ramp` nimmt seinen Fault-Pfad und `bystander` auch, obwohl sie nichts
/// rechnet. Danach laeuft der Lauf ohne Reset zu Ende, und der Interpreter
/// spielt ihn mit der Zeile `runtime` als Stimulus nach (12.5).
pub fn overrun_reaches_every_machine(board: &mut dyn Board) -> Vec<String> {
    const RUN: u64 = 300;
    let path = board::root().join("crates/takt-conformance/tests/programs/overrun.takt");
    let options = Options::timed(RUN);
    let text = match board.build(&path, &options).and_then(|elf| board.run(&elf, &options)) {
        Ok(t) => t,
        Err(e) => return vec![format!("kein Lauf: {e}")],
    };
    let tick_of = |needle: &str| {
        text.lines()
            .find(|l| l.contains(needle))
            .and_then(|l| l.strip_prefix("t=")?.split_whitespace().next()?.parse::<u64>().ok())
    };
    let raised = tick_of(" runtime Overrun");
    let (ramp, bystander) = (tick_of(" fault ramp Runtime(Overrun)"), tick_of(" fault bystander Runtime(Overrun)"));
    let Some(at) = raised.filter(|_| raised == ramp && raised == bystander) else {
        return vec![format!(
            "nicht im selben Tick: runtime {raised:?}, ramp {ramp:?}, bystander {bystander:?}\n{text}"
        )];
    };
    let mut failed = Vec::new();
    if text.matches(" runtime Overrun").count() != 1 {
        failed.push(format!("mehr als ein Ueberlauf; `SAFE` rechnet nicht mehr:\n{text}"));
    }
    let stimulus = takt_interp::Trace::parse(&format!("t={at} runtime Overrun\n")).expect("Stimulus");
    let options = takt_interp::RunOptions { ticks: RUN, ..Default::default() };
    let replayed = match takt_interp::run(&program(&path), &stimulus, &options) {
        Ok(r) => r.trace.render(),
        Err(e) => return vec![format!("Interpreter: {e:?}")],
    };
    let diffs = compare(&replayed, &text);
    if !diffs.is_empty() {
        failed.push(format!("{} Abweichungen vom nachgespielten Lauf: {diffs:?}\n{text}", diffs.len()));
    }
    failed
}

/// Der Soll-Trace ueber [`TICKS`] Ticks.
pub fn run_interpreted(p: &Program) -> String {
    let options = takt_interp::RunOptions { ticks: TICKS, ..Default::default() };
    match takt_interp::run(p, &takt_interp::Trace::default(), &options) {
        Ok(r) => r.trace.render(),
        Err(e) => panic!("Interpreter: {e:?}"),
    }
}

/// Die Namen der Ausgaenge, die ein Trace nennt.
pub fn output_names(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            w.next()?.strip_prefix("t=")?;
            (w.next()? == "out").then(|| w.next().map(String::from))?
        })
        .collect()
}

/// Der letzte `out`-Wert eines Ausgangs im Trace.
pub fn last_output(text: &str, name: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            w.next()?.strip_prefix("t=")?;
            (w.next()? == "out" && w.next()? == name).then(|| w.collect::<Vec<_>>().join(" "))
        })
        .next_back()
}

/// Laesst `names` auf dem Board laufen und haelt jeden Trace gegen den
/// Interpreter; liefert je abweichendem Programm eine Meldung.
///
/// `only` beschraenkt den Lauf auf ein Programm (`TAKT_…_ONLY`); ein Name,
/// den die Liste nicht kennt, ist ein Fehlschlag mit den bekannten Namen,
/// kein Lauf ohne Programm (KON1-027).
pub fn agreement(board: &mut dyn Board, names: &[&str], only: Option<&str>) -> Vec<String> {
    agreement_with(board, names, only, &Options::fresh(TICKS))
}

/// Wie [`agreement`], mit anderen Optionen des Baus — etwa im Profil
/// `shared` unter dem RTOS des Boards (12.8).
pub fn agreement_with(board: &mut dyn Board, names: &[&str], only: Option<&str>, options: &Options) -> Vec<String> {
    let chosen: Vec<&str> = names.iter().copied().filter(|n| only.is_none_or(|o| o == *n)).collect();
    if chosen.is_empty() {
        return vec![format!(
            "kein Programm gewaehlt (`{}`); bekannt sind: {}",
            only.unwrap_or_default(),
            names.join(", ")
        )];
    }
    let mut failed = Vec::new();
    for name in chosen {
        let p = corpus(name);
        let text = match board.build(&board::corpus_path(name), options).and_then(|elf| board.run(&elf, options)) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: kein Lauf auf dem Board:\n{e}"));
                continue;
            }
        };
        let interpreted = run_interpreted(&p);
        let missing: Vec<String> = output_names(&interpreted).difference(&output_names(&text)).cloned().collect();
        // Ein `f32` schreibt das Board als seinen Wert in `f64` (4.2, FB-356).
        let widened = takt_conformance::run::widen_f32(&interpreted, &takt_conformance::run::f32_outputs(&p));
        let diffs = compare(&widened, &text);
        if !missing.is_empty() || !diffs.is_empty() {
            let list: Vec<String> = diffs.iter().take(8).map(|d| format!("  {d}")).collect();
            failed.push(format!(
                "{name}: {} Abweichungen, fehlende Ausgaenge {missing:?}\n{}\n--- Interpreter ---\n{}\n--- Board ---\n{}",
                diffs.len(),
                list.join("\n"),
                interpreted.lines().take(12).collect::<Vec<_>>().join("\n"),
                text.lines().filter(|l| l.starts_with("t=")).take(12).collect::<Vec<_>>().join("\n")
            ));
        }
        eprintln!("{} {name}: {} Abweichungen", board.name(), diffs.len());
    }
    failed
}

/// Programme, deren `f32` die FPU des Boards rechnet: Subnormale
/// (Flush-to-Zero) und Produkte vor der korrekt gerundeten Mathematik
/// (Rundungsmodus).
const HOSTILE_FPU_PROGRAMS: [&str; 2] = ["105_subnormals_f32.takt", "102_correct_math_f32.takt"];

/// **Eine verstellte FPU aendert nichts** (4.2, 12.11): FPSCR und FPDSCR
/// stehen vor dem Lauf auf Flush-to-Zero, Default-NaN und Rundung gegen
/// null. Jedes Programm rechnet wie der Interpreter, und nach dem Lauf steht
/// FPSCR, wie das Board es setzte — jeder Einstieg hat die IEEE-Umgebung
/// hergestellt und die des Aufrufers zurueckgegeben.
pub fn a_hostile_fpu_changes_nothing(board: &mut dyn Board) -> Vec<String> {
    let options = Options::fresh(TICKS).with_hostile_fpu();
    let mut failed = agreement_with(board, &HOSTILE_FPU_PROGRAMS, None, &options);
    let name = HOSTILE_FPU_PROGRAMS[0];
    match board.build(&board::corpus_path(name), &options).and_then(|elf| board.run(&elf, &options)) {
        Ok(text) if text.contains("takt fpscr 0x03c00000") => {}
        Ok(text) => {
            let lines: Vec<&str> = text.lines().filter(|l| l.starts_with("takt ")).collect();
            failed.push(format!(
                "{name}: FPSCR nach dem Lauf nicht, wie das Board es setzte:
{}",
                lines.join(
                    "
"
                )
            ));
        }
        Err(e) => failed.push(format!(
            "{name}: kein Lauf auf dem Board:
{e}"
        )),
    }
    failed
}

/// **Die kuratierten Natives rechnen auf dem Board wie auf dem Wirt, und
/// ihr Stack bleibt in der Zusage** (13.8, 4.5): je Funktion eine Meldung,
/// wenn nicht.
pub fn natives_agree(board: &mut dyn Board) -> Vec<String> {
    let program = board::corpus_path("01_minimal.takt");
    let name = board.name().to_string();
    match takt_conformance::bench::natives_on(board, &program) {
        Ok((natives, math)) => {
            let natives = natives
                .iter()
                .inspect(|r| eprintln!("{name} {}: Stack {} von {} Byte", r.native.name(), r.stack, r.contract))
                .filter(|r| !r.same_result() || !r.within_contract())
                .map(|r| {
                    format!(
                        "{}: abweichende Zeilen {:?}, Stack {} von {} Byte",
                        r.native.name(),
                        r.deviations,
                        r.stack,
                        r.contract
                    )
                })
                .collect::<Vec<_>>();
            let contract = takt_mir::analysis::stack::MATH_STACK;
            let math = math
                .iter()
                .inspect(|r| {
                    eprintln!("{name} {}: Stack {} von {contract} Byte, {} Zyklen", r.name(), r.stack, r.cycles)
                })
                .filter(|r| !r.same_result() || !r.within_contract())
                .map(|r| {
                    format!(
                        "{}: abweichende Zeilen {:?}, Stack {} von {contract} Byte",
                        r.name(),
                        r.deviations,
                        r.stack
                    )
                })
                .collect::<Vec<_>>();
            natives.into_iter().chain(math).collect()
        }
        Err(e) => vec![format!("kein Lauf der Natives: {e}")],
    }
}
