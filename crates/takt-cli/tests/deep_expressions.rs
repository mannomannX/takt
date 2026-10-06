//! **Ein Ausdruck an der Grenze braucht keinen Stapel vom Aufrufer** (2.1,
//! FB-433): Sema, Interpreter und Codegen bemessen ihren Stapel selbst
//! (`takt_diag::stack`). Gerufen von einem Thread mit 256 KiB — weniger als
//! jeder Hauptthread —, nehmen sie eine Summe aus 256 Gliedern, eine Kette
//! in 15 Klammerebenen und die Summe unter 55 Anweisungsebenen an.

const HEAD: &str = "system:\n    language = 1\n    tick     = 10 ms\n\noutput y : int @ hw(\"o/y\") with safe = 0\n\n\
                    machine m:\n    var a : int in 0..1 = 1\n\n    initial RUN\n\n    state RUN:\n        loop:\n";

/// Die drei Formen an der Grenze aus 2.1.
fn sources() -> [String; 3] {
    let sum = vec!["a"; 256].join(" + ");
    let mut nested = "a".to_string();
    for _ in 0..15 {
        nested = format!("({nested}){}", " + a".repeat(16));
    }
    let mut deep = HEAD.to_string();
    for level in 0..55 {
        deep.push_str(&format!("{}if a > 0:\n", " ".repeat(12 + 4 * level)));
    }
    deep.push_str(&format!("{}y = {sum}\n", " ".repeat(12 + 4 * 55)));
    [format!("{HEAD}            y = {sum}\n"), format!("{HEAD}            y = {nested}\n"), deep]
}

#[test]
fn an_expression_at_the_limit_needs_no_stack_from_the_caller() {
    for src in sources() {
        takt_diag::stack::with_stack(256 << 10, || {
            let out = takt_sema::compile(&src, &takt_sema::Options::default());
            let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
            assert!(errors.is_empty(), "{}", errors.join("\n"));
            let p = out.program.expect("Programm");
            let options = takt_interp::RunOptions { ticks: 2, ..Default::default() };
            takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf");
            let lowered = takt_llvm::lower::program(&p, "x86_64-unknown-linux-gnu", &Default::default());
            assert!(lowered.skipped.is_empty(), "nichts ausgelassen");
        });
    }
}
