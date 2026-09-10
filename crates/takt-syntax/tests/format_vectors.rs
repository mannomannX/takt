//! Liest die Vektoren aus grammar/format.md: jeder ```fmt-Block enthaelt
//! Eingabe, `---`, erwartete Ausgabe. Die Ausgabe muss ausserdem kanonisch sein.

use takt_syntax::format_snippet;

fn vectors() -> Vec<(usize, String, String)> {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../grammar/format.md"))
        .expect("grammar/format.md lesbar");
    let mut out = Vec::new();
    let mut lines = text.lines().enumerate().peekable();
    while let Some((n, line)) = lines.next() {
        if line.trim() != "```fmt" {
            continue;
        }
        let mut input = String::new();
        let mut expected = String::new();
        let mut in_output = false;
        for (_, l) in lines.by_ref() {
            if l.trim() == "```" {
                break;
            }
            if l == "---" && !in_output {
                in_output = true;
                continue;
            }
            let target = if in_output { &mut expected } else { &mut input };
            target.push_str(l);
            target.push('\n');
        }
        out.push((n + 1, input, expected));
    }
    out
}

#[test]
fn vectors_format_as_specified() {
    let vectors = vectors();
    assert!(vectors.len() >= 8, "zu wenige Vektoren: {}", vectors.len());
    let mut failures = Vec::new();
    for (line, input, expected) in vectors {
        match format_snippet(&input) {
            Ok(actual) if actual == expected => {}
            Ok(actual) => {
                failures.push(format!("Vektor Zeile {line}:\n--- erwartet ---\n{expected}--- erhalten ---\n{actual}"))
            }
            Err(errors) => failures.push(format!("Vektor Zeile {line}: {}", errors[0])),
        }
        if let Ok(again) = format_snippet(&expected) {
            if again != expected {
                failures.push(format!("Vektor Zeile {line}: Ausgabe ist nicht kanonisch:\n{again}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
