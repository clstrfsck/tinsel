//! Lexer (SPEC 3), built on `logos`. Produces raw tokens with byte spans;
//! string contents are kept raw so that the parser can treat them as either
//! interpolated strings or string patterns.

use crate::error::{Error, LineIndex, Span};
use logos::Logos;
use std::rc::Rc;

#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
#[logos(skip r"[ \t\r\n\f]+")]
#[logos(skip(r"#[^\n]*", allow_greedy = true))]
enum Raw {
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Ident,
    #[regex(r"[0-9][0-9_]*")]
    Int,
    #[regex(r"'([^'\\\n]|\\[^\n])'")]
    Char,
    /// Interpolations `{...}` may contain nested string literals.
    #[token("\"", lex_string)]
    Str,
    #[token("\"\"\"", lex_triple)]
    TripleStr,

    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
    #[token("=")]
    Assign,
    #[token("=>")]
    Arrow,
    #[token(".")]
    Dot,
    #[token("..")]
    DotDot,
    #[token("..<")]
    DotDotLt,
    #[token("@")]
    At,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("~")]
    Tilde,
    #[token("*")]
    Star,
    #[token("**")]
    StarStar,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("++")]
    PlusPlus,
    #[token("<<")]
    Shl,
    #[token(">>")]
    Shr,
    #[token("&")]
    Amp,
    #[token("|")]
    Pipe,
    #[token("??")]
    Coalesce,
    #[token("==")]
    Eq,
    #[token("!=")]
    Ne,
    #[token("<")]
    Lt,
    #[token("<=")]
    Le,
    #[token(">")]
    Gt,
    #[token(">=")]
    Ge,
    #[token(".==")]
    DotEq,
    #[token(".!=")]
    DotNe,
    #[token(".<")]
    DotLt,
    #[token(".<=")]
    DotLe,
    #[token(".>")]
    DotGt,
    #[token(".>=")]
    DotGe,
}

fn lex_string(lex: &mut logos::Lexer<Raw>) -> bool {
    match scan_string(lex.remainder().as_bytes(), 0) {
        Some(end) => {
            lex.bump(end);
            true
        }
        None => false,
    }
}

/// Scans a `"..."` string whose contents start at `b[i]`. Returns the index
/// just past the closing quote, or `None` if the string is unterminated.
pub fn scan_string(b: &[u8], mut i: usize) -> Option<usize> {
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'\n' => return None,
            b'"' => return Some(i + 1),
            b'{' | b'}' if b.get(i + 1) == Some(&b[i]) => i += 2,
            b'{' => i = scan_interpolation(b, i + 1)?,
            _ => i += 1,
        }
    }
    None
}

/// Scans an interpolation whose contents start at `b[i]` (just after the
/// `{`). Nested braces, strings and char literals are skipped. Returns the
/// index just past the closing `}`, or `None` if it isn't closed on this
/// line.
pub fn scan_interpolation(b: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 1;
    while i < b.len() {
        match b[i] {
            b'"' => i = scan_string(b, i + 1)?,
            b'\'' => i = skip_char_literal(b, i),
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'\n' => return None,
            _ => i += 1,
        }
    }
    None
}

/// Skips a char literal such as `'}'` or `'\''` starting at `b[i]`.
fn skip_char_literal(b: &[u8], i: usize) -> usize {
    let j = i + 1;
    // Length of the char (or escape) from its first byte.
    let len = match b.get(j) {
        Some(b'\\') => 2,
        Some(&x) if x < 0x80 => 1,
        Some(&x) if x >= 0xF0 => 4,
        Some(&x) if x >= 0xE0 => 3,
        Some(_) => 2,
        None => return i + 1,
    };
    // Not a well-formed char literal: step over the quote only.
    if b.get(j + len) == Some(&b'\'') {
        j + len + 1
    } else {
        i + 1
    }
}

