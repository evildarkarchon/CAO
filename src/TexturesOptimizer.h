/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkExecutionPolicy.h"
#include "FilesystemOperations.h"
#include "PluginsOperations.h"
#include "TextureAssetTransaction.h"
#include "pch.h"

class TexturesOptimizer final : public QObject, public TextureTransformEngine {
  Q_DECLARE_TR_FUNCTIONS(TexturesOptimizer)

public:
  /*!
   * \brief Creates a texture optimizer from resolved texture execution
   * rules.
   * \param policy The texture execution rules and target Profile
   * format.
   */
  explicit TexturesOptimizer(TextureExecutionPolicy policy);
  /*! \brief Releases COM ownership on the worker that created this engine. */
  ~TexturesOptimizer() override;

  /*!
   * \brief Completes read-only texture inspection and in-memory
   * transformation.
   * \param request The source texture kind and Dry Run
   * contract.
   * \return Encoded, validated DDS bytes or a classified
   * failure.
   */
  [[nodiscard]] TextureTransformResult
  transform(const TextureTransformRequest &request) override;

private:
  enum TextureType { DDS, TGA };

  void listLandscapeTextures(QDirIterator &it);

  bool open(const void *pSource, const size_t &size, const TextureType &type,
            const QString &fileName);
  bool open(const QString &filePath, const TextureType &type);

  bool saveToFile(const QString &filePath) const;
  /*!
   * \brief Decompress the current texture. It is required to use several
   * functions.
   * \return False if an error happens
   */
  bool decompress();
  /*!
   * \brief Convenience function, that will use appropriately
   * convertWithCompression or convertWithoutCompression
   * \param format The format to use
   * \return False in case of error
   */
  bool convert(const DXGI_FORMAT &format);
  /*!
   * \brief Compress the file using the provided compression format,
   * \param format The format to use
   */
  bool convertWithCompression(const DXGI_FORMAT &format);
  bool convertWithoutCompression(const DXGI_FORMAT &format);
  /*!
   * \brief Check if a texture is compressed
   * \return True if the file is compressed
   */
  bool isCompressed() const;
  [[nodiscard]] bool canBeCompressed() const;
  /*!
   * \brief Perform various optimizations on the current texture
   * \param tWidth Optional resolved width target for the current texture.
   * \param tHeight Optional resolved height target for the current texture.
   *
   * \return False if an error happens
   */
  bool optimize(const std::optional<size_t> &tWidth,
                const std::optional<size_t> &tHeight);

  /*!
   * \brief Reports the texture operations that would run for the current
   * texture.
   * \param tWidth Optional resolved width target for the current texture.
   * \param tHeight Optional resolved height target for the current texture.
   */
  void dryOptimize(const std::optional<size_t> &tWidth,
                   const std::optional<size_t> &tHeight);

  bool resize(size_t targetWidth, size_t targetHeight);

  static void fitPowerOfTwo(size_t &resultX, size_t &resultY);
  bool isPowerOfTwo() const;

  bool generateMipMaps();
  bool canHaveMipMaps();
  size_t calculateOptimalMipMapsNumber() const;

  bool convertLandscapeTextures();

  DirectX::TexMetadata getInfo() const;

  bool isIncompatible() const;

  static bool compareInfo(const DirectX::TexMetadata &info1,
                          const DirectX::TexMetadata &info2);

  struct TexOptOptionsResult {
    bool bNeedsResize;
    bool bNeedsCompress;
    bool bNeedsMipmaps;
    size_t tWidth;
    size_t tHeight;
  };

  TexOptOptionsResult processArguments(const bool &bNecessary,
                                       const bool &bCompress,
                                       const bool &bMipmaps,
                                       const std::optional<size_t> &tWidth,
                                       const std::optional<size_t> &tHeight);

  bool modifiedCurrentTexture = false;

  TextureExecutionPolicy _policy;
  std::unique_ptr<DirectX::ScratchImage> _image{};
  DirectX::TexMetadata _info{};
  QString _name;
  TextureType _type;

  Microsoft::WRL::ComPtr<ID3D11Device> _pDevice;
  bool _comInitialized = false;

  bool createDevice(int adapter, ID3D11Device **pDevice) const;
  bool getDXGIFactory(IDXGIFactory1 **pFactory) const;

  QStringList _landscapeTextures;
};
