#include "CliRun.h"
#include "Run/RunWorkRecord.h"
#include <chrono>
#include <atomic>
#include <csignal>
#include <sstream>
#include <stdexcept>
#include <thread>
#ifdef _WIN32
#define NOMINMAX
#include <windows.h>
#endif

namespace cao::cli {
namespace {
#ifdef _WIN32
std::atomic<bool> interruptRequested{};
/// Windows dispatches console events on another thread; never borrow a handle in this callback.
BOOL WINAPI consoleControl(const DWORD event) {
    if (event != CTRL_C_EVENT && event != CTRL_BREAK_EVENT) return FALSE;
    interruptRequested.store(true, std::memory_order_relaxed);
    return TRUE;
}
#else
volatile std::sig_atomic_t interruptRequested = 0;
/// A signal handler only latches intent; normal execution performs cancellation and output.
void consoleSignal(int) { interruptRequested = 1; }
#endif
/// Uses the glossary labels for lifecycle facts supplied by the service.
const char* phaseName(const run::RunPhase phase) noexcept {
    switch (phase) {
        case run::RunPhase::Preparing:
            return "Preparing";
        case run::RunPhase::DiscoveringArchives:
            return "Discovering Archives";
        case run::RunPhase::ExtractingArchives:
            return "Extracting Archives";
        case run::RunPhase::BuildingEffectiveAssetTree:
            return "Building the Effective Asset Tree";
        case run::RunPhase::ProcessingAssets:
            return "Processing Assets";
        case run::RunPhase::ArchiveFinalization:
            return "Archive Finalization";
        case run::RunPhase::SafetyCleanup:
            return "Safety Cleanup";
    }
    return "Unknown Phase";
}
/// Keeps terminal labels distinct from phase completion and progress.
const char* outcomeName(const run::RunOutcome outcome) noexcept {
    switch (outcome) {
        case run::RunOutcome::Succeeded:
            return "Succeeded";
        case run::RunOutcome::CompletedWithFailures:
            return "Completed With Failures";
        case run::RunOutcome::Cancelled:
            return "Cancelled";
        case run::RunOutcome::Failed:
            return "Failed";
    }
    return "Failed";
}
/// Names durable mutation scopes independently of the current lifecycle phase.
const char* mutationName(const run::MutationKind kind) noexcept {
    switch (kind) {
        case run::MutationKind::ArchiveExtraction:
            return "Archive Extraction";
        case run::MutationKind::AssetProcessing:
            return "Asset Processing";
        case run::MutationKind::ArchiveFinalization:
            return "Archive Finalization";
    }
    return "Unknown Mutation";
}

/// Renders terminal-owned evidence, including effects that cancellation cannot roll back.
void renderDetails(std::ostream& text, const run::OptimizationRunResult& result) {
    text << "\nCancellation Observed|" << (result.cancellationObserved() ? "yes" : "no");
    for (const auto& root : result.modRoots()) text << "\nMod Root|" << root;
    for (const auto& failure : result.cleanupFailures())
        text << "\nCleanup Failure|" << failure.detail() << '|' << failure.path();
    for (const auto& attempt : result.work().assetAttempts) {
        if (!attempt.result.succeeded())
            text << "\nAsset Failure|" << attempt.asset.executionPath() << '|'
                 << attempt.result.operation() << '|' << attempt.result.message() << '|'
                 << attempt.result.serviceDetail();
    }
    for (const auto& attempt : result.work().archiveAttempts)
        if (!attempt.succeeded())
            text << "\nArchive Failure|" << attempt.archivePath << '|' << attempt.detail;
    for (const auto& finalization : result.work().finalizations) {
        if (finalization.failure) text << "\nFinalization Failure|" << finalization.detail;
        for (const auto& attempt : finalization.attempts)
            if (!attempt.succeeded())
                text << "\nArchive Failure|" << attempt.archivePath << '|' << attempt.detail;
    }
    for (const auto& mutation : result.mutationSummaries()) {
        // These are completed effects retained by the service, never estimates of remaining work.
        text << "\nCommitted Mutations Retained|" << mutation.modRoot.generic_string() << '|'
             << mutationName(mutation.kind) << '|' << mutation.committed
             << "|partial-or-unknown=" << mutation.partialOrUnknown;
    }
    for (const auto& collision : result.work().collisions) {
        text << "\nArchive Collision|" << collision.gamePath()
             << "|winner=" << collision.winningArchive()
             << "|loose-asset-wins=" << collision.looseAssetWins();
        for (const auto& shadowed : collision.shadowedArchives()) text << "|shadowed=" << shadowed;
    }
    for (const auto reason :
         {routing::SkipReason::DisabledPhase, routing::SkipReason::DisabledAssetKind,
          routing::SkipReason::ExcludedAssetVariant})
        if (const auto count = result.skippedAssetCount(reason); count != 0)
            text << "\nSkipped Assets|" << static_cast<int>(reason) << '|' << count;
}
}  // namespace

void renderEvent(std::ostream& output, const run::RunEvent& event) {
    std::ostringstream text;
    text << "EVENT:|" << event.runId() << '|' << event.sequence() << '|';
    if (const auto* phase = std::get_if<run::RunPhaseRecord>(&event.payload())) {
        if (phase->progress()) {
            const auto& progress = *phase->progress();
            text << "PROGRESS:|" << phaseName(phase->phase()) << '|' << progress.completed() << '|'
                 << progress.total() << "|succeeded=" << progress.succeeded()
                 << "|failed=" << progress.failed();
        } else {
            text << phaseName(phase->phase());
            if (phase->status() == run::RunPhaseStatus::Skipped)
                text << "|Skipped|"
                     << (phase->skipReason() == run::PhaseSkipReason::DryRun ? "Dry Run"
                                                                             : "No Requested Work");
            else
                text << "|Indeterminate";
        }
    } else if (const auto* diagnostic = std::get_if<run::RunDiagnostic>(&event.payload())) {
        text << "Diagnostic|" << phaseName(diagnostic->phase()) << '|' << diagnostic->detail()
             << '|' << diagnostic->path();
    } else if (const auto* failure = std::get_if<run::RunFailure>(&event.payload())) {
        text << "Failure|" << phaseName(failure->phase()) << '|'
             << static_cast<int>(failure->code()) << '|' << failure->detail() << '|'
             << failure->path();
    } else {
        const auto& result =
            *std::get<std::shared_ptr<const run::OptimizationRunResult>>(event.payload());
        text << "Outcome|" << outcomeName(result.outcome()) << "|Final Phase|"
             << phaseName(result.finalPhase());
        renderDetails(text, result);
    }
    output << text.str() << std::endl;
}
int exitCode(const run::RunOutcome outcome) noexcept {
    switch (outcome) {
        case run::RunOutcome::Succeeded:
            return 0;
        case run::RunOutcome::CompletedWithFailures:
            return 1;
        case run::RunOutcome::Failed:
            return 2;
        case run::RunOutcome::Cancelled:
            return 130;
    }
    return 2;
}

int run(run::OptimizationRunService& service, run::RunRequest request,
        std::shared_ptr<std::ostream> output, const std::function<bool()>& interrupted) {
    auto started = service.start(
        std::move(request), [output](const run::RunEvent& event) { renderEvent(*output, event); });
    if (!started.started()) {
        *output << "Start Error: " << static_cast<int>(*started.startError()) << std::endl;
        return 2;
    }
    auto handle = std::move(*started.handle());
    while (!handle.terminalResult()) {
        if (interrupted()) handle.requestCancellation();
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }
    return exitCode(handle.wait().outcome());
}
ConsoleInterrupt::ConsoleInterrupt() {
#ifdef _WIN32
    interruptRequested.store(false, std::memory_order_relaxed);
    if (!SetConsoleCtrlHandler(consoleControl, TRUE))
        throw std::runtime_error("Could not install cooperative console interrupt handling");
#else
    interruptRequested = 0;
    _previous = std::signal(SIGINT, consoleSignal);
    if (_previous == SIG_ERR)
        throw std::runtime_error("Could not install cooperative console interrupt handling");
#endif
}
ConsoleInterrupt::~ConsoleInterrupt() {
#ifdef _WIN32
    SetConsoleCtrlHandler(consoleControl, FALSE);
#else
    std::signal(SIGINT, _previous);
#endif
}
bool ConsoleInterrupt::requested() const noexcept {
#ifdef _WIN32
    return interruptRequested.load(std::memory_order_relaxed);
#else
    return interruptRequested != 0;
#endif
}
}  // namespace cao::cli
