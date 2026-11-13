//! Hand-written Pratt parser (SPEC 5.1 for precedence). Works on the token
//! stream after the layout pass.

use crate::ast::*;
use crate::error::{Error, Result, Span};
use crate::int::Int;
use crate::layout::layout;
use crate::lexer::{
    Tok, Token, item_keywords, lex, no_item_keywords, scan_interpolation, unescape_char,
};
use crate::value::{V, Value};
use std::rc::Rc;

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// Placeholders parsed so far that haven't been wrapped into a lambda
    /// yet: 0 for `_`, or n for `_n` (SPEC 5.4), with where each was.
    placeholders: Vec<(usize, Span)>,
    /// Token index where the match guard being parsed starts, if any. At
    /// the guard's own bracket depth, `=>` ends the guard rather than
    /// starting a lambda, so `n when n < lim => ...` works.
    guard_start: Option<usize>,
    /// A `where` block written as the last entry of a `match`, at the
    /// arms' indentation, and the span of that `match`. It belongs to the
    /// definition whose body is the `match` (SPEC 7.2).
    trailing_where: Option<(Wheres, Span)>,
}

// Binding powers, loosest first.
const BP_COALESCE: u8 = 1;
const BP_OR: u8 = 2;
const BP_AND: u8 = 3;
const BP_CMP: u8 = 4;
const BP_DOTCMP: u8 = 5;
const BP_RANGE: u8 = 6;
const BP_BITOR: u8 = 7;
const BP_XOR: u8 = 8;
const BP_BITAND: u8 = 9;
const BP_SHIFT: u8 = 10;
const BP_ADD: u8 = 11;
const BP_MUL: u8 = 12;
const BP_UNARY: u8 = 13;
const BP_POW: u8 = 14;

type BuildFn = fn(Box<Expr>, Box<Expr>) -> ExprKind;

pub fn parse_program(src: &str) -> Result<Vec<Item>> {
    parse_file(src, 0)
}

/// Parses the file whose text is `all[base..]`. Spans are offsets into
/// `all`, which may hold other files before this one (SPEC 2.8). `base`
/// must be at the start of a line.
pub fn parse_file(all: &str, base: usize) -> Result<Vec<Item>> {
    let text = all.get(base..).unwrap_or("");
    let toks = layout(all, item_keywords(all, lex(text, base)?));
    let mut p = Parser {
        toks,
        pos: 0,
        placeholders: Vec::new(),
        guard_start: None,
        trailing_where: None,
    };
    p.program()
}

impl Parser {
    // --- Token helpers -------------------------------------------------------

