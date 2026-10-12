//! Takt-Syntax: Tokenizer, Parser, AST, S-Expression-Ausgabe und Formatter.
//!
//! Normative Quellen: `grammar/lexer.md` (Tokens), `grammar/takt.ebnf` (Satzform),
//! `plan/definition.md` Abschnitte 2.1 bis 2.5. Die Tests lesen diese Dateien
//! direkt, damit Spezifikation und Implementierung nicht auseinanderlaufen.

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::todo,
        clippy::unimplemented,
        clippy::dbg_macro,
        clippy::exit,
        deprecated,
        unreachable_code,
        unused_must_use,
        unused_assignments,
        unused_mut,
        unfulfilled_lint_expectations,
        reason = "Stufe 1 des Regelwerks (plan/cert.md Abschnitt 6)"
    )
)]

pub mod ast;
pub mod census;
pub mod edition;
pub mod fmt;
pub mod keywords;
pub mod lexer;
pub mod parser;
pub mod sexpr;
pub mod subtext;
pub mod token;

pub use edition::Edition;
pub use fmt::{format, format_snippet};
pub use lexer::{tokenize, tokenize_at, tokenize_file, tokenize_in};
pub use parser::{parse_expr, parse_file, parse_snippet};
pub use takt_diag::{Diagnostic, Severity, SourceMap, Span};
pub use token::{ErrorCode, Token, TokenKind, Tokens, Trivia, TriviaKind};
