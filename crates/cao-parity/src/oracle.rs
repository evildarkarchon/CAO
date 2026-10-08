//! The oracle parser: the C++ CLI's stdout and exit code to raw [`RunFacts`].
//!
//! The grammar is the one `src/CliRun.cpp` renders and `docs/cli.md` documents.
//! Every record ends at a newline (CRLF, since the oracle writes stdout in text
//! mode). Fields are separated by `|`, and text fields escape `\`, `|`, CR and
//! LF as `\\`, `\p`, `\r` and `\n`, so a line is split on every raw `|` first
//! and each text field is unescaped afterwards.
//!
//! The parser is strict. Anything outside the grammar, including a name or
//! code the [`tables`] do not know or terminal details out of their rendering
//! order, is a [`HarnessError`]: the harness must never turn a transcript it
//! does not understand into a verdict.

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
        let error = known_code(first_line, "Start Error", code, tables::start_error)?;
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

/// Fails with [`HarnessError::ExitCodeMismatch`] unless the oracle exited
/// with the code its reported result requires.
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
    /// The sequence number of the last event read; 0 before the first.
    last_sequence: u64,
    events: Vec<RunEventFact>,
    terminal: Option<TerminalBuilder>,
}

/// The terminal event's facts while its detail records are still arriving.
struct TerminalBuilder {
    cancellation_observed: Option<bool>,
    /// The [`detail_rank`] of the last detail record read.
    last_rank: u8,
    facts: TerminalFacts,
}

impl Parser {
    /// Reads one record: an `EVENT:` record, or a detail record of the
    /// terminal event, which may only directly follow that event.
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

