//! Zeigt, was die Runtime in ihrer Umgebung vorfindet (12.2).
//!
//! `cargo run -p takt-rt-linux --example befund`
//!
//! Ohne `chrt` und `taskset` meldet er, was fehlt — und genau das ist der
//! Zweck: Eine Zeitgarantie, die still ausfaellt, ist schlimmer als eine,
//! die fehlt.

fn main() {
    let g = takt_rt_linux::prepare(None);
    println!("Befund (12.2):");
    for line in g.header_lines() {
        println!("  {line}");
    }
    if g.is_complete() {
        println!("\nDie Zeitgarantie aus 12.2 haelt.");
    } else {
        println!("\nDie Zeitgarantie aus 12.2 haelt NICHT:");
        for m in g.missing() {
            println!("  - {m}");
        }
    }
}
