//! Baut das Programm mit `takt build --emit embed` fuer das Ziel dieses Baus.

fn main() {
    takt_embed::build::Program::new("takt/valve.takt").drivers("crate::Tank").build();
}
