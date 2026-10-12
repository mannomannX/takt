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
    takt_embed::build::Program::new("takt/aging.takt").drivers("crate::Once").build();
    // Viele Instanzen eines Programms in Faeden (12.11): `tests/instances.rs`.
    takt_embed::build::Program::new("takt/instance.takt").drivers("crate::Seed").build();
    // `linux_rt` (12.2, 12.11): dieselben Programme in der Form `linux`, fuer
    // den Tick-Faden des Ports aus `takt-rt-linux` (`tests/linux_rt.rs`).
    let linux = |source: &str, prefix: &str| takt_embed::build::Program::new(source).form("linux").prefix(prefix);
    linux("takt/instance.takt", "instance_rt").drivers("crate::Seed").build();
    linux("takt/valve.takt", "valve_rt").drivers("crate::Tank").build();
    linux("takt/lifecycle.takt", "lifecycle_rt").build();
    linux("takt/idle_multirate.takt", "idle_multirate_rt").build();
    // Das Messprogramm von `takt bench` (13.8): `tests/bench.rs`.
    takt_embed::build::Bench::new().build();
}
