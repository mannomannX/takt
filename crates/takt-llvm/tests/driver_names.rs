//! Treibernamen aus Adressen (8.10, 12.11): `Address::ident` macht aus
//! der Adresse den Namen der Treiberfunktion (`P_in_<ident>`). Zwei
//! Adressen, die denselben Namen ergeben, bekaemen denselben Treiber —
//! Pruefung 7 muss sie darum wie eine doppelte Adresse ablehnen.

fn errors(body: &str) -> Vec<String> {
    let src = format!("system:\n    language = 1\n    tick = 10 ms\n\n{body}");
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect()
}

/// GEN-022: `a/b_c` und `a_b/c`, `UI/led` und `ui/led` haben dieselbe
/// Treiberfunktion.
#[test]
fn addresses_with_the_same_driver_name_are_refused() {
    for (first, second) in [("a/b_c", "a_b/c"), ("UI/led", "ui/led")] {
        let a = takt_mir::pattern::Address::simple(first).ident();
        let b = takt_mir::pattern::Address::simple(second).ident();
        assert_eq!(a, b, "die Adressen ergeben nicht mehr denselben Namen; der Test braucht ein anderes Paar");
        let body = format!(
            "input x : bool @ hw(\"{first}\")\ninput y : bool @ hw(\"{second}\")\n\nmachine m:\n    initial RUN\n    state RUN:\n        when x and y: -> RUN\n"
        );
        let e = errors(&body);
        assert!(!e.is_empty(), "`{first}` und `{second}` teilen den Treiber `{a}`, und das Programm uebersetzt");
    }
}
