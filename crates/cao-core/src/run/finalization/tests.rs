//! Archive Finalization's outer panic containment, which no public seam
//! reaches after the phase has planned.
//!
//! Every fake the public API offers is already contained closer in once the
//! output total is recorded: an output attempt contains its own panics as
//! that attempt's Unexpected Exception, Loading Plugin maintenance and
//! pruning run inside their own boundary, and the Capacity Check contains
//! its probes. The one callback the outer boundary calls directly between
//! outputs is the cancellation poll, and only the crate supplies it:
//! production polls the run's token, which cannot panic. So this scenario
//! drives the phase through the executor's crate-private work seam,
//! [`RunExecutor::execute_with_work`], whose work opens Archive Finalization
//! and runs it over the real Run Toolset with a poll that panics once the
//! first output has been written. Everything else, Preparing included, is a
//! production run over the fakes.

use std::ops::ControlFlow;
use std::path::PathBuf;

use crate::execution::MutationState;
use crate::run::{
    ArchiveFinalizationFailure, ArchiveFinalizationSettings, CancellationToken, RunExecutor,
    RunOutcome, RunPhase, RunServices, create_run_id,
};
use crate::test_support::{ArchiveRunRequest, CountingCleanup, ScriptedToolset};

/// A fresh, empty, canonical directory for one scenario under the system
/// temporary directory: `CARGO_TARGET_TMPDIR` exists only for integration
/// tests.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("cao-core-unit")
        .join(format!("{name}-{}", std::process::id()));
    // A missing directory is the expected case; anything else surfaces in create_dir_all.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    cao_winfs::msvc_canonical(&dir).unwrap()
}

/// A panic after the plan that reaches the outer containment finishes the
/// phase once as a phase-level Unexpected Exception that forbids
/// continuation, and the output attempt streamed before it survives in the
/// public result beside that failure.
#[test]
fn a_panic_after_the_plan_reaching_the_outer_containment_keeps_the_streamed_attempts() {
    let parent = scratch_dir("finalization-outer-panic");
    let roots = [parent.join("mod-a"), parent.join("mod-b")];
    for root in &roots {
        let texture = root.join("textures").join("asset.dds");
        std::fs::create_dir_all(texture.parent().unwrap()).unwrap();
        std::fs::write(&texture, b"source bytes").unwrap();
    }
    // One Texture per Mod Root plans one output each.
    let run = ArchiveRunRequest::over_mods(&parent).with_archive_finalization(
        ArchiveFinalizationSettings {
            compress: false,
            create_dummy_plugins: false,
            ..ArchiveFinalizationSettings::default()
        },
    );
    let configuration = run.configuration(ScriptedToolset::default());
    let packer = configuration.toolset.packer.clone();
    let mut cleanup = CountingCleanup::default();

    let result = RunExecutor.execute_with_work(
        &run.request(),
        RunServices {
            safety_cleanup: &mut cleanup,
            observations: None,
            configuration: Some(&configuration),
        },
        &CancellationToken::new(),
        create_run_id(),
        move |preparation, choices, toolset, evidence, artifacts, _stop| {
            let finalization = evidence.open_archive_finalization()?;
            // Planning and the first output poll before anything is
            // written; the poll before the second output is the first to
            // follow a write, so it panics between the two outputs.
            let panicking_poll = || {
                if !packer.writes().is_empty() {
                    panic!("the cancellation poll panicked");
                }
                false
            };
            toolset.finalize_archives(
                preparation,
                &choices.archive_finalization,
                finalization,
                artifacts,
                &panicking_poll,
            )?;
            Ok(ControlFlow::Continue(()))
        },
    );

    let progress = result
        .phase(RunPhase::ArchiveFinalization)
        .and_then(|record| record.progress())
        .expect("the phase planned its outputs");
    assert_eq!((progress.total(), progress.completed()), (2, 1));
    let finalization = result
        .archive_finalization()
        .expect("the phase finished with a result");
    assert_eq!(
        finalization.attempts.len(),
        1,
        "the streamed attempt survives"
    );
    let attempt = &finalization.attempts[0];
    assert!(attempt.succeeded(), "{:?}", attempt.outcome);
    assert_eq!(attempt.outcome.mutation(), MutationState::Committed);
    assert_eq!(attempt.mod_root, roots[0]);
    assert_eq!(
        finalization.failure,
        Some(ArchiveFinalizationFailure::UnexpectedException)
    );
    assert_eq!(finalization.detail, "the cancellation poll panicked");
    assert!(!finalization.safe_to_continue);
    assert!(!finalization.cancelled);
    // The phase-level verdict forbids continuation, so the run fails, but
    // the panic was the phase's own: it escaped no further.
    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert_eq!(cleanup.passes, 1);
}