fn lex_triple(lex: &mut logos::Lexer<Raw>) -> bool {
    match lex.remainder().find("\"\"\"") {
        Some(end) => {
            lex.bump(end + 3);
            true
        }
        None => false,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(Rc<str>),
    Int(crate::int::Int),
    Char(char),
    /// Raw contents of a `"..."` string, without the quotes and with escapes
    /// and braces unprocessed.
    Str(Rc<str>),
    /// Contents of a `"""..."""` string, already processed.
    TripleStr(Rc<str>),
    // Keywords
    Day,
    Example,
    Answers,
    Test,
    TestError,
    Where,
    Match,
    When,
    If,
    Then,
    Else,
    For,
    Let,
    In,
    And,
    Or,
    Not,
    Xor,
    True,
    False,
    None,
    Inf,
    Underscore,
    // Punctuation
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Assign,
    Arrow,
    Dot,
    DotDot,
    DotDotLt,
    At,
    Plus,
    Minus,
    Tilde,
    Star,
    StarStar,
    Slash,
    Percent,
    PlusPlus,
    Shl,
    Shr,
    Amp,
    Pipe,
    Coalesce,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    DotEq,
    DotNe,
    DotLt,
    DotLe,
    DotGt,
    DotGe,
    // Inserted by the layout pass (SPEC 3.1)
    BlockOpen,
    BlockSep,
    BlockClose,
    Eof,
}

impl Tok {
    pub fn describe(&self) -> String {
        match self {
            Tok::Ident(n) => format!("identifier `{n}`"),
            Tok::Int(i) => format!("integer `{i}`"),
            Tok::Char(_) => "a character literal".into(),
            Tok::Str(_) | Tok::TripleStr(_) => "a string".into(),
            Tok::BlockOpen => "an indented block".into(),
            Tok::BlockSep => "a new line".into(),
            Tok::BlockClose => "the end of the block".into(),
            Tok::Eof => "the end of the file".into(),
            other => format!("`{}`", other.text()),
        }
    }

    /// Source text of keywords and punctuation.
    pub fn text(&self) -> &'static str {
        match self {
            Tok::Day => "day",
            Tok::Example => "example",
            Tok::Answers => "answers",
            Tok::Test => "test",
            Tok::TestError => "test_error",
            Tok::Where => "where",
            Tok::Match => "match",
            Tok::When => "when",
            Tok::If => "if",
            Tok::Then => "then",
            Tok::Else => "else",
            Tok::For => "for",
            Tok::Let => "let",
            Tok::In => "in",
            Tok::And => "and",
            Tok::Or => "or",
            Tok::Not => "not",
            Tok::Xor => "xor",
            Tok::True => "true",
            Tok::False => "false",
            Tok::None => "none",
            Tok::Inf => "inf",
            Tok::Underscore => "_",
            Tok::LParen => "(",
            Tok::RParen => ")",
            Tok::LBracket => "[",
            Tok::RBracket => "]",
            Tok::LBrace => "{",
            Tok::RBrace => "}",
            Tok::Comma => ",",
            Tok::Colon => ":",
            Tok::Assign => "=",
            Tok::Arrow => "=>",
            Tok::Dot => ".",
            Tok::DotDot => "..",
            Tok::DotDotLt => "..<",
            Tok::At => "@",
            Tok::Plus => "+",
            Tok::Minus => "-",
            Tok::Tilde => "~",
            Tok::Star => "*",
            Tok::StarStar => "**",
            Tok::Slash => "/",
            Tok::Percent => "%",
            Tok::PlusPlus => "++",
            Tok::Shl => "<<",
            Tok::Shr => ">>",
            Tok::Amp => "&",
            Tok::Pipe => "|",
            Tok::Coalesce => "??",
            Tok::Eq => "==",
            Tok::Ne => "!=",
            Tok::Lt => "<",
            Tok::Le => "<=",
            Tok::Gt => ">",
            Tok::Ge => ">=",
            Tok::DotEq => ".==",
            Tok::DotNe => ".!=",
            Tok::DotLt => ".<",
            Tok::DotLe => ".<=",
            Tok::DotGt => ".>",
            Tok::DotGe => ".>=",
            _ => "",
        }
    }

    /// The identifier spelling of a keyword, so that `g.where(...)` works.
    pub fn keyword_name(&self) -> Option<&'static str> {
        match self {
            Tok::Day
            | Tok::Example
            | Tok::Answers
            | Tok::Test
            | Tok::TestError
            | Tok::Where
            | Tok::Match
            | Tok::When
            | Tok::If
            | Tok::Then
            | Tok::Else
            | Tok::For
            | Tok::Let
            | Tok::In
            | Tok::And
            | Tok::Or
            | Tok::Not
            | Tok::Xor
            | Tok::True
            | Tok::False
            | Tok::None
            | Tok::Inf => Some(self.text()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

fn keyword(s: &str) -> Option<Tok> {
    Some(match s {
        "day" => Tok::Day,
        "example" => Tok::Example,
        "answers" => Tok::Answers,
        "test" => Tok::Test,
        "test_error" => Tok::TestError,
        "where" => Tok::Where,
        "match" => Tok::Match,
        "when" => Tok::When,
        "if" => Tok::If,
        "then" => Tok::Then,
        "else" => Tok::Else,
        "for" => Tok::For,
        "let" => Tok::Let,
        "in" => Tok::In,
        "and" => Tok::And,
        "or" => Tok::Or,
        "not" => Tok::Not,
        "xor" => Tok::Xor,
        "true" => Tok::True,
        "false" => Tok::False,
        "none" => Tok::None,
        "inf" => Tok::Inf,
        "_" => Tok::Underscore,
        _ => return None,
    })
}

/// The escapes of strings and characters (SPEC 3). A character literal
/// can also write `\'`.
pub fn unescape_char(c: char) -> Option<char> {
    Some(match c {
        'n' => '\n',
        't' => '\t',
        '\\' => '\\',
        '"' => '"',
        _ => return None,
    })
}

/// `day`, `example`, `answers`, `test` and `test_error` are keywords only
/// where they start a top-level item (SPEC 3): at the start of a line in
/// column 1, followed by what that item needs. Anywhere else they are
/// turned into ordinary names. `src` is the text the tokens' spans index.
pub fn item_keywords(src: &str, mut toks: Vec<Token>) -> Vec<Token> {
    let index = LineIndex::new(src);
    let line = |t: &Token| index.line(t.span.start);
    for i in 0..toks.len() {
        let word = match toks[i].tok {
            Tok::Day => "day",
            Tok::Example => "example",
            Tok::Answers => "answers",
            Tok::Test => "test",
            Tok::TestError => "test_error",
            _ => continue,
        };
        let starts_line = i == 0 || line(&toks[i - 1]) < line(&toks[i]);
        let column_one = index.line_col(src, toks[i].span.start).1 == 1;
        let next = toks.get(i + 1);
        let fits = match (&toks[i].tok, next.map(|t| &t.tok)) {
            (Tok::Day, Some(Tok::Int(_))) => true,
            (Tok::Example | Tok::Test | Tok::TestError, Some(Tok::Str(_) | Tok::TripleStr(_))) => {
                true
            }
            (Tok::Answers, None) => true,
            (Tok::Answers, Some(_)) => next.is_some_and(|n| line(n) > line(&toks[i])),
            _ => false,
        };
        if !(starts_line && column_one && fits) {
            toks[i].tok = Tok::Ident(word.into());
        }
    }
    toks
}

/// Turns the item keywords into names, for code that can't start an item,
/// such as an expression interpolated into a string.
pub fn no_item_keywords(mut toks: Vec<Token>) -> Vec<Token> {
    for t in &mut toks {
        let word = match t.tok {
            Tok::Day => "day",
            Tok::Example => "example",
            Tok::Answers => "answers",
            Tok::Test => "test",
            Tok::TestError => "test_error",
            _ => continue,
        };
        t.tok = Tok::Ident(word.into());
    }
    toks
}

/// Lexes `src`, whose first byte is at offset `base` in the file.
pub fn lex(src: &str, base: usize) -> Result<Vec<Token>, Error> {
    let mut out = Vec::new();
    let mut lexer = Raw::lexer(src);
    while let Some(raw) = lexer.next() {
        let r = lexer.span();
        let span = Span::new(base + r.start, base + r.end);
        let text = lexer.slice();
        let raw = match raw {
            Ok(raw) => raw,
            Err(()) => {
                let msg = if text.starts_with('"') {
                    "unterminated string (an unclosed `{` also causes this; write `{{` for a literal brace)".to_string()
                } else if text.starts_with('\'') {
                    "invalid character literal".to_string()
                } else {
                    format!("unexpected character `{text}`")
                };
                return Err(Error::new(msg, span));
            }
        };
        let tok = match raw {
            Raw::Ident => keyword(text).unwrap_or_else(|| Tok::Ident(text.into())),
            Raw::Int => Tok::Int(
                crate::int::Int::parse(text).ok_or_else(|| Error::new("invalid integer", span))?,
            ),
            Raw::Char => {
                let inner = &text[1..text.len() - 1];
                let mut cs = inner.chars();
                let c = match cs.next() {
                    Some('\\') => cs
                        .next()
                        .and_then(|c| if c == '\'' { Some(c) } else { unescape_char(c) })
                        .ok_or_else(|| Error::new("unknown escape in character literal", span))?,
                    Some(c) => c,
                    None => return Err(Error::new("empty character literal", span)),
                };
                Tok::Char(c)
            }
            Raw::Str => Tok::Str(text[1..text.len() - 1].into()),
            Raw::TripleStr => {
                let inner = &text[3..text.len() - 3];
                let inner = inner
                    .strip_prefix("\r\n")
                    .or_else(|| inner.strip_prefix('\n'))
                    .unwrap_or(inner);
                Tok::TripleStr(inner.into())
            }
            Raw::LParen => Tok::LParen,
            Raw::RParen => Tok::RParen,
            Raw::LBracket => Tok::LBracket,
            Raw::RBracket => Tok::RBracket,
            Raw::LBrace => Tok::LBrace,
            Raw::RBrace => Tok::RBrace,
            Raw::Comma => Tok::Comma,
            Raw::Colon => Tok::Colon,
            Raw::Assign => Tok::Assign,
            Raw::Arrow => Tok::Arrow,
            Raw::Dot => Tok::Dot,
            Raw::DotDot => Tok::DotDot,
            Raw::DotDotLt => Tok::DotDotLt,
            Raw::At => Tok::At,
            Raw::Plus => Tok::Plus,
            Raw::Minus => Tok::Minus,
            Raw::Tilde => Tok::Tilde,
            Raw::Star => Tok::Star,
            Raw::StarStar => Tok::StarStar,
            Raw::Slash => Tok::Slash,
            Raw::Percent => Tok::Percent,
            Raw::PlusPlus => Tok::PlusPlus,
            Raw::Shl => Tok::Shl,
            Raw::Shr => Tok::Shr,
            Raw::Amp => Tok::Amp,
            Raw::Pipe => Tok::Pipe,
            Raw::Coalesce => Tok::Coalesce,
            Raw::Eq => Tok::Eq,
            Raw::Ne => Tok::Ne,
            Raw::Lt => Tok::Lt,
            Raw::Le => Tok::Le,
            Raw::Gt => Tok::Gt,
            Raw::Ge => Tok::Ge,
            Raw::DotEq => Tok::DotEq,
            Raw::DotNe => Tok::DotNe,
            Raw::DotLt => Tok::DotLt,
            Raw::DotLe => Tok::DotLe,
            Raw::DotGt => Tok::DotGt,
            Raw::DotGe => Tok::DotGe,
        };
        // `g.where(...)`: a keyword after `.` is a name.
        let tok = match (out.last(), tok.keyword_name()) {
            (Some(Token { tok: Tok::Dot, .. }), Some(name)) => Tok::Ident(name.into()),
            _ => tok,
        };
        // The string after `example`, `test`, `test_error` or `include` at
        // the start of a line is never interpolated, so its braces are
        // plain text (SPEC 3): read it without looking for `{...}`.
        // (`include` is a name, not a keyword; a function of that name is
        // followed by `(`, not a string, so it isn't affected.)
        let item_word = match &tok {
            Tok::Example | Tok::Test | Tok::TestError => true,
            Tok::Ident(n) => &**n == "include",
            _ => false,
        };
        let item = item_word && (r.start == 0 || src.as_bytes()[r.start - 1] == b'\n');
        out.push(Token { tok, span });
        if item && let Some(tok) = plain_string_after(&mut lexer, base)? {
            out.push(tok);
        }
    }
    Ok(out)
}

/// The `"..."` string next in `lexer`, after spaces, read with its braces
/// as plain text, or `None` if a `"..."` string isn't next.
fn plain_string_after(lexer: &mut logos::Lexer<Raw>, base: usize) -> Result<Option<Token>, Error> {
    let rest = lexer.remainder().as_bytes();
    let gap = rest
        .iter()
        .take_while(|&&c| c == b' ' || c == b'\t')
        .count();
    if rest.get(gap) != Some(&b'"') || rest.get(gap + 1..gap + 3) == Some(b"\"\"") {
        return Ok(None);
    }
    let start = base + lexer.span().end + gap;
    let mut i = gap + 1;
    loop {
        match rest.get(i) {
            Some(b'\\') => i += 2,
            Some(b'"') => break,
            Some(b'\n') | None => {
                return Err(Error::new(
                    "unterminated string",
                    Span::new(start, start + i - gap),
                ));
            }
            Some(_) => i += 1,
        }
    }
    let text = &lexer.remainder()[gap + 1..i];
    let tok = Tok::Str(text.into());
    let span = Span::new(start, start + i + 1 - gap);
    lexer.bump(i + 1);
    Ok(Some(Token { tok, span }))
}
