//! `flash_model` (8.11): Loeschen, Programmieren ueber `data.sent`,
//! Ruecklesen als Element von `rx`, und die Stromausfall-Injektion ueber
//! `cut_at_byte`.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

const CHANNELS: &str = "
output flash_cmd        : FlashCmd            @ hw(\"flash/cmd\")    with safe = NONE
output flash_data       : stream<u8>          @ hw(\"flash/tx\")     with max_rate = 4000 Hz, capacity = 64
input  flash_status     : FlashStatus         @ hw(\"flash/status\") with max_age = 10 ms
input  flash_rx         : stream<bytes<4096>> @ hw(\"flash/rx\")     with max_rate = 200 Hz, capacity = 2
output flash_status_sim : FlashStatus         @ sim(\"flash/status\")
output flash_rx_sim     : stream<bytes<4096>> @ sim(\"flash/rx\")     with capacity = 4096
input  flash_seed       : stream<u8>          @ hw(\"flash/seed\")   with max_rate = 200 kHz, capacity = 256
input  flash_seed_addr  : int in 0..4194304   @ hw(\"flash/seed_addr\")
output got   : int in 0..4096 @ hw(\"o/got\")   with safe = 0
output first : int in 0..255  @ hw(\"o/first\") with safe = 0
output phase : int in 0..9    @ hw(\"o/phase\") with safe = 0

fn first_byte(b: bytes<4096>) -> int:
    var v : int in 0..255 = 0
    var seen : bool = false
    for x in b:
        if not seen:
            v = x as int
            seen = true
    return v
";

fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{CHANNELS}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn trace(p: &Program, ticks: u64) -> String {
    run(p, &Trace::default(), &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

#[test]
fn erase_program_and_read_back() {
    let p = compile(
        "
instance flash = flash_model(cmd = flash_cmd, data = flash_data, status = flash_status_sim, rx = flash_rx_sim, seed = flash_seed, seed_addr = flash_seed_addr, sectors = 4, t_erase = 3 ms, t_program = 1 ms, cut_at_byte = 0)

machine dut:
    var msg : bytes<8> = default
    initial START
    state START:
        enter:
            msg.push(0x61)
            msg.push(0x62)
            msg.push(0x63)
        sequence:
            flash_cmd = ERASE(sector = 0)
            until flash_status == DONE timeout 20 ms
            flash_cmd = NONE
            until flash_status == IDLE timeout 20 ms
            phase = 1
            flash_cmd = PROGRAM(addr = 0, size = 3)
            send flash_data, msg
            until flash_status == DONE timeout 20 ms
            flash_cmd = NONE
            until flash_status == IDLE timeout 20 ms
            phase = 2
            flash_cmd = READ(addr = 0, size = 3)
            until flash_rx as c timeout 20 ms
            got = c.data.len
            first = first_byte(c.data)
            flash_cmd = NONE
            phase = 3
            -> DONE
    state DONE:
        when false: -> START
",
    );
    let t = trace(&p, 60);
    assert!(t.contains("out phase 3"), "die Sequenz laeuft durch:\n{t}");
    assert!(t.contains("out got 3"), "{t}");
    assert!(t.contains("out first 97"), "{t}");
    assert!(!t.contains("fault"), "{t}");
}

#[test]
fn a_cut_leaves_the_sector_erased_and_reports_an_error() {
    let p = compile(
        "
output cut : bool @ hw(\"o/cut\") with safe = false
output third : int in 0..255 @ hw(\"o/third\") with safe = 0
fn byte_at(b: bytes<4096>, pos: int) -> int:
    var v : int in 0..255 = 0
    var i : int in 0..4096 = 0
    for x in b:
        if i == pos:
            v = x as int
        i = i + 1
    return v
instance flash = flash_model(cmd = flash_cmd, data = flash_data, status = flash_status_sim, rx = flash_rx_sim, seed = flash_seed, seed_addr = flash_seed_addr, sectors = 4, t_erase = 3 ms, t_program = 1 ms, cut_at_byte = 2)

machine dut:
    var msg : bytes<8> = default
    initial START
    state START:
        enter:
            msg.push(0x61)
            msg.push(0x62)
            msg.push(0x63)
        sequence:
            flash_cmd = PROGRAM(addr = 0, size = 3)
            send flash_data, msg
            until flash_status == ERROR(code = 1) timeout 20 ms
            cut = true
            flash_cmd = NONE
            until flash_status == IDLE timeout 20 ms
            flash_cmd = READ(addr = 0, size = 3)
            until flash_rx as c timeout 20 ms
            got = c.data.len
            first = first_byte(c.data)
            third = byte_at(c.data, 2)
            flash_cmd = NONE
            phase = 3
            -> DONE
    state DONE:
        when false: -> START
",
    );
    let t = trace(&p, 60);
    assert!(t.contains("out cut true"), "{t}");
    assert!(t.contains("out phase 3"), "{t}");
    // Zwei Bytes kamen an, das dritte nicht: geloescht liest 0xFF.
    assert!(t.contains("out first 97"), "{t}");
    assert!(t.contains("out third 255"), "{t}");
    assert!(!t.contains("fault"), "{t}");
}

#[test]
fn an_address_outside_the_sectors_is_rejected() {
    let p = compile(
        "
instance flash = flash_model(cmd = flash_cmd, data = flash_data, status = flash_status_sim, rx = flash_rx_sim, seed = flash_seed, seed_addr = flash_seed_addr, sectors = 4, t_erase = 3 ms, t_program = 1 ms, cut_at_byte = 0)

machine dut:
    initial START
    state START:
        sequence:
            flash_cmd = ERASE(sector = 4)
            until flash_status == ERROR(code = 2) timeout 20 ms
            flash_cmd = NONE
            until flash_status == IDLE timeout 20 ms
            phase = 3
            -> DONE
    state DONE:
        when false: -> START
",
    );
    let t = trace(&p, 30);
    assert!(t.contains("out phase 3"), "{t}");
    assert!(!t.contains("fault"), "{t}");
}

#[test]
fn the_library_template_appears_only_when_instantiated() {
    let without = compile("\nmachine dut:\n    initial RUN\n    state RUN:\n        loop:\n            phase = 1\n");
    assert!(without.machines.iter().all(|m| m.name != "flash_model"), "{:?}", without.machines[0].name);
    assert_eq!(without.machines[0].name, "dut");
    let with = compile(
        "
instance flash = flash_model(cmd = flash_cmd, data = flash_data, status = flash_status_sim, rx = flash_rx_sim, seed = flash_seed, seed_addr = flash_seed_addr, sectors = 4, t_erase = 3 ms, t_program = 1 ms, cut_at_byte = 0)
",
    );
    let names: Vec<&str> = with.machines.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["flash_model", "flash"]);
    assert_eq!(with.machines[0].kind, takt_mir::machine::MachineKind::Template);
}

/// Eine Instanz des Modells mit diesem `cut_at_byte`.
fn instance(cut: &str) -> String {
    format!(
        "instance flash = flash_model(cmd = flash_cmd, data = flash_data, status = flash_status_sim, rx = flash_rx_sim, \
         seed = flash_seed, seed_addr = flash_seed_addr, sectors = 4, t_erase = 3 ms, t_program = 1 ms, cut_at_byte = {cut})\n"
    )
}

/// Die Statuszeilen des Modells im Trace, als `(Tick, Wert)`.
fn status_lines(t: &str) -> Vec<(u64, String)> {
    t.lines()
        .filter_map(|l| {
            let (tick, rest) = l.strip_prefix("t=")?.split_once(' ')?;
            Some((tick.parse().ok()?, rest.strip_prefix("out flash_status_sim ")?.to_string()))
        })
        .collect()
}

#[test]
fn the_model_stays_busy_for_the_erase_and_program_times() {
    // 8.11: Loeschen haelt `t_erase` lang BUSY, Programmieren bis zum
    // letzten Byte und dann `t_program`; erst danach DONE. Das Modell
    // liest `cmd` mit Unit-Delay: ERASE aus Tick 0 sieht es in Tick 1.
    let p = compile(&format!(
        "{}
machine dut:
    var msg : bytes<8> = default
    initial START
    state START:
        enter:
            msg.push(0x61)
        sequence:
            flash_cmd = ERASE(sector = 0)
            until flash_status == DONE timeout 20 ms
            flash_cmd = NONE
            until flash_status == IDLE timeout 20 ms
            flash_cmd = PROGRAM(addr = 0, size = 1)
            send flash_data, msg
            until flash_status == DONE timeout 20 ms
            flash_cmd = NONE
            phase = 3
            -> DONE
    state DONE:
        when false: -> START
",
        instance("0")
    ));
    let t = trace(&p, 30);
    let lines = status_lines(&t);
    let busy = |from: u64| lines.iter().find(|(k, v)| *k >= from && v == "BUSY").map(|(k, _)| *k);
    let done_after = |from: u64| lines.iter().find(|(k, v)| *k > from && v == "DONE").map(|(k, _)| *k);
    let erase = busy(0).unwrap_or_else(|| panic!("kein BUSY:\n{t}"));
    assert_eq!(erase, 1, "ERASE aus Tick 0 wirkt in Tick 1:\n{t}");
    assert_eq!(done_after(erase), Some(erase + 3), "DONE nach t_erase = 3 ms:\n{t}");
    let program = busy(erase + 4).unwrap_or_else(|| panic!("kein zweites BUSY:\n{t}"));
    let done = done_after(program).unwrap_or_else(|| panic!("kein DONE nach PROGRAM:\n{t}"));
    assert!(done > program, "PROGRAM_DONE haelt t_program = 1 ms BUSY:\n{t}");
    assert!(t.contains("out phase 3") && !t.contains("fault"), "{t}");
}

/// Ein Programm, das nacheinander programmiert und alles zurueckliest.
fn programs(cut: &str, steps: &str) -> String {
    format!(
        "{}
output cut : bool @ hw(\"o/cut\") with safe = false
output sixth : int in 0..255 @ hw(\"o/sixth\") with safe = 0
fn byte_at(b: bytes<4096>, pos: int) -> int:
    var v : int in 0..255 = 0
    var i : int in 0..4096 = 0
    for x in b:
        if i == pos:
            v = x as int
        i = i + 1
    return v

machine dut:
    var abc : bytes<8> = default
    var de : bytes<8> = default
    initial START
    state START:
        enter:
            abc.push(0x61)
            abc.push(0x62)
            abc.push(0x63)
            de.push(0x64)
            de.push(0x65)
            de.push(0x66)
        sequence:
{steps}            flash_cmd = READ(addr = 0, size = 6)
            until flash_rx as c timeout 20 ms
            got = c.data.len
            first = first_byte(c.data)
            sixth = byte_at(c.data, 5)
            flash_cmd = NONE
            phase = 3
            -> DONE
    state DONE:
        when false: -> START
",
        instance(cut)
    )
}

/// Ein PROGRAM-Schritt der Sequenz mit erwartetem Ausgang.
fn program_step(addr: u32, size: u32, data: &str, until: &str) -> String {
    format!(
        "            flash_cmd = PROGRAM(addr = {addr}, size = {size})\n            send flash_data, {data}\n            \
         until flash_status == {until} timeout 20 ms\n            flash_cmd = NONE\n            \
         until flash_status == IDLE timeout 20 ms\n"
    )
}

#[test]
fn the_cut_counts_across_program_commands_and_happens_once() {
    // 8.11: `cut_at_byte = 5` zaehlt ueber alle PROGRAM-Kommandos: drei
    // Bytes im ersten, das zweite bricht nach zwei ab. Der Schnitt kommt
    // einmal; ein drittes PROGRAM schreibt das sechste Byte.
    let steps = program_step(0, 3, "abc", "DONE")
        + &program_step(3, 3, "de", "ERROR(code = 1)")
        + "            cut = true\n"
        + &program_step(5, 1, "abc", "DONE");
    let t = trace(&compile(&programs("5", &steps)), 80);
    assert!(t.contains("out cut true") && t.contains("out phase 3"), "{t}");
    assert!(t.contains("out got 6") && t.contains("out first 97"), "{t}");
    assert!(t.contains("out sixth 97"), "das dritte PROGRAM schrieb `a` an Adresse 5:\n{t}");
    assert!(!t.contains("fault"), "{t}");
}

#[test]
fn a_range_past_the_last_sector_is_an_error_and_the_last_byte_is_not() {
    // 8.11: vier Sektoren enden bei 16384; Byte 16383 ist das letzte.
    for (cmd, ok) in [
        ("PROGRAM(addr = 16383, size = 1)", true),
        ("PROGRAM(addr = 16383, size = 2)", false),
        ("READ(addr = 16383, size = 1)", true),
        ("READ(addr = 16383, size = 2)", false),
    ] {
        let until = if ok { "DONE" } else { "ERROR(code = 2)" };
        let send = if cmd.starts_with("PROGRAM") { "            send flash_data, b\n" } else { "" };
        let p = compile(&format!(
            "{}
machine dut:
    var b : bytes<8> = default
    initial START
    state START:
        enter:
            b.push(0x61)
        sequence:
            flash_cmd = {cmd}
{send}            until flash_status == {until} timeout 20 ms
            flash_cmd = NONE
            phase = 3
            -> DONE
    state DONE:
        when false: -> START
",
            instance("0")
        ));
        let t = trace(&p, 30);
        assert!(t.contains("out phase 3") && !t.contains("fault"), "{cmd}:\n{t}");
    }
}

#[test]
fn seeding_counts_neither_as_busy_nor_towards_the_cut() {
    // 8.11: `seed` fuellt den Flash ohne Busy und ohne Zaehlung fuer
    // `cut_at_byte`: Nach drei gesaeten Bytes schneidet `cut_at_byte = 2`
    // erst beim zweiten programmierten. Gesaet wird in Tick 1: Im
    // Eintritts-Tick ist das Fenster leer (9.6).
    let steps = "            wait 3 ms\n".to_string() + &program_step(3, 3, "de", "ERROR(code = 1)");
    let p = compile(&programs("2", &steps));
    let seed = "t=0 in flash_seed_addr 0\nt=1 in flash_seed 120\nt=1 in flash_seed 121\nt=1 in flash_seed 122\n";
    let stimulus = Trace::parse(seed).expect("Stimulus");
    let t = run(&p, &stimulus, &RunOptions { ticks: 60, ..Default::default() }).expect("Lauf").trace.render();
    let lines = status_lines(&t);
    let first_busy = lines.iter().find(|(_, v)| v == "BUSY").map(|(k, _)| *k).unwrap_or_else(|| panic!("{t}"));
    assert!(first_busy >= 4, "das Saeen macht nicht BUSY:\n{t}");
    assert!(t.contains("out first 120"), "die gesaeten Bytes stehen:\n{t}");
    // Bytes 3 und 4 programmiert, Byte 5 geloescht.
    assert!(t.contains("out sixth 255") && t.contains("out phase 3"), "{t}");
}

#[test]
fn a_param_may_parameterise_the_model_and_a_tunable_may_not() {
    // 8.11: „Ein Instanzargument darf ein `param` sein (kein `tunable`)".
    let body = |decl: &str| format!("{decl}\n{}", instance("CUT"));
    let p = compile(&body("param CUT : int in 0..100 = 2"));
    assert!(p.machines.iter().any(|m| m.name == "flash"), "die Instanz steht");
    let src = format!("{HEAD}{CHANNELS}{}", body("tunable param CUT : int in 0..100 = 2"));
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let errors: Vec<String> = takt_sema::compile(&src, &options)
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| format!("{d}"))
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("[SC-3]") && errors[0].contains("ein `tunable param` ist kein Instanzargument"),
        "{errors:?}"
    );
}

/// SEM1-038: Das Modell haelt hoechstens 256 beschriebene Chunks (64 KiB).
/// Der 257. scheitert nicht still: `check` nach dem `insert` faultet das
/// Modell mit der Grenze in der Meldung. 256 Chunks gehen durch.
#[test]
fn more_than_256_written_chunks_fault_the_model() {
    let p = compile(&format!(
        "{}
machine dut:
    initial WAIT
    state WAIT:
        loop:
            phase = 1
",
        instance("0")
    ));
    // Je Tick ein Byte in einen neuen Chunk; ab Tick 1, weil das Fenster im
    // Eintritts-Tick leer ist (9.6).
    let seed: String = (0..257u32)
        .map(|i| format!("t={} in flash_seed_addr {}\nt={} in flash_seed 7\n", i + 1, i * 256, i + 1))
        .collect();
    let stimulus = Trace::parse(&seed).expect("Stimulus");
    let t = run(&p, &stimulus, &RunOptions { ticks: 260, ..Default::default() }).expect("Lauf").trace.render();
    let faults: Vec<&str> = t.lines().filter(|l| l.contains(" fault ")).collect();
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert!(faults[0].starts_with("t=257 fault flash CheckFailed"), "{faults:?}");
    assert!(faults[0].contains("256") && faults[0].contains("64 KiB"), "{faults:?}");
}

/// 8.11 (fuenfte Runde): `cut_at_chunk = 5` schneidet das Loeschen nach
/// fuenf Chunks. Die Chunks davor lesen 0xFF, die ab dem Schnitt behalten,
/// was gesaet war, und der Vorgang endet mit ERROR(code = 1). Ohne den
/// Parameter (Vorgabe 0) loescht dasselbe ERASE den ganzen Sektor.
#[test]
fn a_cut_in_the_middle_of_an_erase_keeps_the_rest_of_the_sector() {
    let program = |chunk: &str, until: &str| {
        format!(
            "
output last_erased : int in 0..255 @ hw(\"o/last_erased\") with safe = 0
output first_kept  : int in 0..255 @ hw(\"o/first_kept\") with safe = 0
fn byte_at(b: bytes<4096>, pos: int) -> int:
    var v : int in 0..255 = 0
    var i : int in 0..4096 = 0
    for x in b:
        if i == pos:
            v = x as int
        i = i + 1
    return v
instance flash = flash_model(cmd = flash_cmd, data = flash_data, status = flash_status_sim, rx = flash_rx_sim, seed = flash_seed, seed_addr = flash_seed_addr, sectors = 4, t_erase = 4 ms, t_program = 1 ms, cut_at_byte = 0{chunk})

machine dut:
    initial START
    state START:
        sequence:
            wait 20 ms
            flash_cmd = ERASE(sector = 0)
            until flash_status == {until} timeout 20 ms
            flash_cmd = NONE
            until flash_status == IDLE timeout 20 ms
            flash_cmd = READ(addr = 0, size = 4096)
            until flash_rx as c timeout 20 ms
            last_erased = byte_at(c.data, 4 * 256)
            first_kept = byte_at(c.data, 5 * 256)
            flash_cmd = NONE
            phase = 3
            -> DONE
    state DONE:
        when false: -> START
"
        )
    };
    // Je Chunk ein gesaetes Byte 0x11 an seinem Anfang, ab Tick 1 (9.6).
    let seed: String = (0..16u32)
        .map(|i| format!("t={} in flash_seed_addr {}\nt={} in flash_seed 17\n", i + 1, i * 256, i + 1))
        .collect();
    let stimulus = Trace::parse(&seed).expect("Stimulus");
    let run_it = |src: &str| {
        let p = compile(src);
        run(&p, &stimulus, &RunOptions { ticks: 80, ..Default::default() }).expect("Lauf").trace.render()
    };
    let t = run_it(&program(", cut_at_chunk = 5", "ERROR(code = 1)"));
    assert!(t.contains("out phase 3") && !t.contains(" fault "), "{t}");
    assert!(t.contains("out last_erased 255") && t.contains("out first_kept 17"), "{t}");
    let t = run_it(&program("", "DONE"));
    assert!(t.contains("out phase 3") && !t.contains(" fault "), "{t}");
    assert!(t.contains("out last_erased 255") && t.contains("out first_kept 255"), "{t}");
}
