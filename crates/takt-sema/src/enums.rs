//! Pruefung 51 (Referenz 2.5, 10): `match` ueber ein offenes Enum braucht
//! `case _`. Ohne Typinferenz (M1) bestimmt diese Naeherung das Enum aus der
//! deklarierten Art des Subjekts oder aus den Variantennamen der Zweige;
//! unbestimmbare Subjekte bleiben ohne Meldung.

use std::collections::HashMap;

use takt_diag::{Diagnostic, Span};
use takt_syntax::Edition;
use takt_syntax::ast::*;

use crate::visit::{Declared, Scopes, Visitor, walk_file};

/// Code der Pruefung.
pub const CODE: &str = "SC-51";

/// Prueft alle `match`-Anweisungen der Datei.
pub fn check(file: &File, edition: Edition) -> Vec<Diagnostic> {
    let mut enums: HashMap<String, bool> = HashMap::new();
    let mut variants: HashMap<String, Vec<String>> = HashMap::new();
    for (name, vars) in edition.open_enums() {
        enums.insert((*name).to_string(), true);
        for v in *vars {
            variants.entry((*v).to_string()).or_default().push((*name).to_string());
        }
    }
    for item in &file.items {
        if let Item::Enum(e) = item {
            enums.insert(e.name.name.clone(), e.open);
            for v in &e.variants {
                variants.entry(v.name.name.clone()).or_default().push(e.name.name.clone());
            }
        }
    }
    let mut visitor = Matches { enums, variants, out: Vec::new() };
    walk_file(file, &mut visitor);
    visitor.out
}

struct Matches {
    enums: HashMap<String, bool>,
    variants: HashMap<String, Vec<String>>,
    out: Vec<Diagnostic>,
}

impl Matches {
    /// Enum des Subjekts, soweit syntaktisch bestimmbar.
    fn subject_enum(&self, subject: &Expr, cases: &[Case], scopes: &Scopes) -> Option<String> {
        match &subject.kind {
            ExprKind::Ident(id) => {
                if let Some(Declared::Type(ty)) = scopes.lookup(&id.name) {
                    if let TypeKind::Named { name, wrap: None } = &ty.kind {
                        if self.enums.contains_key(&name.name) {
                            return Some(name.name.clone());
                        }
                    }
                }
            }
            ExprKind::Member { name, args: None, base } => {
                if name.name == "reason" {
                    return Some("Reason".into());
                }
                if name.name == "kind" && matches!(&base.kind, ExprKind::Ident(b) if b.name == "last_fault") {
                    return Some("FaultKind".into());
                }
            }
            _ => {}
        }
        // Variantennamen der Zweige: eindeutig, wenn alle zu genau einem Enum gehoeren
        let mut candidate: Option<String> = None;
        for c in cases {
            let CasePattern::Variant { name, .. } = &c.pattern else { continue };
            let owners = self.variants.get(&name.name)?;
            if owners.len() != 1 {
                return None;
            }
            match &candidate {
                None => candidate = Some(owners[0].clone()),
                Some(prev) if prev == &owners[0] => {}
                Some(_) => return None,
            }
        }
        candidate
    }
}

impl Visitor for Matches {
    fn stmt(&mut self, stmt: &Stmt, scopes: &Scopes) {
        let StmtKind::Match { subject, cases } = &stmt.kind else { return };
        if cases.iter().any(|c| matches!(c.pattern, CasePattern::Wild)) {
            return;
        }
        let Some(enum_name) = self.subject_enum(subject, cases, scopes) else { return };
        if self.enums.get(&enum_name) != Some(&true) {
            return;
        }
        let span = Span { file: stmt.span.file, start: stmt.span.start, end: stmt.span.start + 5 };
        self.out.push(
            Diagnostic::error(CODE, span, format!("`match` ueber das offene Enum `{enum_name}` braucht `case _`"))
                .with_suggestion("`case _:` ergaenzen; offene Enums bekommen neue Varianten, ohne bestehende Programme zu brechen (2.5)"),
        );
    }
}
