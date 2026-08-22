#include "MeshReferenceMaintenance.h"

#include <nifly/BasicTypes.hpp>
#include <nifly/NifFile.hpp>

#include <QTest>

#include <string>
#include <vector>

class MeshReferenceMaintenanceTests final : public QObject
{
    Q_OBJECT

private slots:
    /// Verifies referenced TGA Texture names are detected without changing Mesh content.
    void detectsReferencedTgaWithoutMutation();

    /// Verifies maintenance replaces referenced TGA Texture names and leaves unrelated names intact.
    void replacesReferencedTgaTextureNames();
};

namespace
{
/// Creates a minimal in-memory SSE Mesh containing one referenced Texture name.
nifly::NifFile meshWithTexture(const std::string &texturePath)
{
    nifly::NifFile mesh;
    mesh.Create(nifly::NiVersion::getSSE());

    std::vector<nifly::Vector3> vertices{{0.0F, 0.0F, 0.0F},
                                         {1.0F, 0.0F, 0.0F},
                                         {0.0F, 1.0F, 0.0F}};
    std::vector<nifly::Triangle> triangles{{0, 1, 2}};
    auto *shape = mesh.CreateShapeFromData("TestShape", &vertices, &triangles, nullptr);
    auto mutableTexturePath = texturePath;
    mesh.SetTextureSlot(shape, mutableTexturePath);
    return mesh;
}

/// Reads the first Texture slot from the minimal test Mesh.
std::string textureAt(const nifly::NifFile &mesh)
{
    const auto shapes = mesh.GetShapes();
    std::string texturePath;
    mesh.GetTextureSlot(shapes.front(), texturePath);
    return texturePath;
}
}

void MeshReferenceMaintenanceTests::detectsReferencedTgaWithoutMutation()
{
    const auto mesh = meshWithTexture("textures\\armor\\body.TgA");
    const auto originalTexture = textureAt(mesh);

    QVERIFY(cao::execution::hasReferencedTgaTexture(mesh));
    QCOMPARE(textureAt(mesh), originalTexture);
}

void MeshReferenceMaintenanceTests::replacesReferencedTgaTextureNames()
{
    auto convertibleMesh = meshWithTexture("textures\\armor\\body.TgA");
    auto nativeMesh = meshWithTexture("textures\\armor\\body.dds");

    QVERIFY(cao::execution::replaceReferencedTgaTextureNames(convertibleMesh));
    QCOMPARE(textureAt(convertibleMesh), std::string("textures\\armor\\body.dds"));
    QVERIFY(!cao::execution::replaceReferencedTgaTextureNames(nativeMesh));
    QCOMPARE(textureAt(nativeMesh), std::string("textures\\armor\\body.dds"));
}

QTEST_APPLESS_MAIN(MeshReferenceMaintenanceTests)

#include "MeshReferenceMaintenanceTests.moc"
