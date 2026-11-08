//! Abstract syntax tree. The parser produces names as `ExprKind::Name`; the
//! resolver rewrites them to `Local`, `Global` or `Builtin`, and assigns
//! frame slots to pattern bindings.

use crate::error::Span;
use crate::value::Value;
use std::rc::Rc;

pub type Name = Rc<str>;

/// Reserved parameter name for the `_` placeholder lambda (SPEC 5.4).
pub const PLACEHOLDER: &str = "_";
/// Reserved parameter name for leading-dot lambdas (SPEC 5.4).
pub const DOT_PARAM: &str = ".";

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    /// `~n`, bitwise not: `-n - 1`.
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    Xor,
    /// Element-wise comparisons `.==` etc.
    Dot(CmpOp),
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Pow => "**",
            BinOp::Concat => "++",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::Xor => "xor",
            BinOp::Dot(CmpOp::Eq) => ".==",
            BinOp::Dot(CmpOp::Ne) => ".!=",
            BinOp::Dot(CmpOp::Lt) => ".<",
            BinOp::Dot(CmpOp::Le) => ".<=",
            BinOp::Dot(CmpOp::Gt) => ".>",
            BinOp::Dot(CmpOp::Ge) => ".>=",
            BinOp::Dot(_) => ".?",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    NotIn,
}

impl CmpOp {
    pub fn symbol(self) -> &'static str {
        match self {
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
            CmpOp::In => "in",
            CmpOp::NotIn => "not in",
        }
    }
}

