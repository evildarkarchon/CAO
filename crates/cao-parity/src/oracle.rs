//! The oracle parser: the C++ CLI's stdout and exit code to raw [`RunFacts`].
//!
//! The grammar is the one `src/CliRun.cpp` renders and `docs/cli.md` documents.
//! Every record ends at a newline (CRLF, since the oracle writes stdout in text
//! mode). Fields are separated by `|`, and text fields escape `\`, `|`, CR and
//! LF as `\\`, `\p`, `\r` and `\n`, so a line is split on every raw `|` first
//! and each text field is unescaped afterwards.
//!
//! The parser is strict. Anything outside the grammar, including a name or
//! code the [`tables`] do not know, is a [`HarnessError`]: the harness must
//! never turn a transcript it does not understand into a verdict.

pub mod tables;

use crate::HarnessError;
use crate::facts::{
    ArchiveCollision, ArchiveFailure, AssetFailure, CommittedMutations, DetailedPath, PhaseStatus,
    Progress, RunEventFact, RunEventPayload, RunFacts, RunOutcome, RunPhase, SkippedAssets,
    StartedRun, TerminalFacts,
};

/// Parses the oracle's complete stdout, read to process exit, and its exit code.
///
/// Reading to exit matters: diagnostics can be published after the terminal
/// event. The exit code is not a fact; it is only checked against the reported
/// result (0, 1, 2 or 130, or 2 for a Start Error), and a contradiction is a
/// [`HarnessError::ExitCodeMismatch`].
pub fn parse(stdout: &[u8], exit_code: i32) -> Result<RunFacts, HarnessError> {
    let text = std::str::from_utf8(stdout).map_err(|error| HarnessError::Transcript {
        line: line_of_offset(stdout, error.valid_up_to()),
        message: "stdout is not UTF-8".into(),
    })?;
    let lines = records(text)?;
    let Some(&(first_line, first)) = lines.first() else {
        return Err(HarnessError::Transcript {
            line: 0,
            message: "the oracle wrote no records; it rejected its arguments or crashed".into(),
        });
    };

    if let Some(code) = first.strip_prefix("Start Error: ") {
        if let Some(&(line, _)) = lines.get(1) {
            return Err(transcript(line, "a Start Error must be the only record"));
        }
        let code = number(code, first_line)?;
        let error = tables::start_error(code as i64).ok_or(HarnessError::UnknownCode {
            line: first_line,
            table: "StartError",
            code: code as i64,
        })?;
        check_exit_code(2, exit_code)?;
        return Ok(RunFacts::StartError(error));
    }

    let mut parser = Parser::default();
    for &(line, record) in &lines {
        parser.record(line, record)?;
    }
    let last_line = lines.last().map_or(0, |&(line, _)| line);
    let run = parser.finish(last_line)?;
    check_exit_code(exit_code_for(run.terminal.outcome), exit_code)?;
    Ok(RunFacts::Started(run))
}

/// The CLI's exit code for each Run Outcome (`cao::cli::exitCode`).
fn exit_code_for(outcome: RunOutcome) -> i32 {
    match outcome {
        RunOutcome::Succeeded => 0,
        RunOutcome::CompletedWithFailures => 1,
        RunOutcome::Failed => 2,
        RunOutcome::Cancelled => 130,
    }
}

fn check_exit_code(expected: i32, actual: i32) -> Result<(), HarnessError> {
    if expected == actual {
        Ok(())
    } else {
        Err(HarnessError::ExitCodeMismatch { expected, actual })
    }
}

/// Splits stdout into numbered records, dropping each record's line terminator.
///
/// A raw CR may only appear as part of a CRLF terminator, because the oracle
/// escapes CR inside fields. A final record without a terminator means the
/// output was cut off, so it is rejected rather than parsed as complete.
fn records(text: &str) -> Result<Vec<(usize, &str)>, HarnessError> {
    let mut records = Vec::new();
    let mut rest = text;
    let mut line = 0;
    while !rest.is_empty() {
        line += 1;
        let Some(end) = rest.find('\n') else {
            return Err(transcript(line, "the final record has no line terminator"));
        };
        let record = &rest[..end];
        let record = record.strip_suffix('\r').unwrap_or(record);
        if record.contains('\r') {
            return Err(transcript(line, "a raw carriage return inside a record"));
        }
        records.push((line, record));
        rest = &rest[end + 1..];
    }
    Ok(records)
}

/// The 1-based line holding byte `offset`, for reporting invalid UTF-8.
fn line_of_offset(bytes: &[u8], offset: usize) -> usize {
    bytes[..offset]
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count()
        + 1
}

fn transcript(line: usize, message: impl Into<String>) -> HarnessError {
    HarnessError::Transcript {
        line,
        message: message.into(),
    }
}