    /// Reads one `EVENT:` record, after checking its Run ID and sequence.
    ///
    /// Only a Diagnostic may follow the terminal event; it ends the terminal
    /// details, since C++ prints them in the same write as the terminal event.
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
                RunEventPayload::Failure {
                    phase: phase(line, body[1])?,
                    code: known_code(line, "Run Failure Code", body[2], tables::run_failure_code)?,
                    detail: unescape(line, body[3])?,
                    path: unescape(line, body[4])?,
                }
            }
            // Anything else is a phase record; an unknown kind is an unknown phase name.
            _ => phase_event(line, body)?,
        };

        match (&self.stage, &payload) {
            (Stage::Events, _) => {}
            (_, RunEventPayload::Diagnostic { .. }) => self.stage = Stage::LateEvents,
            _ => {
                return Err(transcript(
                    line,
                    "only Diagnostic events may follow the terminal event",
                ));
            }
        }
        self.events.push(RunEventFact {
            sequence: self.last_sequence,
            payload,
        });
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
        let sequence = number(line, sequence)?;
        let expected = self.last_sequence + 1;
        if sequence != expected {
            return Err(transcript(
                line,
                format!("sequence {sequence} where {expected} was expected"),
            ));
        }
        self.last_sequence = sequence;
        Ok(())
    }

    /// Reads the terminal event, `Outcome|<outcome>|Final Phase|<phase>`, and
    /// starts collecting its detail records.
    fn outcome(&mut self, line: usize, body: &[&str]) -> Result<(), HarnessError> {
        if self.stage != Stage::Events {
            return Err(transcript(line, "a second terminal event"));
        }
        expect_fields(line, body, 4, "the terminal event")?;
        if body[2] != "Final Phase" {
            return Err(transcript(line, "the terminal event lacks `Final Phase`"));
        }
        self.terminal = Some(TerminalBuilder {
            cancellation_observed: None,
            last_rank: 0,
            facts: TerminalFacts {
                outcome: known_name(line, "Run Outcome", body[1], tables::run_outcome)?,
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

    /// Completes the run once stdout is exhausted. Fails if the stream had no
    /// terminal event or its details lacked `Cancellation Observed`.
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

/// The position of each terminal detail label in `renderDetails`'s output.
///
/// Records must arrive in non-decreasing rank. Archive Failures share a rank
/// with the Finalization Failure because C++ prints extraction failures, then
/// the Finalization Failure, then finalization's own Archive Failures.
fn detail_rank(label: &str) -> Option<u8> {
    Some(match label {
        "Cancellation Observed" => 0,
        "Mod Root" => 1,
        "Run Failure" => 2,
        "Cleanup Failure" => 3,
        "Asset Failure" => 4,
        "Archive Failure" | "Finalization Failure" => 5,
        "Committed Mutations Retained" => 6,
        "Archive Collision" => 7,
        "Skipped Assets" => 8,
        _ => return None,
    })
}

impl TerminalBuilder {
    /// Parses one terminal detail record into the matching fact list,
    /// rejecting a record that arrives out of rendering order.
    fn detail(&mut self, line: usize, fields: &[&str]) -> Result<(), HarnessError> {
        let rank = known_name(line, "terminal detail", fields[0], detail_rank)?;
        if rank < self.last_rank {
            return Err(transcript(
                line,
                format!("`{}` arrives out of the rendering order", fields[0]),
            ));
        }
        self.last_rank = rank;

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
                    kind: known_name(line, "Mutation Kind", fields[2], tables::mutation_kind)?,
                    committed: number(line, fields[3])?,
                    partial_or_unknown: number(
                        line,
                        prefixed(line, fields[4], "partial-or-unknown=")?,
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
                let reason = known_code(line, "Skip Reason", fields[1], tables::skip_reason)?;
                if facts
                    .skipped_assets
                    .iter()
                    .any(|skipped| skipped.reason == reason)
                {
                    return Err(transcript(line, "a second count for one Skip Reason"));
                }
                // The oracle prints only non-zero counts.
                let count = number(line, fields[2])?;
                if count == 0 {
                    return Err(transcript(line, "a zero Skipped Assets count"));
                }
                facts.skipped_assets.push(SkippedAssets { reason, count });
            }
            _ => unreachable!("detail_rank accepted only the labels matched above"),
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
            completed: number(line, body[2])?,
            total: number(line, body[3])?,
            succeeded: number(line, prefixed(line, body[4], "succeeded=")?)?,
            failed: number(line, prefixed(line, body[5], "failed=")?)?,
        }),
    })
}

/// `<phase>|Indeterminate` or `<phase>|Skipped|<reason>`.
fn phase_event(line: usize, body: &[&str]) -> Result<RunEventPayload, HarnessError> {
    let phase = phase(line, body[0])?;
    let status = match body.get(1..) {
        Some(["Indeterminate"]) => PhaseStatus::Indeterminate,
        Some(["Skipped", reason]) => PhaseStatus::Skipped(known_name(
            line,
            "Phase Skip Reason",
            reason,
            tables::phase_skip_reason,
        )?),
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
    known_name(line, "Run Phase", name, tables::run_phase)
}

/// Looks `name` up in a name table; a miss is [`HarnessError::UnknownName`].
fn known_name<T>(
    line: usize,
    table: &'static str,
    name: &str,
    lookup: fn(&str) -> Option<T>,
) -> Result<T, HarnessError> {
    lookup(name).ok_or_else(|| HarnessError::UnknownName {
        line,
        table,
        name: name.to_owned(),
    })
}

/// Parses an integer code field and looks it up in a code table; a code the
/// table lacks is [`HarnessError::UnknownCode`], a field that is not a code
/// at all is a transcript error.
fn known_code<T>(
    line: usize,
    table: &'static str,
    field: &str,
    lookup: fn(i64) -> Option<T>,
) -> Result<T, HarnessError> {
    let code = i64::try_from(number(line, field)?)
        .map_err(|_| transcript(line, format!("code `{field}` is out of range")))?;
    lookup(code).ok_or(HarnessError::UnknownCode { line, table, code })
}

fn detailed_path(line: usize, fields: &[&str]) -> Result<DetailedPath, HarnessError> {
    Ok(DetailedPath {
        detail: unescape(line, fields[1])?,
        path: unescape(line, fields[2])?,
    })
}

/// Fails unless a record has exactly `count` fields; `what` names the record.
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
fn number(line: usize, field: &str) -> Result<u64, HarnessError> {
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
