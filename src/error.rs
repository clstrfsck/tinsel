//! Errors with source spans and a short Tinsel stack trace (SPEC 8.3).

use std::fmt;

/// Byte range in the program's source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

/// One file of a program's source (SPEC 2.8). A program's files are
/// stored one after another in a single string, so a [`Span`] is an offset
/// into that string; a file covers the offsets from its `start` up to the
/// next file's `start`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    pub start: usize,
    /// Relative to the main file's directory. Empty for the main file.
    pub path: std::path::PathBuf,
}

/// Finds the file holding `offset`: its index in `files` and its text.
/// With no files listed, the whole of `src` counts as one file.
pub fn file_at<'a>(src: &'a str, files: &[SourceFile], offset: usize) -> (usize, usize, &'a str) {
    let i = files.iter().rposition(|f| f.start <= offset).unwrap_or(0);
    let start = files.get(i).map_or(0, |f| f.start);
    let end = files.get(i + 1).map_or(src.len(), |f| f.start);
    (i, start, src.get(start..end).unwrap_or(""))
}

/// Maximum number of stack-trace entries kept (SPEC 8.3).
pub const MAX_TRACE: usize = 20;

/// A runtime or compile error. Boxed, so that `Result<Value>` stays small:
/// every evaluator frame holds a few of them, and frame size limits how deep
/// recursion can go (SPEC 8.2).
#[derive(Debug, Clone)]
pub struct Error(Box<ErrorInfo>);

#[derive(Debug, Clone)]
pub struct ErrorInfo {
    pub msg: String,
    pub span: Option<Span>,
    /// Most recent call first.
    pub trace: Vec<TraceEntry>,
}

/// One entry of a Tinsel stack trace (SPEC 8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEntry {
    pub name: String,
    /// Where the function was called.
    pub span: Span,
    /// How many times in a row this same call appears: deep recursion
    /// shows as one entry rather than filling the trace.
    pub repeats: usize,
    /// Tail calls made after this call and before the error. Tail calls
    /// reuse the caller's place on the stack, so they aren't listed
    /// individually.
    pub tail_calls: usize,
    /// The call's arguments, shown briefly, when they were recorded (SPEC
    /// 8.3): only the tree-walker records them, and only when asked. For a
    /// repeated call, the innermost call's.
    pub args: Option<Vec<String>>,
}

impl TraceEntry {
    /// e.g. "`down` called here (30 times)" or "`step` called at
    /// day1.tin:3:7, then 3 tail calls", where `at` is "here" or a location.
    /// With arguments, "`step((7, 0), 3)` called here".
    pub fn describe(&self, at: &str) -> String {
        let call = match &self.args {
            Some(args) => format!("{}({})", self.name, args.join(", ")),
            None => self.name.clone(),
        };
        let mut s = format!("`{call}` called {at}");
        if self.repeats > 1 {
            // Only the innermost of the repeated calls' arguments are kept.
            let shown = if self.args.is_some() {
                ", the innermost shown"
            } else {
                ""
            };
            s.push_str(&format!(" ({} times{shown})", self.repeats));
        }
        match self.tail_calls {
            0 => {}
            1 => s.push_str(", then 1 tail call"),
            n => s.push_str(&format!(", then {n} tail calls")),
        }
        s
    }
}

impl std::ops::Deref for Error {
    type Target = ErrorInfo;
    fn deref(&self) -> &ErrorInfo {
        &self.0
    }
}

impl std::ops::DerefMut for Error {
    fn deref_mut(&mut self) -> &mut ErrorInfo {
        &mut self.0
    }
}

impl Error {
    pub fn new(msg: impl Into<String>, span: Span) -> Error {
        Error(Box::new(ErrorInfo {
            msg: msg.into(),
            span: Some(span),
            trace: Vec::new(),
        }))
    }

    pub fn msg(msg: impl Into<String>) -> Error {
        Error(Box::new(ErrorInfo {
            msg: msg.into(),
            span: None,
            trace: Vec::new(),
        }))
    }

    /// The message, without the span or trace.
    pub fn into_msg(self) -> String {
        self.0.msg
    }

    /// Attaches a span if the error doesn't have one yet.
    pub fn at(mut self, span: Span) -> Error {
        if self.span.is_none() {
            self.span = Some(span);
        }
        self
    }

    pub fn push_trace(
        &mut self,
        name: &str,
        span: Span,
        tail_calls: usize,
        args: Option<Vec<String>>,
    ) {
        if let Some(last) = self.trace.last_mut()
            && last.name == name
            && last.span == span
            && last.tail_calls == tail_calls
        {
            last.repeats += 1;
        } else if self.trace.len() < MAX_TRACE {
            self.trace.push(TraceEntry {
                name: name.to_string(),
                span,
                repeats: 1,
                tail_calls,
                args,
            });
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Maps byte offsets to 1-based line and column numbers.
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(src: &str) -> LineIndex {
        let mut starts = vec![0];
        starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { starts }
    }

    /// 1-based line number.
    pub fn line(&self, offset: usize) -> usize {
        match self.starts.binary_search(&offset) {
            Ok(i) => i + 1,
            Err(i) => i,
        }
    }

    /// 1-based (line, column), counting columns in chars.
    pub fn line_col(&self, src: &str, offset: usize) -> (usize, usize) {
        let line = self.line(offset);
        let start = self.starts[line - 1];
        let col = src.get(start..offset).map_or(0, |s| s.chars().count()) + 1;
        (line, col)
    }
}
