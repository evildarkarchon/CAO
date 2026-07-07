#include "AssetWorkPlanExecutor.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFile>
#include <QTemporaryDir>

#include <vector>

namespace
{
void createFile(const QString &path)
{
    QFile file(path);
    REQUIRE(file.open(QIODevice::WriteOnly));
}

AssetWorkPlanRequest defaultRequest(const QString &selectedPath)
{
    const auto profile = ProfilePlanningSnapshot{true, true, true, true, true, ".bsa"};
    const auto requested = RequestedAssetWork{true, true, true, true, true};
    return AssetWorkPlanRequest{selectedPath,
                                AssetWorkMode::SeveralMods,
                                {},
                                AssetWorkPolicy::resolve(requested, profile)};
}

struct RecordedWorkAdapter final : AssetWorkPlanExecutionAdapter
{
    QStringList events;
    std::function<void(const ArchiveExtractionWorkItem &workItem)> onExtractArchive;

    void extractArchive(const ArchiveExtractionWorkItem &workItem) override
    {
        events << "extract:" + workItem.path;
        if (onExtractArchive)
            onExtractArchive(workItem);
    }

    void processLooseAsset(const LooseAssetWorkItem &workItem) override
    {
        events << "process:" + workItem.path;
    }

    void packArchive(const ArchivePackingWorkItem &workItem) override
    {
        events << "pack:" + workItem.folder;
    }
};
}

TEST_CASE("AssetWorkPlanExecutor runs extraction before post-extraction Loose Asset Discovery and packing")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha/meshes"));
    REQUIRE(root.mkpath("Alpha/textures"));
    REQUIRE(root.mkpath("Alpha/empty/nested"));

    createFile(root.filePath("Alpha/Alpha.bsa"));
    createFile(root.filePath("Alpha/meshes/body.nif"));

    RecordedWorkAdapter adapter;
    adapter.onExtractArchive = [&](const ArchiveExtractionWorkItem &) {
        createFile(root.filePath("Alpha/textures/extracted.dds"));
    };

    AssetWorkPlanExecutor executor(defaultRequest(tempDir.path()), adapter);
    const auto result = executor.execute();

    const QString extractEvent = "extract:" + root.filePath("Alpha/Alpha.bsa");
    const QString existingAssetEvent = "process:" + root.filePath("Alpha/meshes/body.nif");
    const QString extractedAssetEvent = "process:" + root.filePath("Alpha/textures/extracted.dds");
    const QString packEvent = "pack:" + root.filePath("Alpha");

    REQUIRE(result == AssetWorkPlanExecutionResult::Completed);
    REQUIRE(adapter.events.first() == extractEvent);
    REQUIRE(adapter.events.contains(existingAssetEvent));
    REQUIRE(adapter.events.contains(extractedAssetEvent));
    REQUIRE(adapter.events.last() == packEvent);
    REQUIRE(adapter.events.indexOf(extractedAssetEvent) > adapter.events.indexOf(extractEvent));
    REQUIRE_FALSE(root.exists("Alpha/empty"));
}

TEST_CASE("AssetWorkPlanExecutor reports semantic progress")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha"));
    createFile(root.filePath("Alpha/Alpha.bsa"));

    RecordedWorkAdapter adapter;
    std::vector<AssetWorkPlanProgress> progress;

    AssetWorkPlanExecutor executor(defaultRequest(tempDir.path()), adapter);
    const auto result = executor.execute(AssetWorkPlanExecutionCallbacks{
        [&](const AssetWorkPlanProgress &entry) { progress.push_back(entry); },
        {}});

    REQUIRE(result == AssetWorkPlanExecutionResult::Completed);
    REQUIRE(progress.size() == 4);
    REQUIRE(progress[0].phase == AssetWorkPlanExecutionPhase::ArchiveExtraction);
    REQUIRE(progress[0].completed == 1);
    REQUIRE(progress[0].total == 1);
    REQUIRE(progress[1].phase == AssetWorkPlanExecutionPhase::LooseAssetProcessing);
    REQUIRE(progress[1].completed == 0);
    REQUIRE(progress[1].total == 0);
    REQUIRE(progress[2].phase == AssetWorkPlanExecutionPhase::ArchivePacking);
    REQUIRE(progress[2].completed == 0);
    REQUIRE(progress[2].total == 1);
    REQUIRE(progress[3].phase == AssetWorkPlanExecutionPhase::ArchivePacking);
    REQUIRE(progress[3].completed == 1);
    REQUIRE(progress[3].total == 1);
    REQUIRE(progress[3].currentLabel == "Alpha");
}

TEST_CASE("AssetWorkPlanExecutor stops before Loose Asset Discovery when cancellation follows extraction")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha/empty/nested"));
    createFile(root.filePath("Alpha/Alpha.bsa"));

    RecordedWorkAdapter adapter;
    const QString extractEvent = "extract:" + root.filePath("Alpha/Alpha.bsa");

    AssetWorkPlanExecutor executor(defaultRequest(tempDir.path()), adapter);
    const auto result = executor.execute(AssetWorkPlanExecutionCallbacks{
        {},
        [&]() { return adapter.events.contains(extractEvent); }});

    REQUIRE(result == AssetWorkPlanExecutionResult::Cancelled);
    REQUIRE(adapter.events == QStringList{extractEvent});
    REQUIRE(root.exists("Alpha/empty"));
}
