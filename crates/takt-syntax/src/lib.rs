//! Takt-Syntax: Tokenizer, Parser, AST, S-Expression-Ausgabe und Formatter.
//!
//! Normative Quellen: `grammar/lexer.md` (Tokens), `grammar/takt.ebnf` (Satzform),
//! `plan/definition.md` Abschnitte 2.1 bis 2.5. Die Tests lesen diese Dateien
//! direkt, damit Spezifikation und Implementierung nicht auseinanderlaufen.

pub mod ast;
pub mod fmt;
pub mod keywords;
pub mod lexer;
pub mod parser;
pub mod sexpr;
pub mod subtext;
pub mod token;

pub use fmt::{format, format_snippet};
pub use lexer::tokenize;
pub use parser::{ParseError, parse_file, parse_snippet};
pub use token::{ErrorCode, LexError, Token, TokenKind, Tokens, Trivia, TriviaKind};