/// Where the parser is in the stream.
#[derive(Default, PartialEq, Eq)]
enum Stage {
    /// Before the terminal `Outcome` event.
    #[default]
    Events,
    /// Directly after the terminal event, reading its detail records.
    TerminalDetails,
    /// After a late event; no further detail records may appear.
    LateEvents,
}

/// Accumulates one started run's records.
#[derive(Default)]
struct Parser {
    stage: Stage,
    run_id: Option<String>,
    next_sequence: u64,
    events: Vec<RunEventFact>,
    terminal: Option<TerminalBuilder>,
}

/// The terminal event's facts while its detail records are still arriving.
struct TerminalBuilder {
    cancellation_observed: Option<bool>,
    facts: TerminalFacts,
}

impl Parser {
    fn record(&mut self, line: usize, record: &str) -> Result<(), HarnessError> {
        let fields: Vec<&str> = record.split('|').collect();
        if fields[0] == "EVENT:" {
            return self.event(line, &fields);
        }
        if self.stage != Stage::TerminalDetails {
            return Err(transcript(
                line,
                "a record without the `EVENT:` prefix outside the terminal details",
            ));
        }
        let terminal = self
            .terminal
            .as_mut()
            .expect("terminal details follow the terminal event");
        terminal.detail(line, &fields)
    }

    fn event(&mut self, line: usize, fields: &[&str]) -> Result<(), HarnessError> {
        if fields.len() < 4 {
            return Err(transcript(
                line,
                "an event needs a Run ID, a sequence and a kind",
            ));
        }
        self.check_identity(line, fields[1], fields[2])?;
        let body = &fields[3..];

        if body[0] == "Outcome" {
            return self.outcome(line, body);
        }
        let payload = match body[0] {
            "PROGRESS:" => progress_event(line, body)?,
            "Diagnostic" => {
                expect_fields(line, body, 4, "a Diagnostic event")?;
                RunEventPayload::Diagnostic {
                    phase: phase(line, body[1])?,
                    detail: unescape(line, body[2])?,
                    path: unescape(line, body[3])?,
                }
            }
            "Failure" => {
                expect_fields(line, body, 5, "a Failure event")?;
                let code = number(body[2], line)? as i64;
                RunEventPayload::Failure {
                    phase: phase(line, body[1])?,
                    code: tables::run_failure_code(code).ok_or(HarnessError::UnknownCode {
                        line,
                        table: "RunFailureCode",
                        code,
                    })?,
                    detail: unescape(line, body[3])?,
                    path: unescape(line, body[4])?,
                }
            }
            // Anything else is a phase record; an unknown kind is an unknown phase name.
            _ => phase_event(line, body)?,
        };

        match (&self.stage, &payload) {
            (Stage::Events, _) => {}
            // Only presentation diagnostics can be published after terminal commit.
            (_, RunEventPayload::Diagnostic { .. }) => self.stage = Stage::LateEvents,
            _ => {
                return Err(transcript(
                    line,
                    "only Diagnostic events may follow the terminal event",
                ));
            }
        }
        let sequence = self.next_sequence;
        self.events.push(RunEventFact { sequence, payload });
        Ok(())
    }

    /// Checks the event belongs to this run and arrives at the next sequence number.
    fn check_identity(
        &mut self,
        line: usize,
        run_id: &str,
        sequence: &str,
    ) -> Result<(), HarnessError> {
        let valid_id = !run_id.is_empty()
            && run_id.len() <= 128
            && run_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        if !valid_id {
            return Err(transcript(line, format!("invalid Run ID `{run_id}`")));
        }
        match &self.run_id {
            None => self.run_id = Some(run_id.to_owned()),
            Some(known) if known == run_id => {}
            Some(known) => {
                return Err(transcript(
                    line,
                    format!("Run ID `{run_id}` differs from the run's `{known}`"),
                ));
            }
        }
        let sequence = number(sequence, line)?;
        let expected = self.next_sequence + 1;
        if sequence != expected {
            return Err(transcript(
                line,
                format!("sequence {sequence} where {expected} was expected"),
            ));
        }
        self.next_sequence = sequence;
        Ok(())
    }

    fn outcome(&mut self, line: usize, body: &[&str]) -> Result<(), HarnessError> {
        if self.stage != Stage::Events {
            return Err(transcript(line, "a second terminal event"));
        }
        expect_fields(line, body, 4, "the terminal event")?;
        if body[2] != "Final Phase" {
            return Err(transcript(line, "the terminal event lacks `Final Phase`"));
        }
        let outcome = tables::run_outcome(body[1]).ok_or_else(|| HarnessError::UnknownName {
            line,
            table: "Run Outcome",
            name: body[1].to_owned(),
        })?;
        self.terminal = Some(TerminalBuilder {
            cancellation_observed: None,
            facts: TerminalFacts {
                outcome,
                final_phase: phase(line, body[3])?,
                // Set from the `Cancellation Observed` record in `finish`.
                cancellation_observed: false,
                mod_roots: Vec::new(),
                run_failures: Vec::new(),
                cleanup_failures: Vec::new(),
                asset_failures: Vec::new(),
                archive_failures: Vec::new(),
                finalization_failure: None,
                committed_mutations: Vec::new(),
                archive_collisions: Vec::new(),
                skipped_assets: Vec::new(),
            },
        });
        self.stage = Stage::TerminalDetails;
        Ok(())
    }