    fn peek(&self) -> &Tok {
        &self.toks[self.pos.min(self.toks.len() - 1)].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    fn span(&self) -> Span {
        self.toks[self.pos.min(self.toks.len() - 1)].span
    }

    fn prev_span(&self) -> Span {
        self.toks[self.pos.saturating_sub(1)].span
    }

    fn advance(&mut self) -> Token {
        let t = self.toks[self.pos.min(self.toks.len() - 1)].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == t {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &Tok, context: &str) -> Result<Span> {
        if self.peek() == t {
            Ok(self.advance().span)
        } else {
            Err(self.unexpected(&format!("{} {context}", t.describe())))
        }
    }

    fn unexpected(&self, expected: &str) -> Error {
        Error::new(
            format!("expected {expected}, found {}", self.peek().describe()),
            self.span(),
        )
    }

    fn ident(&mut self, what: &str) -> Result<(Name, Span)> {
        match self.peek().clone() {
            Tok::Ident(n) => Ok((n, self.advance().span)),
            _ => Err(self.unexpected(what)),
        }
    }

    // --- Program structure ---------------------------------------------------

    fn program(&mut self) -> Result<Vec<Item>> {
        let mut items = Vec::new();
        let mut annotation = None;
        loop {
            if *self.peek() == Tok::Eof {
                break;
            }
            if *self.peek() == Tok::At {
                annotation = Some(self.annotation()?);
            } else {
                items.push(self.item(annotation.take())?);
            }
            match self.peek() {
                Tok::BlockSep => {
                    self.advance();
                }
                Tok::Eof => break,
                _ => return Err(self.unexpected("a new line after this item")),
            }
        }
        if annotation.is_some() {
            return Err(Error::new(
                "annotation must be followed by a function",
                self.span(),
            ));
        }
        Ok(items)
    }

    fn annotation(&mut self) -> Result<Annotation> {
        self.expect(&Tok::At, "")?;
        let (name, span) = self.ident("`memo` or `nomemo` after `@`")?;
        match &*name {
            "memo" => Ok(Annotation::Memo),
            "nomemo" => Ok(Annotation::NoMemo),
            _ => Err(Error::new(format!("unknown annotation `@{name}`"), span)),
        }
    }

    fn item(&mut self, annotation: Option<Annotation>) -> Result<Item> {
        let start = self.span();
        if annotation.is_some() && !matches!(self.peek(), Tok::Ident(_)) {
            return Err(Error::new(
                "annotation must be followed by a function",
                start,
            ));
        }
        match self.peek().clone() {
            Tok::Ident(n) if &*n == "include" && annotation.is_none() => {
                match self.peek_at(1).clone() {
                    Tok::Str(raw) => {
                        self.advance();
                        let span = self.advance().span;
                        Ok(Item::Include {
                            path: plain_string(&raw, span)?,
                            span: start.to(span),
                        })
                    }
                    _ => Ok(Item::Def(self.def(annotation)?)),
                }
            }
            Tok::Day => {
                self.advance();
                let year = self.small_int("a year after `day`")?;
                self.expect(&Tok::Dot, "between year and day")?;
                let day = self.small_int("a day number")?;
                Ok(Item::Day {
                    year,
                    day,
                    span: start.to(self.prev_span()),
                })
            }
            Tok::Example => {
                self.advance();
                let text = match self.advance().tok {
                    Tok::TripleStr(s) => s,
                    Tok::Str(raw) => plain_string(&raw, self.prev_span())?.into(),
                    _ => {
                        return Err(Error::new(
                            "expected a string after `example`",
                            self.prev_span(),
                        ));
                    }
                };
                let mut withs = Vec::new();
                let mut expects = Vec::new();
                self.block(|p| {
                    if matches!(p.peek(), Tok::Ident(n) if &**n == "with")
                        && matches!(p.peek_at(1), Tok::Ident(_))
                    {
                        p.advance();
                        let (name, span) = p.ident("a name")?;
                        p.expect(&Tok::Assign, "after the name")?;
                        let body = p.rhs()?;
                        let pat = Pattern {
                            kind: PatKind::Bind(Binding {
                                name: name.clone(),
                                slot: 0,
                                span,
                            }),
                            span,
                        };
                        let def = ConstDef {
                            pat,
                            body,
                            wheres: Wheres::default(),
                            frame_size: 0,
                            span: span.to(p.prev_span()),
                        };
                        withs.push((name, Rc::new(def), span));
                    } else {
                        expects.push(p.expect_entry()?);
                    }
                    Ok(())
                })?;
                Ok(Item::Example(Example {
                    text,
                    withs,
                    expects,
                    span: start,
                }))
            }
            Tok::Answers => {
                self.advance();
                let mut expects = Vec::new();
                self.block(|p| {
                    expects.push(p.expect_entry()?);
                    Ok(())
                })?;
                Ok(Item::Answers {
                    expects,
                    span: start,
                })
            }
            Tok::Test | Tok::TestError => {
                let expect_error = *self.peek() == Tok::TestError;
                self.advance();
                let name: Name = match self.advance().tok {
                    Tok::Str(raw) => plain_string(&raw, self.prev_span())?.into(),
                    _ => return Err(Error::new("expected a test name string", self.prev_span())),
                };
                self.expect(&Tok::Assign, "after the test name")?;
                let expr = self.rhs()?;
                let span = start.to(self.prev_span());
                Ok(Item::Test {
                    check: Check {
                        name,
                        expr,
                        frame_size: 0,
                        span,
                    },
                    expect_error,
                })
            }
            _ => Ok(Item::Def(self.def(annotation)?)),
        }
    }

    fn small_int(&mut self, what: &str) -> Result<u32> {
        match self.peek().clone() {
            Tok::Int(Int::Small(i)) if (0..=u32::MAX as i64).contains(&i) => {
                self.advance();
                Ok(i as u32)
            }
            _ => Err(self.unexpected(what)),
        }
    }

    /// `name = expr` inside `example` or `answers`.
    fn expect_entry(&mut self) -> Result<Check> {
        let (name, span) = self.ident("`part1 = ...` or similar")?;
        self.expect(&Tok::Assign, "after the name")?;
        let expr = self.rhs()?;
        Ok(Check {
            name,
            expr,
            frame_size: 0,
            span: span.to(self.prev_span()),
        })
    }

    /// Parses an indented block: `BlockOpen entry (BlockSep entry)* BlockClose`.
    fn block(&mut self, mut entry: impl FnMut(&mut Parser) -> Result<()>) -> Result<()> {
        if *self.peek() != Tok::BlockOpen {
            return Ok(());
        }
        self.advance();
        if self.eat(&Tok::BlockClose) {
            return Ok(());
        }
        loop {
            entry(self)?;
            match self.peek() {
                Tok::BlockSep => {
                    self.advance();
                }
                Tok::BlockClose => {
                    self.advance();
                    return Ok(());
                }
                _ => return Err(self.unexpected("a new line or the end of the block")),
            }
        }
    }

    fn def(&mut self, annotation: Option<Annotation>) -> Result<RawDef> {
        let start = self.span();
        match (self.peek().clone(), self.peek_at(1).clone()) {
            (Tok::Ident(name), Tok::LParen) => {
                self.advance();
                self.advance();
                let params = self.pattern_list(&Tok::RParen)?;
                let guard = if self.eat(&Tok::When) {
                    Some(self.expr()?)
                } else {
                    None
                };
                self.expect(&Tok::Assign, "in function definition")?;
                let body = self.rhs()?;
                let wheres = self.definition_wheres(&body)?;
                let clause = Clause {
                    params,
                    guard,
                    body,
                    wheres,
                    frame_size: 0,
                    span: start.to(self.prev_span()),
                };
                Ok(RawDef::Clause {
                    name,
                    clause,
                    annotation,
                })
            }
            (Tok::Ident(_), Tok::Assign) | (Tok::LParen, _) | (Tok::LBracket, _) => {
                if annotation.is_some() {
                    return Err(Error::new("annotations apply only to functions", start));
                }
                let pat = self.pattern()?;
                self.expect(&Tok::Assign, "in definition")?;
                let body = self.rhs()?;
                let wheres = self.definition_wheres(&body)?;
                Ok(RawDef::Const(ConstDef {
                    pat,
                    body,
                    wheres,
                    frame_size: 0,
                    span: start.to(self.prev_span()),
                }))
            }
            _ => Err(self.unexpected("a definition")),
        }
    }

    /// The `where` block of a definition with this body: after the body, or
    /// as the last entry of a `match` that is the whole body.
    fn definition_wheres(&mut self, body: &Expr) -> Result<Wheres> {
        match self.trailing_where.take() {
            None => self.wheres(),
            Some((wheres, match_span)) => {
                if !matches!(body.kind, ExprKind::Match(..)) || body.span != match_span {
                    return Err(Error::new(
                        "a `where` among `match` arms belongs to a definition whose whole body is that `match`",
                        match_span,
                    ));
                }
                if *self.peek() == Tok::Where {
                    return Err(self.unexpected("one `where` block, not two"));
                }
                Ok(wheres)
            }
        }
    }

    fn wheres(&mut self) -> Result<Wheres> {
        if !self.eat(&Tok::Where) {
            return Ok(Wheres::default());
        }
        if *self.peek() != Tok::BlockOpen {
            return Err(self.unexpected("indented definitions after `where`"));
        }
        let mut raw = Vec::new();
        let mut annotation = None;
        self.block(|p| {
            if *p.peek() == Tok::At {
                annotation = Some(p.annotation()?);
            } else {
                raw.push(Some(p.def(annotation.take())?));
            }
            Ok(())
        })?;
        let (fns, consts) = group_defs(raw)?;
        Ok(Wheres {
            consts: consts.into_iter().map(Rc::new).collect(),
            fns: fns.into_iter().map(|f| (Rc::new(f), 0)).collect(),
        })
    }

    /// The right-hand side of a definition. A `_` anywhere in it (outside a
    /// call argument) makes the whole expression a lambda (SPEC 5.4).
    fn rhs(&mut self) -> Result<Expr> {
        self.wrapping_placeholders(|p| p.expr())
    }

    fn wrapping_placeholders(
        &mut self,
        f: impl FnOnce(&mut Parser) -> Result<Expr>,
    ) -> Result<Expr> {
        let before = self.placeholders.len();
        let e = f(self)?;
        self.wrap_new_placeholders(before, e)
    }

    /// Makes `e` a lambda if it contains placeholders parsed since there
    /// were `before` of them: one parameter for `_`, or `_1` to `_n` for
    /// numbered ones.
    fn wrap_new_placeholders(&mut self, before: usize, e: Expr) -> Result<Expr> {
        if self.placeholders.len() <= before {
            return Ok(e);
        }
        let new = self.placeholders.split_off(before);
        let plain = new.iter().find(|(n, _)| *n == 0);
        let numbered = new.iter().find(|(n, _)| *n > 0);
        match (plain, numbered) {
            (Some(_), Some((_, span))) => Err(Error::new(
                "`_` and numbered placeholders like `_1` can't be mixed in one lambda",
                *span,
            )),
            (Some(_), None) => Ok(make_lambda(&[PLACEHOLDER.into()], e)),
            _ => {
                let most = new.iter().map(|(n, _)| *n).max().unwrap_or(1);
                let names: Vec<Name> = (1..=most).map(|i| format!("_{i}").into()).collect();
                Ok(make_lambda(&names, e))
            }
        }
    }

    // --- Patterns ------------------------------------------------------------

    /// Comma-separated patterns up to and including `close`.
    fn pattern_list(&mut self, close: &Tok) -> Result<Vec<Pattern>> {
        let mut out = Vec::new();
        while self.peek() != close {
            out.push(self.pattern()?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(close, "to close the patterns")?;
        Ok(out)
    }

    pub fn pattern(&mut self) -> Result<Pattern> {
        let start = self.span();
        let tok = self.advance().tok;
        let kind = match tok {
            Tok::Underscore => match self.type_annotation()? {
                Some(ty) => PatKind::Typed(None, ty),
                None => PatKind::Wild,
            },
            Tok::Ident(name) => {
                if numbered_placeholder(&name).is_some() {
                    return Err(Error::new(
                        format!("`{name}` is a placeholder (SPEC 5.4), not a name to bind"),
                        start,
                    ));
                }
                let bind = Binding {
                    name,
                    slot: 0,
                    span: start,
                };
                match self.type_annotation()? {
                    Some(ty) => PatKind::Typed(Some(bind), ty),
                    None => PatKind::Bind(bind),
                }
            }
            Tok::Int(i) => PatKind::Lit(Value::from_int(i)),
            Tok::Minus => match self.advance().tok {
                Tok::Int(i) => PatKind::Lit(Value::from_int(i.neg())),
                Tok::Inf => PatKind::Lit(Value::from_int(Int::Inf(false))),
                _ => {
                    return Err(Error::new(
                        "expected a number after `-` in pattern",
                        self.prev_span(),
                    ));
                }
            },
            Tok::Inf => PatKind::Lit(Value::from_int(Int::Inf(true))),
            Tok::Char(c) => PatKind::Lit(Value::char(c)),
            Tok::True => PatKind::Lit(Value::bool(true)),
            Tok::False => PatKind::Lit(Value::bool(false)),
            Tok::None => PatKind::Lit(Value::NONE),
            // A triple-quoted pattern: verbatim, so no escapes. Its text
            // ends just before the closing quotes, which locates captures
            // even when a leading newline was dropped (SPEC 6.1).
            Tok::TripleStr(s) => {
                let base = start.end - 3 - s.len();
                string_pattern(&s, start, base, true)?
            }
            Tok::Str(raw) => string_pattern(&raw, start, start.start + 1, false)?,
            Tok::LParen => {
                let mut items = Vec::new();
                let mut trailing_comma = false;
                while *self.peek() != Tok::RParen {
                    items.push(self.pattern()?);
                    trailing_comma = self.eat(&Tok::Comma);
                    if !trailing_comma {
                        break;
                    }
                }
                self.expect(&Tok::RParen, "to close the tuple pattern")?;
                if items.len() == 1 && !trailing_comma {
                    return Ok(items.pop().unwrap_or(Pattern {
                        kind: PatKind::Wild,
                        span: start,
                    }));
                }
                PatKind::Tuple(items)
            }
            Tok::LBracket => {
                let mut before = Vec::new();
                let mut after = Vec::new();
                let mut rest = None;
                while *self.peek() != Tok::RBracket {
                    if self.eat(&Tok::DotDot) {
                        if rest.is_some() {
                            return Err(Error::new(
                                "at most one `..` is allowed in a list pattern",
                                self.prev_span(),
                            ));
                        }
                        rest = Some(match self.peek().clone() {
                            Tok::Ident(name) => {
                                let span = self.advance().span;
                                Some(Binding {
                                    name,
                                    slot: 0,
                                    span,
                                })
                            }
                            _ => None,
                        });
                    } else if rest.is_some() {
                        after.push(self.pattern()?);
                    } else {
                        before.push(self.pattern()?);
                    }
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(&Tok::RBracket, "to close the list pattern")?;
                PatKind::List {
                    before,
                    rest,
                    after,
                }
            }
            other => {
                return Err(Error::new(
                    format!("expected a pattern, found {}", other.describe()),
                    start,
                ));
            }
        };
        Ok(Pattern {
            kind,
            span: start.to(self.prev_span()),
        })
    }

    fn type_annotation(&mut self) -> Result<Option<TypeName>> {
        if !self.eat(&Tok::Colon) {
            return Ok(None);
        }
        let span = self.span();
        let name: Name = match self.advance().tok {
            Tok::Ident(n) => n,
            Tok::None => "none".into(),
            other => {
                return Err(Error::new(
                    format!("expected a type name, found {}", other.describe()),
                    span,
                ));
            }
        };
        TypeName::parse(&name)
            .map(Some)
            .ok_or_else(|| Error::new(format!("unknown type `{name}`"), span))
    }

    // --- Expressions ---------------------------------------------------------

    pub fn expr(&mut self) -> Result<Expr> {
        self.expr_bp(0)
    }

    fn expr_bp(&mut self, min_bp: u8) -> Result<Expr> {
        let mut lhs = self.prefix()?;
        loop {
            let tok = self.peek().clone();
            if let Some(op) = cmp_op(&tok, self.peek_at(1)) {
                if BP_CMP < min_bp {
                    break;
                }
                let mut rest = Vec::new();
                let mut op = op;
                loop {
                    self.advance();
                    if op == CmpOp::NotIn {
                        self.advance();
                    }
                    rest.push((op, self.expr_bp(BP_CMP + 1)?));
                    match cmp_op(self.peek(), self.peek_at(1)) {
                        Some(o) => op = o,
                        None => break,
                    }
                }
                let span = lhs.span.to(self.prev_span());
                lhs = Expr::new(ExprKind::Compare(Box::new(lhs), rest), span);
                continue;
            }
            if matches!(tok, Tok::DotDot | Tok::DotDotLt) {
                if BP_RANGE < min_bp {
                    break;
                }
                self.advance();
                let hi = if self.starts_expr() {
                    Some(Box::new(self.expr_bp(BP_RANGE + 1)?))
                } else {
                    None
                };
                if tok == Tok::DotDotLt && hi.is_none() {
                    return Err(Error::new("`..<` needs an end", self.prev_span()));
                }
                let span = lhs.span.to(self.prev_span());
                lhs = Expr::new(
                    ExprKind::Range {
                        lo: Some(Box::new(lhs)),
                        hi,
                        inclusive: tok == Tok::DotDot,
                    },
                    span,
                );
                if matches!(self.peek(), Tok::DotDot | Tok::DotDotLt) {
                    return Err(Error::new("ranges can't be chained", self.span()));
                }
                continue;
            }
            let (lbp, rbp, build): (u8, u8, BuildFn) = match tok {
                Tok::Coalesce => (BP_COALESCE, BP_COALESCE, ExprKind::Coalesce),
                Tok::Or => (BP_OR, BP_OR + 1, ExprKind::Or),
                Tok::And => (BP_AND, BP_AND + 1, ExprKind::And),
                _ => match bin_op(&tok) {
                    Some((op, lbp, rbp)) => {
                        if lbp < min_bp {
                            break;
                        }
                        self.advance();
                        let rhs = self.expr_bp(rbp)?;
                        let span = lhs.span.to(rhs.span);
                        lhs = Expr::new(ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), span);
                        continue;
                    }
                    None => break,
                },
            };
            if lbp < min_bp {
                break;
            }
            self.advance();
            let rhs = self.expr_bp(rbp)?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr::new(build(Box::new(lhs), Box::new(rhs)), span);
        }
        Ok(lhs)
    }

    /// Whether the next token can begin an expression (used for open ranges).
    fn starts_expr(&self) -> bool {
        matches!(
            self.peek(),
            Tok::Ident(_)
                | Tok::Int(_)
                | Tok::Char(_)
                | Tok::Str(_)
                | Tok::TripleStr(_)
                | Tok::True
                | Tok::False
                | Tok::None
                | Tok::Inf
                | Tok::Underscore
                | Tok::LParen
                | Tok::LBracket
                | Tok::LBrace
                | Tok::Minus
                | Tok::Tilde
                | Tok::Not
                | Tok::If
                | Tok::Match
                | Tok::Dot
        )
    }

    fn prefix(&mut self) -> Result<Expr> {
        let start = self.span();
        match self.peek().clone() {
            Tok::Minus => {
                self.advance();
                let operand = self.expr_bp(BP_UNARY + 1)?;
                let span = start.to(operand.span);
                // Fold negative literals so that `-7` is a constant.
                if let ExprKind::Lit(lit) = &operand.kind
                    && let V::Int(i) = lit.view()
                {
                    return Ok(Expr::new(ExprKind::Lit(Value::from_int(i.neg())), span));
                }
                Ok(Expr::new(
                    ExprKind::Unary(UnOp::Neg, Box::new(operand)),
                    span,
                ))
            }
            Tok::Not | Tok::Tilde => {
                let op = if self.advance().tok == Tok::Not {
                    UnOp::Not
                } else {
                    UnOp::BitNot
                };
                let operand = self.expr_bp(BP_UNARY + 1)?;
                let span = start.to(operand.span);
                Ok(Expr::new(ExprKind::Unary(op, Box::new(operand)), span))
            }
            Tok::DotDot | Tok::DotDotLt => {
                // `..<b` / `..b` / `..` in slices.
                let inclusive = self.advance().tok == Tok::DotDot;
                let hi = if self.starts_expr() {
                    Some(Box::new(self.expr_bp(BP_RANGE + 1)?))
                } else {
                    None
                };
                Ok(Expr::new(
                    ExprKind::Range {
                        lo: None,
                        hi,
                        inclusive,
                    },
                    start.to(self.prev_span()),
                ))
            }
            Tok::If => {
                self.advance();
                let c = self.expr()?;
                self.expect(&Tok::Then, "after the `if` condition")?;
                let a = self.expr()?;
                self.expect(&Tok::Else, "after the `then` branch")?;
                let b = self.expr()?;
                let span = start.to(b.span);
                Ok(Expr::new(
                    ExprKind::If(Box::new(c), Box::new(a), Box::new(b)),
                    span,
                ))
            }
            Tok::Match => {
                self.advance();
                let scrutinee = self.expr()?;
                if *self.peek() != Tok::BlockOpen {
                    return Err(self.unexpected("indented `pattern => expr` arms after `match`"));
                }
                let mut arms = Vec::new();
                let mut wheres = None;
                self.block(|p| {
                    // A `where` at the arms' indentation ends the arms
                    // and belongs to the enclosing definition.
                    if *p.peek() == Tok::Where {
                        if wheres.is_some() {
                            return Err(p.unexpected("one `where` block, not two"));
                        }
                        wheres = Some(p.wheres()?);
                        return Ok(());
                    }
                    if wheres.is_some() {
                        return Err(p.unexpected("the end of the `match` after its `where` block"));
                    }
                    let pat = p.pattern()?;
                    let guard = if p.eat(&Tok::When) {
                        let saved = p.guard_start.replace(p.pos);
                        let guard = p.expr();
                        p.guard_start = saved;
                        Some(guard?)
                    } else {
                        None
                    };
                    p.expect(&Tok::Arrow, "after the match pattern")?;
                    let body = p.expr()?;
                    // A `where` indented under one arm (SPEC 7.2).
                    if *p.peek() == Tok::Where {
                        return Err(Error::new(
                            "a `where` block can't belong to one `match` arm: put it after \
                             the last arm, lined up with the arms, where it serves every \
                             arm, or move this arm's work into a function",
                            p.span(),
                        ));
                    }
                    arms.push(Arm { pat, guard, body });
                    Ok(())
                })?;
                let span = start.to(self.prev_span());
                if let Some(w) = wheres {
                    if self.trailing_where.is_some() {
                        return Err(Error::new("only one `where` block per definition", span));
                    }
                    self.trailing_where = Some((w, span));
                }
                Ok(Expr::new(ExprKind::Match(Box::new(scrutinee), arms), span))
            }
            Tok::Dot => {
                // Leading-dot lambda: `.a.b(1)[0]`, or `.[i]`.
                match self.peek_at(1) {
                    Tok::Ident(_) | Tok::Int(_) => {}
                    Tok::LBracket => {
                        self.advance();
                    }
                    _ => return Err(self.unexpected("an expression")),
                }
                let param = Expr::new(ExprKind::Name(DOT_PARAM.into()), start);
                let body = self.postfix(param)?;
                Ok(make_lambda(&[DOT_PARAM.into()], body))
            }
            _ => {
                if self.lambda_ahead() {
                    return self.lambda();
                }
                let e = self.primary()?;
                self.postfix(e)
            }
        }
    }

    /// Is the next thing a lambda's parameter list followed by `=>`?
    fn lambda_ahead(&self) -> bool {
        if self.at_guard_level() {
            return false;
        }
        match self.peek() {
            Tok::Ident(_) | Tok::Underscore | Tok::Str(_) | Tok::TripleStr(_) => {
                *self.peek_at(1) == Tok::Arrow
            }
            Tok::LParen | Tok::LBracket => {
                let mut depth = 0usize;
                let mut i = self.pos;
                while i < self.toks.len() {
                    match self.toks[i].tok {
                        Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                        Tok::RParen | Tok::RBracket | Tok::RBrace => {
                            depth -= 1;
                            if depth == 0 {
                                return self.toks.get(i + 1).is_some_and(|t| t.tok == Tok::Arrow);
                            }
                        }
                        Tok::Eof => return false,
                        _ => {}
                    }
                    i += 1;
                }
                false
            }
            _ => false,
        }
    }

    /// Are we inside a match guard, outside any brackets opened within it?
    fn at_guard_level(&self) -> bool {
        let Some(start) = self.guard_start else {
            return false;
        };
        let mut depth = 0isize;
        for t in &self.toks[start..self.pos] {
            match t.tok {
                Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                Tok::RParen | Tok::RBracket | Tok::RBrace => depth -= 1,
                _ => {}
            }
        }
        depth <= 0
    }

    fn lambda(&mut self) -> Result<Expr> {
        let start = self.span();
        let params = if self.eat(&Tok::LParen) {
            self.pattern_list(&Tok::RParen)?
        } else {
            vec![self.pattern()?]
        };
        self.expect(&Tok::Arrow, "after lambda parameters")?;
        let body = self.expr()?;
        let span = start.to(body.span);
        Ok(Expr::new(
            ExprKind::Lambda(Rc::new(Lambda {
                params,
                body,
                frame_size: 0,
                span,
                closed: None,
            })),
            span,
        ))
    }

    fn primary(&mut self) -> Result<Expr> {
        let start = self.span();
        let tok = self.advance().tok;
        let lit = |v: Value| Ok(Expr::new(ExprKind::Lit(v), start));
        match tok {
            Tok::Int(i) => lit(Value::from_int(i)),
            Tok::Char(c) => lit(Value::char(c)),
            Tok::True => lit(Value::bool(true)),
            Tok::False => lit(Value::bool(false)),
            Tok::None => lit(Value::NONE),
            Tok::Inf => lit(Value::from_int(Int::Inf(true))),
            Tok::TripleStr(s) => lit(Value::from_str_rc(s)),
            Tok::Str(raw) => self.interpolated(&raw, start),
            Tok::Ident(name) => {
                if let Some(n) = numbered_placeholder(&name) {
                    self.placeholders.push((n, start));
                }
                Ok(Expr::new(ExprKind::Name(name), start))
            }
            Tok::Underscore => {
                self.placeholders.push((0, start));
                Ok(Expr::new(ExprKind::Name(PLACEHOLDER.into()), start))
            }
            Tok::LParen => {
                if self.eat(&Tok::RParen) {
                    return Ok(Expr::new(
                        ExprKind::Tuple(Vec::new()),
                        start.to(self.prev_span()),
                    ));
                }
                let first = self.expr()?;
                if *self.peek() == Tok::For {
                    let comp = self.comprehension(CompKind::Gen, first, None)?;
                    self.expect(&Tok::RParen, "to close the generator")?;
                    return Ok(Expr::new(comp, start.to(self.prev_span())));
                }
                if self.eat(&Tok::RParen) {
                    return Ok(first);
                }
                self.expect(&Tok::Comma, "or `)`")?;
                let mut items = vec![first];
                self.expr_list(&Tok::RParen, &mut items)?;
                Ok(Expr::new(
                    ExprKind::Tuple(items),
                    start.to(self.prev_span()),
                ))
            }
            Tok::LBracket => {
                if self.eat(&Tok::RBracket) {
                    return Ok(Expr::new(
                        ExprKind::List(Vec::new()),
                        start.to(self.prev_span()),
                    ));
                }
                let first = self.expr()?;
                if *self.peek() == Tok::For {
                    let comp = self.comprehension(CompKind::List, first, None)?;
                    self.expect(&Tok::RBracket, "to close the list comprehension")?;
                    return Ok(Expr::new(comp, start.to(self.prev_span())));
                }
                let mut items = vec![first];
                if self.eat(&Tok::Comma) {
                    self.expr_list(&Tok::RBracket, &mut items)?;
                } else {
                    self.expect(&Tok::RBracket, "or `,`")?;
                }
                Ok(Expr::new(ExprKind::List(items), start.to(self.prev_span())))
            }
            Tok::LBrace => {
                if self.eat(&Tok::RBrace) {
                    return Ok(Expr::new(
                        ExprKind::Map(Vec::new()),
                        start.to(self.prev_span()),
                    ));
                }
                let first = self.expr()?;
                if self.eat(&Tok::Colon) {
                    let value = self.expr()?;
                    if *self.peek() == Tok::For {
                        let comp = self.comprehension(CompKind::Map, first, Some(value))?;
                        self.expect(&Tok::RBrace, "to close the map comprehension")?;
                        return Ok(Expr::new(comp, start.to(self.prev_span())));
                    }
                    let mut entries = vec![(first, value)];
                    while self.eat(&Tok::Comma) {
                        if *self.peek() == Tok::RBrace {
                            break;
                        }
                        let k = self.expr()?;
                        self.expect(&Tok::Colon, "between key and value")?;
                        let v = self.expr()?;
                        entries.push((k, v));
                    }
                    self.expect(&Tok::RBrace, "to close the map")?;
                    return Ok(Expr::new(
                        ExprKind::Map(entries),
                        start.to(self.prev_span()),
                    ));
                }
                if *self.peek() == Tok::For {
                    let comp = self.comprehension(CompKind::Set, first, None)?;
                    self.expect(&Tok::RBrace, "to close the set comprehension")?;
                    return Ok(Expr::new(comp, start.to(self.prev_span())));
                }
                let mut items = vec![first];
                if self.eat(&Tok::Comma) {
                    self.expr_list(&Tok::RBrace, &mut items)?;
                } else {
                    self.expect(&Tok::RBrace, "or `,`")?;
                }
                Ok(Expr::new(ExprKind::Set(items), start.to(self.prev_span())))
            }
            other => Err(Error::new(
                format!("expected an expression, found {}", other.describe()),
                start,
            )),
        }
    }

    /// Remaining comma-separated expressions (after a comma) up to `close`.
    fn expr_list(&mut self, close: &Tok, items: &mut Vec<Expr>) -> Result<()> {
        while self.peek() != close {
            items.push(self.expr()?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(close, "to close the list")?;
        Ok(())
    }

    fn comprehension(
        &mut self,
        kind: CompKind,
        body: Expr,
        value: Option<Expr>,
    ) -> Result<ExprKind> {
        let mut clauses = Vec::new();
        loop {
            if self.eat(&Tok::For) {
                let pat = self.pattern()?;
                self.expect(&Tok::In, "after the loop pattern")?;
                let iter = self.expr()?;
                clauses.push(CompClause::For {
                    pat,
                    iter,
                    frame_size: 0,
                });
            } else if self.eat(&Tok::Let) {
                // `let p = e` is the same as `for p in [e]`.
                let pat = self.pattern()?;
                self.expect(&Tok::Assign, "after the `let` pattern")?;
                let value = self.expr()?;
                let span = value.span;
                clauses.push(CompClause::For {
                    pat,
                    iter: Expr::new(ExprKind::List(vec![value]), span),
                    frame_size: 0,
                });
            } else if self.eat(&Tok::If) {
                clauses.push(CompClause::If(self.expr()?));
            } else {
                break;
            }
        }
        Ok(ExprKind::Comp(Rc::new(Comp {
            kind,
            body,
            value,
            clauses,
        })))
    }

    fn postfix(&mut self, mut e: Expr) -> Result<Expr> {
        loop {
            match self.peek().clone() {
                Tok::LParen => {
                    self.advance();
                    let args = self.call_args()?;
                    let span = e.span.to(self.prev_span());
                    e = Expr::new(
                        ExprKind::Call {
                            func: Box::new(e),
                            args,
                            method: false,
                        },
                        span,
                    );
                }
                Tok::LBracket => {
                    self.advance();
                    let idx = self.expr()?;
                    self.expect(&Tok::RBracket, "to close the index")?;
                    let span = e.span.to(self.prev_span());
                    e = Expr::new(ExprKind::Index(Box::new(e), Box::new(idx)), span);
                }
                Tok::Dot => match self.peek_at(1).clone() {
                    Tok::Ident(name) => {
                        self.advance();
                        let name_span = self.advance().span;
                        let func = Box::new(Expr::new(ExprKind::Name(name), name_span));
                        let mut args = vec![Arg {
                            name: None,
                            value: e,
                        }];
                        if self.eat(&Tok::LParen) {
                            args.extend(self.call_args()?);
                        }
                        let span = args[0].value.span.to(self.prev_span());
                        e = Expr::new(
                            ExprKind::Call {
                                func,
                                args,
                                method: true,
                            },
                            span,
                        );
                    }
                    Tok::Int(Int::Small(i)) if i >= 0 => {
                        self.advance();
                        self.advance();
                        let span = e.span.to(self.prev_span());
                        e = Expr::new(ExprKind::TupleIndex(Box::new(e), i as usize), span);
                    }
                    _ => {
                        return Err(Error::new(
                            "expected a name or index after `.`",
                            self.span(),
                        ));
                    }
                },
                _ => return Ok(e),
            }
        }
    }

    /// Call arguments after `(`, up to and including `)`. Each argument
    /// containing `_` becomes a lambda, except an argument that is just `_`:
    /// that one is left for the next enclosing argument or right-hand side to
    /// wrap, so `f(_, k)` is `x => f(x, k)` (SPEC 5.4). A single generator
    /// argument needs no extra parentheses.
    fn call_args(&mut self) -> Result<Vec<Arg>> {
        let mut args = Vec::new();
        while *self.peek() != Tok::RParen {
            let name = match (self.peek().clone(), self.peek_at(1)) {
                (Tok::Ident(n), Tok::Colon) => {
                    self.advance();
                    self.advance();
                    Some(n)
                }
                _ => None,
            };
            let start = self.span();
            let before = self.placeholders.len();
            let e = self.expr()?;
            let value = if *self.peek() == Tok::For {
                let comp = self.comprehension(CompKind::Gen, e, None)?;
                let e = Expr::new(comp, start.to(self.prev_span()));
                self.wrap_new_placeholders(before, e)?
            } else if matches!(&e.kind, ExprKind::Name(n) if is_placeholder(n)) {
                e
            } else {
                self.wrap_new_placeholders(before, e)?
            };
            args.push(Arg { name, value });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen, "to close the arguments")?;
        Ok(args)
    }

    /// An interpolated string literal (SPEC 3).
    fn interpolated(&mut self, raw: &str, span: Span) -> Result<Expr> {
        let base = span.start + 1;
        let mut parts = Vec::new();
        let mut lit = String::new();
        let bytes = raw.as_bytes();
        let mut i = 0;
        while i < raw.len() {
            let c = raw[i..].chars().next().unwrap_or('\0');
            match c {
                '\\' => {
                    let next = raw[i + 1..].chars().next().unwrap_or('\0');
                    let un = unescape_char(next).ok_or_else(|| {
                        Error::new(
                            format!("unknown escape `\\{next}`"),
                            Span::new(base + i, base + i + 2),
                        )
                    })?;
                    lit.push(un);
                    i += 1 + next.len_utf8();
                }
                '{' if bytes.get(i + 1) == Some(&b'{') => {
                    lit.push('{');
                    i += 2;
                }
                '}' if bytes.get(i + 1) == Some(&b'}') => {
                    lit.push('}');
                    i += 2;
                }
                '{' => {
                    let close = scan_interpolation(bytes, i + 1).ok_or_else(|| {
                        Error::new(
                            "unclosed `{` in string (write `{{` for a literal brace)",
                            Span::new(base + i, base + i + 1),
                        )
                    })?;
                    let end = close - 1;
                    if !lit.is_empty() {
                        parts.push(InterpPart::Lit(std::mem::take(&mut lit).into()));
                    }
                    let inner = &raw[i + 1..end];
                    let mut toks = no_item_keywords(lex(inner, base + i + 1)?);
                    toks.push(Token {
                        tok: Tok::Eof,
                        span: Span::new(base + end, base + end),
                    });
                    let mut sub = Parser {
                        toks,
                        pos: 0,
                        placeholders: Vec::new(),
                        guard_start: None,
                        trailing_where: None,
                    };
                    let e = sub.expr()?;
                    if *sub.peek() != Tok::Eof {
                        return Err(sub.unexpected("`}` to end the interpolation"));
                    }
                    self.placeholders.extend(sub.placeholders);
                    parts.push(InterpPart::Expr(e));
                    i = end + 1;
                }
                '}' => {
                    return Err(Error::new(
                        "unmatched `}` in string (write `}}` for a literal brace)",
                        Span::new(base + i, base + i + 1),
                    ));
                }
                _ => {
                    lit.push(c);
                    i += c.len_utf8();
                }
            }
        }
        if parts.is_empty() {
            return Ok(Expr::new(
                ExprKind::Lit(Value::from_str_rc(lit.into())),
                span,
            ));
        }
        if !lit.is_empty() {
            parts.push(InterpPart::Lit(lit.into()));
        }
        Ok(Expr::new(ExprKind::Interp(parts), span))
    }
}

/// A string literal that isn't interpolated (test names, examples,
/// include paths): braces are plain text, so `{{` is two braces (SPEC 3).
/// Backslash escapes still apply.
fn plain_string(raw: &str, span: Span) -> Result<String> {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let n = chars.next().unwrap_or('\0');
                out.push(
                    unescape_char(n)
                        .ok_or_else(|| Error::new(format!("unknown escape `\\{n}`"), span))?,
                );
            }
            _ => out.push(c),
        }
    }
    Ok(out)
}

/// Compiles a string literal in pattern position (SPEC 6.1).
/// `base` is where `raw` starts in the source. A `verbatim` pattern (a
/// triple-quoted one) has no escapes: a backslash is itself.
fn string_pattern(raw: &str, span: Span, base: usize, verbatim: bool) -> Result<PatKind> {
    let mut segs: Vec<StrSeg> = Vec::new();
    let mut lit = String::new();
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        let c = raw[i..].chars().next().unwrap_or('\0');
        match c {
            '\\' if !verbatim => {
                let next = raw[i + 1..].chars().next().unwrap_or('\0');
                lit.push(
                    unescape_char(next)
                        .ok_or_else(|| Error::new(format!("unknown escape `\\{next}`"), span))?,
                );
                i += 1 + next.len_utf8();
            }
            '{' if bytes.get(i + 1) == Some(&b'{') => {
                lit.push('{');
                i += 2;
            }
            '}' if bytes.get(i + 1) == Some(&b'}') => {
                lit.push('}');
                i += 2;
            }
            '{' => {
                let end = raw[i..]
                    .find('}')
                    .map(|e| e + i)
                    .ok_or_else(|| Error::new("unclosed `{` in string pattern", span))?;
                let cap_span = Span::new(base + i, base + end + 1);
                let inner = raw[i + 1..end].trim();
                let (name, ty) = match inner.split_once(':') {
                    Some((n, t)) => (n.trim(), Some(t.trim())),
                    None => (inner, None),
                };
                let kind = match ty {
                    None | Some("str") => CapKind::Str,
                    Some("int") => CapKind::Int,
                    Some("char") => CapKind::Char,
                    Some("word") => CapKind::Word,
                    Some(t) => {
                        return Err(Error::new(
                            format!("unknown capture type `{t}` (expected str, int, char or word)"),
                            cap_span,
                        ));
                    }
                };
                let valid_ident = name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if !valid_ident {
                    return Err(Error::new(
                        format!("invalid capture name `{name}`"),
                        cap_span,
                    ));
                }
                if !lit.is_empty() {
                    segs.push(StrSeg::Lit(std::mem::take(&mut lit)));
                }
                if kind == CapKind::Str
                    && let Some(StrSeg::Cap {
                        kind: CapKind::Str, ..
                    }) = segs.last()
                {
                    return Err(Error::new(
                        "two untyped captures next to each other are ambiguous; give one a type",
                        cap_span,
                    ));
                }
                let bind = (name != "_").then(|| Binding {
                    name: name.into(),
                    slot: 0,
                    span: cap_span,
                });
                segs.push(StrSeg::Cap { bind, kind });
                i = end + 1;
            }
            '}' => {
                return Err(Error::new(
                    "unmatched `}` in string pattern (write `}}`)",
                    span,
                ));
            }
            _ => {
                lit.push(c);
                i += c.len_utf8();
            }
        }
    }
    if segs.is_empty() {
        return Ok(PatKind::Lit(Value::from_str_rc(lit.into())));
    }
    if !lit.is_empty() {
        segs.push(StrSeg::Lit(lit));
    }
    Ok(PatKind::Str(segs))
}

/// n for a numbered placeholder `_n` (n from 1), else none.
fn numbered_placeholder(name: &str) -> Option<usize> {
    let digits = name.strip_prefix('_')?;
    if digits.is_empty() || digits.starts_with('0') || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// `_` or `_n` (SPEC 5.4).
pub fn is_placeholder(name: &str) -> bool {
    name == PLACEHOLDER || numbered_placeholder(name).is_some()
}

fn make_lambda(params: &[Name], body: Expr) -> Expr {
    let span = body.span;
    let pats = params
        .iter()
        .map(|name| Pattern {
            kind: PatKind::Bind(Binding {
                name: name.clone(),
                slot: 0,
                span,
            }),
            span,
        })
        .collect();
    Expr::new(
        ExprKind::Lambda(Rc::new(Lambda {
            params: pats,
            body,
            frame_size: 0,
            span,
            closed: None,
        })),
        span,
    )
}

fn cmp_op(t: &Tok, next: &Tok) -> Option<CmpOp> {
    Some(match t {
        Tok::Eq => CmpOp::Eq,
        Tok::Ne => CmpOp::Ne,
        Tok::Lt => CmpOp::Lt,
        Tok::Le => CmpOp::Le,
        Tok::Gt => CmpOp::Gt,
        Tok::Ge => CmpOp::Ge,
        Tok::In => CmpOp::In,
        Tok::Not if *next == Tok::In => CmpOp::NotIn,
        _ => return None,
    })
}

/// (operator, left binding power, right binding power)
fn bin_op(t: &Tok) -> Option<(BinOp, u8, u8)> {
    let left = |op, bp| Some((op, bp, bp + 1));
    match t {
        Tok::DotEq => left(BinOp::Dot(CmpOp::Eq), BP_DOTCMP),
        Tok::DotNe => left(BinOp::Dot(CmpOp::Ne), BP_DOTCMP),
        Tok::DotLt => left(BinOp::Dot(CmpOp::Lt), BP_DOTCMP),
        Tok::DotLe => left(BinOp::Dot(CmpOp::Le), BP_DOTCMP),
        Tok::DotGt => left(BinOp::Dot(CmpOp::Gt), BP_DOTCMP),
        Tok::DotGe => left(BinOp::Dot(CmpOp::Ge), BP_DOTCMP),
        Tok::Pipe => left(BinOp::BitOr, BP_BITOR),
        Tok::Xor => left(BinOp::Xor, BP_XOR),
        Tok::Amp => left(BinOp::BitAnd, BP_BITAND),
        Tok::Shl => left(BinOp::Shl, BP_SHIFT),
        Tok::Shr => left(BinOp::Shr, BP_SHIFT),
        Tok::Plus => left(BinOp::Add, BP_ADD),
        Tok::Minus => left(BinOp::Sub, BP_ADD),
        Tok::PlusPlus => left(BinOp::Concat, BP_ADD),
        Tok::Star => left(BinOp::Mul, BP_MUL),
        Tok::Slash => left(BinOp::Div, BP_MUL),
        Tok::Percent => left(BinOp::Mod, BP_MUL),
        // Right associative.
        Tok::StarStar => Some((BinOp::Pow, BP_POW, BP_POW)),
        _ => None,
    }
}

/// Groups adjacent clauses into functions (SPEC 7.1). A `None` stands for
/// any other item, such as a test, which separates clauses too.
pub fn group_defs(raw: Vec<Option<RawDef>>) -> Result<(Vec<FnDef>, Vec<ConstDef>)> {
    let mut fns: Vec<FnDef> = Vec::new();
    let mut consts = Vec::new();
    let mut last_was: Option<Name> = None;
    for def in raw {
        let Some(def) = def else {
            last_was = None;
            continue;
        };
        match def {
            RawDef::Const(c) => {
                consts.push(c);
                last_was = None;
            }
            RawDef::Clause {
                name,
                clause,
                annotation,
            } => {
                if last_was.as_ref() == Some(&name) {
                    let f = fns.last_mut().ok_or_else(|| {
                        Error::new("internal: clause without function", clause.span)
                    })?;
                    if clause.params.len() != f.arity {
                        return Err(Error::new(
                            format!(
                                "clause of `{name}` has {} parameters, but its first clause has {}",
                                clause.params.len(),
                                f.arity
                            ),
                            clause.span,
                        ));
                    }
                    if annotation.is_some() {
                        return Err(Error::new(
                            "annotations go before a function's first clause",
                            clause.span,
                        ));
                    }
                    f.clauses.push(clause);
                } else {
                    if fns.iter().any(|f| f.name == name) {
                        return Err(Error::new(
                            format!("clauses of `{name}` must be next to each other"),
                            clause.span,
                        ));
                    }
                    fns.push(FnDef {
                        name: name.clone(),
                        arity: clause.params.len(),
                        span: clause.span,
                        clauses: vec![clause],
                        annotation,
                        id: 0,
                    });
                    last_was = Some(name);
                }
            }
        }
    }
    Ok((fns, consts))
}
