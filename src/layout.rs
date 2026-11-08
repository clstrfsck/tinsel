//! Layout pass (SPEC 3.1). Inserts `BlockOpen`, `BlockSep` and `BlockClose`
//! tokens based on indentation, so the parser never looks at columns.
//!
//! - The whole program is one block at column 1; its entries are the
//!   top-level items, separated by `BlockSep`.
//! - `where`, `match`, `example` and `answers` open a block whose entries
//!   start on the following lines, indented deeper than the line holding the
//!   keyword.
//! - Inside brackets, line breaks are ignored, unless a block was opened
//!   inside those brackets.

use crate::error::{LineIndex, Span};
use crate::lexer::{Tok, Token};

enum Ctx {
    Block(usize),
    Bracket,
}

pub fn layout(src: &str, tokens: Vec<Token>) -> Vec<Token> {
    let index = LineIndex::new(src);
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len() + 16);
    let mut stack = vec![Ctx::Block(1)];
    // Indentation of the line that opened a block that hasn't started yet.
    // Also records the context depth then, so that brackets opened after the
    // keyword (as in `match f(x)`) don't end or start the block early.
    let mut pending: Option<(usize, usize)> = None;
    let mut line_indent = 1;
    let mut prev_end_line = 0;

    let virt = |tok: Tok, at: Span| Token {
        tok,
        span: Span::new(at.start, at.start),
    };

    for token in tokens {
        let (line, col) = index.line_col(src, token.span.start);
        let first_on_line = line != prev_end_line;
        prev_end_line = index.line(token.span.end.saturating_sub(1).max(token.span.start));

        if first_on_line {
            line_indent = col;
            let mut new_line = true;
            if let Some((opener_indent, _)) = pending.take_if(|(_, depth)| *depth == stack.len()) {
                out.push(virt(Tok::BlockOpen, token.span));
                if col > opener_indent {
                    stack.push(Ctx::Block(col));
                    new_line = false;
                } else {
                    out.push(virt(Tok::BlockClose, token.span));
                }
            }
            if new_line {
                while let Some(Ctx::Block(c)) = stack.last() {
                    let c = *c;
                    if col < c && stack.len() > 1 {
                        stack.pop();
                        out.push(virt(Tok::BlockClose, token.span));
                    } else {
                        if col == c && !out.is_empty() {
                            out.push(virt(Tok::BlockSep, token.span));
                        }
                        break;
                    }
                }
            }
        }

        let tok = token.tok;

        match tok {
            Tok::LParen | Tok::LBracket | Tok::LBrace => stack.push(Ctx::Bracket),
            Tok::RParen | Tok::RBracket | Tok::RBrace => {
                // A block keyword inside these brackets with no block after it.
                let bracket = stack
                    .iter()
                    .rposition(|c| matches!(c, Ctx::Bracket))
                    .unwrap_or(0);
                if pending.take_if(|(_, depth)| *depth > bracket).is_some() {
                    out.push(virt(Tok::BlockOpen, token.span));
                    out.push(virt(Tok::BlockClose, token.span));
                }
                while let Some(Ctx::Block(_)) = stack.last() {
                    if stack.len() == 1 {
                        break;
                    }
                    stack.pop();
                    out.push(virt(Tok::BlockClose, token.span));
                }
                if let Some(Ctx::Bracket) = stack.last() {
                    stack.pop();
                }
            }
            Tok::Where | Tok::Match | Tok::Example | Tok::Answers => {
                pending = Some((line_indent, stack.len()))
            }
            _ => {}
        }
        out.push(Token {
            tok,
            span: token.span,
        });
    }

    let end = Span::new(src.len(), src.len());
    if pending.is_some() {
        out.push(virt(Tok::BlockOpen, end));
        out.push(virt(Tok::BlockClose, end));
    }
    while stack.len() > 1 {
        if let Some(Ctx::Block(_)) = stack.pop() {
            out.push(virt(Tok::BlockClose, end));
        }
    }
    out.push(virt(Tok::Eof, end));
    out
}