    fn finish(self, last_line: usize) -> Result<StartedRun, HarnessError> {
        let Some(terminal) = self.terminal else {
            return Err(transcript(
                last_line,
                "the stream has no terminal Outcome event",
            ));
        };
        let Some(cancellation_observed) = terminal.cancellation_observed else {
            return Err(transcript(
                last_line,
                "the terminal details lack `Cancellation Observed`",
            ));
        };
        Ok(StartedRun {
            run_id: self.run_id.expect("a terminal event implies a Run ID"),
            events: self.events,
            terminal: TerminalFacts {
                cancellation_observed,
                ..terminal.facts
            },
        })
    }
}

impl TerminalBuilder {
    /// Parses one terminal detail record into the matching fact list.
    fn detail(&mut self, line: usize, fields: &[&str]) -> Result<(), HarnessError> {
        let facts = &mut self.facts;
        match fields[0] {
            "Cancellation Observed" => {
                expect_fields(line, fields, 2, "Cancellation Observed")?;
                if self.cancellation_observed.is_some() {
                    return Err(transcript(line, "a second `Cancellation Observed`"));
                }
                self.cancellation_observed = Some(yes_no(line, fields[1])?);
            }
            "Mod Root" => {
                expect_fields(line, fields, 2, "a Mod Root")?;
                facts.mod_roots.push(unescape(line, fields[1])?);
            }
            "Run Failure" => {
                expect_fields(line, fields, 3, "a Run Failure")?;
                facts.run_failures.push(detailed_path(line, fields)?);
            }
            "Cleanup Failure" => {
                expect_fields(line, fields, 3, "a Cleanup Failure")?;
                facts.cleanup_failures.push(detailed_path(line, fields)?);
            }
            "Asset Failure" => {
                expect_fields(line, fields, 6, "an Asset Failure")?;
                facts.asset_failures.push(AssetFailure {
                    path: unescape(line, fields[1])?,
                    operation: unescape(line, fields[2])?,
                    message: unescape(line, fields[3])?,
                    affected_path: unescape(line, fields[4])?,
                    service_detail: unescape(line, fields[5])?,
                });
            }
            "Archive Failure" => {
                expect_fields(line, fields, 3, "an Archive Failure")?;
                facts.archive_failures.push(ArchiveFailure {
                    archive_path: unescape(line, fields[1])?,
                    detail: unescape(line, fields[2])?,
                });
            }
            "Finalization Failure" => {
                expect_fields(line, fields, 2, "a Finalization Failure")?;
                if facts.finalization_failure.is_some() {
                    return Err(transcript(line, "a second Finalization Failure"));
                }
                facts.finalization_failure = Some(unescape(line, fields[1])?);
            }
            "Committed Mutations Retained" => {
                expect_fields(line, fields, 5, "Committed Mutations Retained")?;
                facts.committed_mutations.push(CommittedMutations {
                    mod_root: unescape(line, fields[1])?,
                    kind: tables::mutation_kind(fields[2]).ok_or_else(|| {
                        HarnessError::UnknownName {
                            line,
                            table: "mutation kind",
                            name: fields[2].to_owned(),
                        }
                    })?,
                    committed: number(fields[3], line)?,
                    partial_or_unknown: number(
                        prefixed(line, fields[4], "partial-or-unknown=")?,
                        line,
                    )?,
                });
            }
            "Archive Collision" => {
                if fields.len() < 5 {
                    return Err(transcript(
                        line,
                        "an Archive Collision needs at least 5 fields",
                    ));
                }
                let shadowed_archives = fields[5..]
                    .iter()
                    .map(|field| unescape(line, prefixed(line, field, "shadowed=")?))
                    .collect::<Result<_, _>>()?;
                facts.archive_collisions.push(ArchiveCollision {
                    mod_root: unescape(line, fields[1])?,
                    game_path: unescape(line, fields[2])?,
                    winning_archive: unescape(line, prefixed(line, fields[3], "winner=")?)?,
                    loose_asset_wins: yes_no(
                        line,
                        prefixed(line, fields[4], "loose-asset-wins=")?,
                    )?,
                    shadowed_archives,
                });
            }
            "Skipped Assets" => {
                expect_fields(line, fields, 3, "Skipped Assets")?;
                let code = number(fields[1], line)? as i64;
                let reason = tables::skip_reason(code).ok_or(HarnessError::UnknownCode {
                    line,
                    table: "SkipReason",
                    code,
                })?;
                if facts
                    .skipped_assets
                    .iter()
                    .any(|skipped| skipped.reason == reason)
                {
                    return Err(transcript(line, "a second count for one Skip Reason"));
                }
                // The oracle prints only non-zero counts.
                let count = number(fields[2], line)?;
                if count == 0 {
                    return Err(transcript(line, "a zero Skipped Assets count"));
                }
                facts.skipped_assets.push(SkippedAssets { reason, count });
            }
            label => {
                return Err(HarnessError::UnknownName {
                    line,
                    table: "terminal detail",
                    name: label.to_owned(),
                });
            }
        }
        Ok(())
    }
}

