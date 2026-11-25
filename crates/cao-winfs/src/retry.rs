//! A bounded wait for refusals that clear on their own (FS-5).
//!
//! A real-time scanner opens a file a run has just written for a few
//! milliseconds, often without sharing write or delete access. Any open or
//! move CAO makes of that file in the meantime is refused, though the same
//! call a moment later succeeds. Every such call goes through
//! [`retry_transient`], so they all wait the same bounded time.

use std::io;
use std::time::Duration;

/// The waits between attempts of a refused call: about a second in all. The
/// scanner holds observed on a loaded host cleared within 6 ms, so the first
/// retry is usually the last.
const TRANSIENT_REFUSAL_WAITS: [Duration; 7] = [
    Duration::from_millis(5),
    Duration::from_millis(10),
    Duration::from_millis(20),
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(500),
];

/// Runs `attempt` until it succeeds, fails with an error `transient`
/// rejects, or about a second of backoff is spent, and returns the last
/// result.
///
/// The waits block the calling thread, so a lasting refusal (a file another
/// program keeps open, a read-only destination) costs that second before its
/// error is returned.
pub(crate) fn retry_transient<T>(
    attempt: impl FnMut() -> io::Result<T>,
    transient: impl Fn(&io::Error) -> bool,
) -> io::Result<T> {
    retry_with(
        &TRANSIENT_REFUSAL_WAITS,
        std::thread::sleep,
        attempt,
        transient,
    )
}

/// [`retry_transient`] with the waits and the sleep supplied, so a test can
/// count attempts without waiting.
fn retry_with<T>(
    waits: &[Duration],
    mut sleep: impl FnMut(Duration),
    mut attempt: impl FnMut() -> io::Result<T>,
    transient: impl Fn(&io::Error) -> bool,
) -> io::Result<T> {
    let mut waits = waits.iter();
    loop {
        let error = match attempt() {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        match waits.next() {
            Some(wait) if transient(&error) => sleep(*wait),
            _ => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHARING_VIOLATION: i32 = 32;
    const FILE_NOT_FOUND: i32 = 2;

    fn is_sharing_violation(error: &io::Error) -> bool {
        error.raw_os_error() == Some(SHARING_VIOLATION)
    }

    /// Runs `retry_with` over the real waits with a recording sleep, where
    /// attempt `n` (from 0) returns `outcome(n)`. Returns the result, the
    /// attempt count and the waits slept.
    fn run(
        outcome: impl Fn(usize) -> io::Result<usize>,
    ) -> (io::Result<usize>, usize, Vec<Duration>) {
        let mut attempts = 0;
        let mut slept = Vec::new();
        let result = retry_with(
            &TRANSIENT_REFUSAL_WAITS,
            |wait| slept.push(wait),
            || {
                attempts += 1;
                outcome(attempts - 1)
            },
            is_sharing_violation,
        );
        (result, attempts, slept)
    }

    #[test]
    fn a_first_success_never_waits() {
        let (result, attempts, slept) = run(Ok);
        assert_eq!(result.unwrap(), 0);
        assert_eq!(attempts, 1);
        assert!(slept.is_empty());
    }

    #[test]
    fn a_transient_refusal_is_retried_until_it_clears() {
        let (result, attempts, slept) = run(|attempt| {
            if attempt < 2 {
                Err(io::Error::from_raw_os_error(SHARING_VIOLATION))
            } else {
                Ok(attempt)
            }
        });
        assert_eq!(result.unwrap(), 2);
        assert_eq!(attempts, 3);
        assert_eq!(slept, TRANSIENT_REFUSAL_WAITS[..2]);
    }

    #[test]
    fn a_lasting_refusal_spends_every_wait_and_returns_its_error() {
        let (result, attempts, slept) =
            run(|_| Err(io::Error::from_raw_os_error(SHARING_VIOLATION)));
        assert_eq!(result.unwrap_err().raw_os_error(), Some(SHARING_VIOLATION));
        assert_eq!(attempts, TRANSIENT_REFUSAL_WAITS.len() + 1);
        assert_eq!(slept, TRANSIENT_REFUSAL_WAITS);
        let total: Duration = slept.iter().sum();
        assert!(total < Duration::from_secs(2), "{total:?}");
    }

    #[test]
    fn other_errors_are_returned_at_once() {
        let (result, attempts, slept) = run(|_| Err(io::Error::from_raw_os_error(FILE_NOT_FOUND)));
        assert_eq!(result.unwrap_err().raw_os_error(), Some(FILE_NOT_FOUND));
        assert_eq!(attempts, 1);
        assert!(slept.is_empty());
    }
}
