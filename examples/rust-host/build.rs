//! Baut die Programme mit `takt build --emit embed` fuer das Ziel dieses Baus.

fn main() {
    takt_embed::build::Program::new("takt/valve.takt").drivers("crate::Tank").build();
    // Programme ohne Treiber, die den Rahmen gegen den Interpreter halten
    // (13.1): `tests/frame.rs`.
    for program in [
        "takt/drop_oldest.takt",
        "takt/exit_fault.takt",
        "takt/lifecycle.takt",
        "takt/idle_multirate.takt",
        "takt/tuning.takt",
        "takt/byte_ring.takt",
    ] {
        takt_embed::build::Program::new(program).build();
    }
    for program in ["takt/overflow_late.takt", "takt/faulted_pending.takt"] {
        takt_embed::build::Program::new(program).drivers("crate::Script").build();
    }
    takt_embed::build::Program::new("takt/long_line.takt").drivers("crate::Line").build();
    takt_embed::build::Program::new("takt/sys_inputs.takt").drivers("crate::Host").build();
    takt_embed::build::Program::new("takt/float_elements.takt").drivers("crate::Bits").build();
    takt_embed::build::Program::new("takt/overfull_tx.takt").drivers("crate::Overfull").build();
}