/// `PROGRESS:|<phase>|<completed>|<total>|succeeded=<n>|failed=<n>`.
fn progress_event(line: usize, body: &[&str]) -> Result<RunEventPayload, HarnessError> {
    expect_fields(line, body, 6, "a PROGRESS event")?;
    Ok(RunEventPayload::Phase {
        phase: phase(line, body[1])?,
        status: PhaseStatus::Progress(Progress {
            completed: number(body[2], line)?,
            total: number(body[3], line)?,
            succeeded: number(prefixed(line, body[4], "succeeded=")?, line)?,
            failed: number(prefixed(line, body[5], "failed=")?, line)?,
        }),
    })
}

/// `<phase>|Indeterminate` or `<phase>|Skipped|<reason>`.
fn phase_event(line: usize, body: &[&str]) -> Result<RunEventPayload, HarnessError> {
    let phase = phase(line, body[0])?;
    let status = match body.get(1..) {
        Some(["Indeterminate"]) => PhaseStatus::Indeterminate,
        Some(["Skipped", reason]) => {
            PhaseStatus::Skipped(tables::phase_skip_reason(reason).ok_or_else(|| {
                HarnessError::UnknownName {
                    line,
                    table: "Phase Skip Reason",
                    name: (*reason).to_owned(),
                }
            })?)
        }
        _ => {
            return Err(transcript(
                line,
                "a phase record is neither Indeterminate nor Skipped",
            ));
        }
    };
    Ok(RunEventPayload::Phase { phase, status })
}

fn phase(line: usize, name: &str) -> Result<RunPhase, HarnessError> {
    tables::run_phase(name).ok_or_else(|| HarnessError::UnknownName {
        line,
        table: "Run Phase",
        name: name.to_owned(),
    })
}

fn detailed_path(line: usize, fields: &[&str]) -> Result<DetailedPath, HarnessError> {
    Ok(DetailedPath {
        detail: unescape(line, fields[1])?,
        path: unescape(line, fields[2])?,
    })
}

fn expect_fields(
    line: usize,
    fields: &[&str],
    count: usize,
    what: &str,
) -> Result<(), HarnessError> {
    if fields.len() == count {
        Ok(())
    } else {
        Err(transcript(
            line,
            format!(
                "{what} has {} fields where {count} were expected",
                fields.len()
            ),
        ))
    }
}

/// An unsigned decimal counter: ASCII digits only, so `+1` or ` 1` are rejected.
fn number(field: &str, line: usize) -> Result<u64, HarnessError> {
    if field.is_empty() || !field.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(transcript(
            line,
            format!("`{field}` is not an unsigned number"),
        ));
    }
    field
        .parse()
        .map_err(|_| transcript(line, format!("`{field}` is out of range")))
}

fn yes_no(line: usize, field: &str) -> Result<bool, HarnessError> {
    match field {
        "yes" => Ok(true),
        "no" => Ok(false),
        _ => Err(transcript(
            line,
            format!("`{field}` is neither `yes` nor `no`"),
        )),
    }
}

fn prefixed<'a>(line: usize, field: &'a str, prefix: &str) -> Result<&'a str, HarnessError> {
    field
        .strip_prefix(prefix)
        .ok_or_else(|| transcript(line, format!("`{field}` lacks the `{prefix}` prefix")))
}

/// Reverses the oracle's text-field escaping; any other escape is malformed.
fn unescape(line: usize, field: &str) -> Result<String, HarnessError> {
    let mut text = String::with_capacity(field.len());
    let mut characters = field.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            text.push(character);
            continue;
        }
        text.push(match characters.next() {
            Some('\\') => '\\',
            Some('p') => '|',
            Some('r') => '\r',
            Some('n') => '\n',
            Some(other) => return Err(transcript(line, format!("unknown escape `\\{other}`"))),
            None => return Err(transcript(line, "a field ends in a lone backslash")),
        });
    }
    Ok(text)
}
