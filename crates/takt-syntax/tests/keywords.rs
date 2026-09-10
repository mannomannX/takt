//! Paritaet der Schluesselwortlisten mit Referenz 2.2 (plan/definition.md).

use takt_syntax::keywords::{CONTEXTUAL, KEYWORDS, RESERVED};

fn section_2_2() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plan/definition.md");
    let text = std::fs::read_to_string(path).expect("plan/definition.md lesbar");
    let start = text.find("### 2.2 Schlüsselwörter").expect("Abschnitt 2.2");
    let block = &text[start..];
    let open = block.find("```\n").expect("Codeblock") + 4;
    let close = block[open..].find("```").expect("Blockende");
    block[open..open + close].to_string()
}

#[test]
fn keywords_match_reference() {
    let mut keywords = Vec::new();
    let mut reserved = Vec::new();
    for line in section_2_2().lines() {
        if let Some(rest) = line.strip_prefix("reserviert") {
            let list = rest.split_once(':').map(|(_, l)| l).unwrap_or("");
            reserved.extend(list.split_whitespace().map(str::to_string));
        } else {
            keywords.extend(line.split_whitespace().map(str::to_string));
        }
    }
    keywords.sort();
    keywords.dedup();
    reserved.sort();
    let ours: Vec<String> = KEYWORDS.iter().map(|k| k.to_string()).collect();
    let ours_reserved: Vec<String> = RESERVED.iter().map(|k| k.to_string()).collect();
    assert_eq!(ours, keywords, "Schluesselwoerter weichen von 2.2 ab");
    assert_eq!(ours_reserved, reserved, "reservierte Woerter weichen von 2.2 ab");
}

#[test]
fn lists_are_sorted_for_binary_search() {
    assert!(KEYWORDS.windows(2).all(|w| w[0] < w[1]));
    assert!(RESERVED.windows(2).all(|w| w[0] < w[1]));
    assert!(CONTEXTUAL.windows(2).all(|w| w[0] < w[1]));
}

/// Alle Wortterminale der Grammatik ohne Schluesselwoerter und reservierte Woerter.
#[test]
fn contextual_words_match_grammar() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../grammar/takt.ebnf");
    let text = std::fs::read_to_string(path).expect("grammar/takt.ebnf lesbar");
    let mut words = Vec::new();
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let next_comment = rest.find("(*").unwrap_or(rest.len());
        let (chunk, tail) = rest.split_at(next_comment);
        let mut fields = chunk.split('"');
        fields.next();
        while let Some(word) = fields.next() {
            fields.next();
            let is_word = word.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if is_word && !KEYWORDS.contains(&word) && !RESERVED.contains(&word) {
                words.push(word.to_string());
            }
        }
        rest = match tail.find("*)") {
            Some(i) => &tail[i + 2..],
            None => "",
        };
    }
    words.sort();
    words.dedup();
    let ours: Vec<String> = CONTEXTUAL.iter().map(|k| k.to_string()).collect();
    assert_eq!(ours, words, "kontextuelle Terminale weichen von der Grammatik ab");
}
