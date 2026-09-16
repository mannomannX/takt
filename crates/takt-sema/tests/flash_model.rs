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
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
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
instance flash = flash_model(cmd = flash_cmd, data = flash_data, status = flash_status_sim, rx = flash_rx_sim, sectors = 4, t_erase = 3 ms, t_program = 1 ms, cut_at_byte = 0)

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
instance flash = flash_model(cmd = flash_cmd, data = flash_data, status = flash_status_sim, rx = flash_rx_sim, sectors = 4, t_erase = 3 ms, t_program = 1 ms, cut_at_byte = 2)

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
instance flash = flash_model(cmd = flash_cmd, data = flash_data, status = flash_status_sim, rx = flash_rx_sim, sectors = 4, t_erase = 3 ms, t_program = 1 ms, cut_at_byte = 0)

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
instance flash = flash_model(cmd = flash_cmd, data = flash_data, status = flash_status_sim, rx = flash_rx_sim, sectors = 4, t_erase = 3 ms, t_program = 1 ms, cut_at_byte = 0)
",
    );
    let names: Vec<&str> = with.machines.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["flash_model", "flash"]);
    assert_eq!(with.machines[0].kind, takt_mir::machine::MachineKind::Template);
}
