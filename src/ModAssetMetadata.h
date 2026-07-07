/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include <QSet>
#include <QString>
#include <QStringList>

class ProfileAssetReferenceProvider {
public:
  virtual ~ProfileAssetReferenceProvider() = default;

  /*!
   * \brief Reads a Profile-provided Asset reference list.
   * \param fileName The Profile file name to resolve, including fallback
   * behavior.
   * \return Non-comment, non-empty entries from the resolved file, or an empty
   * list when it is unavailable.
   */
  [[nodiscard]] virtual QStringList
  readReferenceList(const QString &fileName) const = 0;
};

class PluginAssetReferenceReader {
public:
  virtual ~PluginAssetReferenceReader() = default;

  /*!
   * \brief Reads Headpart mesh references from one plugin.
   * \param pluginPath The plugin file to inspect.
   * \return Headpart mesh paths referenced by the plugin.
   */
  [[nodiscard]] virtual QStringList
  listHeadparts(const QString &pluginPath) const = 0;
};

class ModAssetMetadata final {
public:
  /*!
   * \brief Creates empty Mod Asset Metadata.
   */
  ModAssetMetadata() = default;

  /*!
   * \brief Creates Mod Asset Metadata from Headpart mesh paths.
   * \param headpartMeshPaths Profile-provided and plugin-derived Headpart mesh
   * paths.
   */
  explicit ModAssetMetadata(const QStringList &headpartMeshPaths);

  /*!
   * \brief Checks whether an Asset path identifies a Headpart mesh.
   * \param assetPath Absolute or relative mesh Asset path.
   * \return True when the path matches Profile/plugin Headpart metadata or the
   * FaceGen convention.
   */
  [[nodiscard]] bool isHeadpartMesh(const QString &assetPath) const;

private:
  QSet<QString> _headpartMeshPaths;
};

class ModAssetMetadataProvider {
public:
  virtual ~ModAssetMetadataProvider() = default;

  /*!
   * \brief Builds Mod Asset Metadata for selected Mods.
   * \param selectedMods Mods selected by the Asset Work Plan.
   * \return Metadata derived only from the selected Mods and active Profile
   * reference lists.
   */
  [[nodiscard]] virtual ModAssetMetadata
  buildForMods(const QStringList &selectedMods) const = 0;
};

class ModAssetMetadataBuilder final : public ModAssetMetadataProvider {
public:
  /*!
   * \brief Creates a builder from the adapters needed to read Profile and
   * plugin references.
   * \param profileReferences Adapter for Profile-provided reference lists.
   * \param pluginReferences Adapter for plugin-derived Asset references.
   */
  ModAssetMetadataBuilder(
      const ProfileAssetReferenceProvider &profileReferences,
      const PluginAssetReferenceReader &pluginReferences);

  /*!
   * \brief Builds Mod Asset Metadata for selected Mods.
   * \param selectedMods Mods selected by the Asset Work Plan.
   * \return Metadata combining Profile Headpart references and plugin-derived
   * Headpart references.
   */
  [[nodiscard]] ModAssetMetadata
  buildForMods(const QStringList &selectedMods) const override;

private:
  const ProfileAssetReferenceProvider &_profileReferences;
  const PluginAssetReferenceReader &_pluginReferences;
};

class ProfileFileAssetReferenceProvider final
    : public ProfileAssetReferenceProvider {
public:
  /*!
   * \brief Reads reference lists through the active Profile file lookup rules.
   * \param fileName The Profile file name to read.
   * \return Non-comment, non-empty entries from the current Profile or its
   * fallback Profile.
   */
  [[nodiscard]] QStringList
  readReferenceList(const QString &fileName) const override;
};

class PluginOperationsAssetReferenceReader final
    : public PluginAssetReferenceReader {
public:
  /*!
   * \brief Reads plugin references through the production plugin parser.
   * \param pluginPath The plugin file to inspect.
   * \return Headpart mesh paths referenced by the plugin.
   */
  [[nodiscard]] QStringList
  listHeadparts(const QString &pluginPath) const override;
};
