//! Takt-Syntax: Tokenizer, spaeter Parser, AST und Formatter.
//!
//! Normative Quellen: `grammar/lexer.md` (Tokens), `grammar/takt.ebnf` (Satzform),
//! `plan/definition.md` Abschnitte 2.1 bis 2.5. Die Tests lesen diese Dateien
//! direkt, damit Spezifikation und Implementierung nicht auseinanderlaufen.

pub mod keywords;
pub mod lexer;
pub mod subtext;
pub mod token;

pub use lexer::tokenize;
pub use token::{ErrorCode, LexError, Token, TokenKind, Tokens, Trivia, TriviaKind};
