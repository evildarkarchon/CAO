//! Harness errors: anything that stops a case from producing a verdict.
//!
//! A harness error is never a Different verdict. It means the harness could not
//! trust what it read (an unknown enum name, a malformed transcript, an
//! inconsistent exit code) or could not run the case at all.

use std::path::PathBuf;

use crate::case::Side;

/// Why the harness could not produce a verdict for a case.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    /// The oracle's stdout broke the event-stream grammar.
    #[error("oracle transcript line {line}: {message}")]
    Transcript { line: usize, message: String },

    /// A name the mapping tables do not know, such as a new Run Phase label.
    #[error("oracle transcript line {line}: unknown {table} name `{name}`")]
    UnknownName {
        line: usize,
        table: &'static str,
        name: String,
    },

    /// An integer code the mapping tables do not know.
    #[error("oracle transcript line {line}: unknown {table} code {code}")]
    UnknownCode {
        line: usize,
        table: &'static str,
        code: i64,
    },

    /// The oracle's exit code contradicts the result its stdout reported.
    #[error("oracle exit code {actual} contradicts its reported result, which requires {expected}")]
    ExitCodeMismatch { expected: i32, actual: i32 },

    /// A reported path lies outside the side's case root, so it cannot be
    /// made comparable.
    #[error("path `{path}` is outside the case root `{root}`")]
    PathOutsideCaseRoot { path: String, root: String },

    /// One side's facts contradict themselves.
    #[error("inconsistent run facts: {0}")]
    InconsistentFacts(String),

    /// The case spec or case directory cannot be used.
    #[error("invalid case: {0}")]
    InvalidCase(String),

    /// A side was still running when the case's timeout ran out, and was killed.
    #[error("the {side} side did not finish within the case's {seconds} s timeout")]
    Timeout { side: Side, seconds: u64 },

    /// The Rust driver exited without producing facts.
    #[error("the Rust driver exited with code {code}")]
    DriverFailed { code: i32 },

    /// A filesystem or process operation failed.
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    /// A JSON document could not be read or written.
    #[error("{path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl HarnessError {
    /// Wraps an I/O error with what the harness was doing.
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}
