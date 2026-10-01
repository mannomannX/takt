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
        t.lines().filter(|l| l.starts_with("t=") && l.contains(what)).map(str::to_string).collect()
    };
    if lines(&interpreted, " driver ") != lines(&text, " driver ") {
        failed.push(format!(
            "andere `driver`-Zeilen:\n{:?}\n{:?}",
            lines(&interpreted, " driver "),
            lines(&text, " driver ")
        ));
    }
    let raised: Vec<String> =
        takt_board_support::edge_probe::DRIVER_FAULTS.iter().map(|k| format!("t={k} runtime Driver o")).collect();
    if lines(&text, " runtime Driver ") != raised {
        failed.push(format!("das Board erhob {:?}, erwartet {raised:?}", lines(&text, " runtime Driver ")));
    }
    eprintln!("{} Treiberrand: {} Abweichungen", board.name(), failed.len());
    failed
}

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
        if edge_probe::DRIVER_FAULTS.contains(&tick) {
            let _ = writeln!(s, "t={tick} runtime Driver o");
        }
    }
    s
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

/// **Was das Programm nicht liest, zeichnet der Rahmen auf** (8.2, 12.5,
/// M10 Schritt 29d). `recorded.takt` importiert `recorded.hw` und nennt
/// keinen der Kanaele `edge_r/*`; das Pruefgeraet liefert vier davon. Der
/// Rahmen schreibt sie als Metazeile `rec` in Tick 0 und bei jeder
/// Aenderung — ein eigener Zeitstempel ist eine —, `Bad` mit Grund
/// `Driver`, eine Diskriminante, die das Enum nicht kennt, mit Grund
/// `OutOfRange`; der Kanal ohne Treiber und der Output erscheinen nie. Die
/// Ausgaben stimmen mit dem Interpreter ueberein, der keine `rec`-Zeile
/// schreibt.
pub fn unread_channels_are_recorded(board: &mut dyn Board) -> Vec<String> {
    const RECORD_TICKS: u64 = 10;
    const RECORDED: [&str; 13] = [
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
/// `only` beschraenkt den Lauf auf ein Programm (`TAKT_…_ONLY`).
pub fn agreement(board: &mut dyn Board, names: &[&str], only: Option<&str>) -> Vec<String> {
    agreement_with(board, names, only, &Options::fresh(TICKS))
}

/// Wie [`agreement`], mit anderen Optionen des Baus — etwa im Profil
/// `rtos` (12.8).
pub fn agreement_with(board: &mut dyn Board, names: &[&str], only: Option<&str>, options: &Options) -> Vec<String> {
    let mut failed = Vec::new();
    for name in names.iter().filter(|n| only.is_none_or(|o| o == **n)) {
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