#[derive(Debug, Clone)]
pub enum InterpPart {
    Lit(Rc<str>),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct Arg {
    pub name: Option<Name>,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct Arm {
    pub pat: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
}

#[derive(Debug, Clone)]
pub struct Lambda {
    pub params: Vec<Pattern>,
    pub body: Expr,
    pub frame_size: usize,
    pub span: Span,
    /// Set by the resolver when the body uses no local of an enclosing
    /// frame. Such a lambda is the same value every time it is evaluated,
    /// so it is built once (`id` indexes the cache) instead of each time.
    pub closed: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompKind {
    List,
    Set,
    Map,
    Gen,
}

#[derive(Debug, Clone)]
pub enum CompClause {
    /// Each item gets a new frame of `frame_size` slots.
    For {
        pat: Pattern,
        iter: Expr,
        frame_size: usize,
    },
    If(Expr),
}

#[derive(Debug, Clone)]
pub struct Comp {
    pub kind: CompKind,
    pub body: Expr,
    /// The value expression of a map comprehension.
    pub value: Option<Expr>,
    pub clauses: Vec<CompClause>,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Lit(Value),
    Interp(Vec<InterpPart>),
    /// Unresolved name.
    Name(Name),
    /// `depth` frames up the parent chain, slot `slot`.
    Local {
        depth: usize,
        slot: usize,
        name: Name,
        /// This is the local's last use, so its value can be taken out of
        /// the frame instead of copied (see `moves`).
        moved: bool,
    },
    Global(usize),
    Builtin(usize),
    Tuple(Vec<Expr>),
    List(Vec<Expr>),
    Set(Vec<Expr>),
    Map(Vec<(Expr, Expr)>),
    Comp(Rc<Comp>),
    /// `f(args)`, or `recv.f(args)` with `method` set and `recv` as the first argument.
    Call {
        func: Box<Expr>,
        args: Vec<Arg>,
        method: bool,
    },
    TupleIndex(Box<Expr>, usize),
    Index(Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// Chained comparison: `a < b <= c`.
    Compare(Box<Expr>, Vec<(CmpOp, Expr)>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Coalesce(Box<Expr>, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Lambda(Rc<Lambda>),
    Match(Box<Expr>, Vec<Arm>),
    /// `lo..hi`, `lo..<hi`, `lo..`, and in slices `..<hi`.
    Range {
        lo: Option<Box<Expr>>,
        hi: Option<Box<Expr>>,
        inclusive: bool,
    },
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Expr {
        Expr { kind, span }
    }
}

// ---------------------------------------------------------------------------
// Patterns (SPEC 6)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeName {
    Int,
    Bool,
    Char,
    Str,
    None,
    Tuple,
    List,
    Set,
    Map,
    Range,
    RangeSet,
    Grid,
    Function,
}

impl TypeName {
    pub fn parse(s: &str) -> Option<TypeName> {
        Some(match s {
            "int" => TypeName::Int,
            "bool" => TypeName::Bool,
            "char" => TypeName::Char,
            "str" => TypeName::Str,
            "none" => TypeName::None,
            "tuple" => TypeName::Tuple,
            "list" => TypeName::List,
            "set" => TypeName::Set,
            "map" => TypeName::Map,
            "range" => TypeName::Range,
            "rangeset" => TypeName::RangeSet,
            "grid" => TypeName::Grid,
            "function" => TypeName::Function,
            _ => return None,
        })
    }
}

/// A variable bound by a pattern; `slot` is filled in by the resolver.
#[derive(Debug, Clone)]
pub struct Binding {
    pub name: Name,
    pub slot: usize,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapKind {
    Str,
    Int,
    Char,
    Word,
}

#[derive(Debug, Clone)]
pub enum StrSeg {
    Lit(String),
    Cap {
        bind: Option<Binding>,
        kind: CapKind,
    },
}

#[derive(Debug, Clone)]
pub struct Pattern {
    pub kind: PatKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum PatKind {
    Wild,
    Bind(Binding),
    Typed(Option<Binding>, TypeName),
    Lit(Value),
    Tuple(Vec<Pattern>),
    /// `[a, b]`, or with a rest marker `[a, ..rest, z]`.
    List {
        before: Vec<Pattern>,
        rest: Option<Option<Binding>>,
        after: Vec<Pattern>,
    },
    Str(Vec<StrSeg>),
}

impl Pattern {
    /// Calls `f` on every binding in the pattern, in source order.
    pub fn bindings(&self, f: &mut dyn FnMut(&Binding)) {
        match &self.kind {
            PatKind::Wild | PatKind::Lit(_) => {}
            PatKind::Bind(b) => f(b),
            PatKind::Typed(b, _) => {
                if let Some(b) = b {
                    f(b);
                }
            }
            PatKind::Tuple(ps) => ps.iter().for_each(|p| p.bindings(f)),
            PatKind::List {
                before,
                rest,
                after,
            } => {
                before.iter().for_each(|p| p.bindings(f));
                if let Some(Some(b)) = rest {
                    f(b);
                }
                after.iter().for_each(|p| p.bindings(f));
            }
            PatKind::Str(segs) => {
                for s in segs {
                    if let StrSeg::Cap { bind: Some(b), .. } = s {
                        f(b);
                    }
                }
            }
        }
    }

    /// Like `bindings`, but each binding can be changed.
    pub fn bindings_mut(&mut self, f: &mut dyn FnMut(&mut Binding)) {
        match &mut self.kind {
            PatKind::Wild | PatKind::Lit(_) => {}
            PatKind::Bind(b) => f(b),
            PatKind::Typed(b, _) => {
                if let Some(b) = b {
                    f(b);
                }
            }
            PatKind::Tuple(ps) => ps.iter_mut().for_each(|p| p.bindings_mut(f)),
            PatKind::List {
                before,
                rest,
                after,
            } => {
                before.iter_mut().for_each(|p| p.bindings_mut(f));
                if let Some(Some(b)) = rest {
                    f(b);
                }
                after.iter_mut().for_each(|p| p.bindings_mut(f));
            }
            PatKind::Str(segs) => {
                for s in segs {
                    if let StrSeg::Cap { bind: Some(b), .. } = s {
                        f(b);
                    }
                }
            }
        }
    }

    pub fn binding_names(&self) -> Vec<Name> {
        let mut names = Vec::new();
        self.bindings(&mut |b| names.push(b.name.clone()));
        names
    }
}

// ---------------------------------------------------------------------------
// Definitions (SPEC 7)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Annotation {
    Memo,
    NoMemo,
}

#[derive(Debug, Clone)]
pub struct Clause {
    pub params: Vec<Pattern>,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub wheres: Wheres,
    pub frame_size: usize,
    pub span: Span,
}

#[derive(Debug)]
pub struct FnDef {
    pub name: Name,
    pub arity: usize,
    pub clauses: Vec<Clause>,
    pub annotation: Option<Annotation>,
    /// Index into `Program::memo`, assigned by the resolver.
    pub id: usize,
    pub span: Span,
}

/// A constant, possibly destructuring: `x = ...` or `(a, b) = ...`.
#[derive(Debug)]
pub struct ConstDef {
    pub pat: Pattern,
    pub body: Expr,
    pub wheres: Wheres,
    /// Size of the frame the body is evaluated in. Only used for top-level
    /// constants; local constants share their enclosing frame.
    pub frame_size: usize,
    pub span: Span,
}

impl ConstDef {
    pub fn describe(&self) -> String {
        self.pat.binding_names().join(", ")
    }
}

/// Local definitions from a `where` block, with their frame slots.
#[derive(Debug, Clone, Default)]
pub struct Wheres {
    pub consts: Vec<Rc<ConstDef>>,
    /// (function, slot)
    pub fns: Vec<(Rc<FnDef>, usize)>,
}

/// A clause or constant as written, before clauses are grouped.
#[derive(Debug)]
pub enum RawDef {
    Clause {
        name: Name,
        clause: Clause,
        annotation: Option<Annotation>,
    },
    Const(ConstDef),
}

#[derive(Debug)]
pub struct Check {
    pub name: Name,
    pub expr: Expr,
    pub frame_size: usize,
    pub span: Span,
}

#[derive(Debug)]
pub struct Example {
    pub text: Rc<str>,
    /// Overrides of top-level constants: `with name = expr`.
    pub withs: Vec<(Name, Rc<ConstDef>, Span)>,
    pub expects: Vec<Check>,
    pub span: Span,
}

#[derive(Debug)]
pub enum Item {
    Day {
        year: u32,
        day: u32,
        span: Span,
    },
    Example(Example),
    Answers {
        expects: Vec<Check>,
        span: Span,
    },
    Test {
        check: Check,
        expect_error: bool,
    },
    Def(RawDef),
    /// `include "path"` (SPEC 2.8). Handled by the loader, which replaces
    /// it with the included file's items.
    Include {
        path: String,
        span: Span,
    },
}
