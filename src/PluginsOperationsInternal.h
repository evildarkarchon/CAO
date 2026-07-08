#pragma once

#include <QString>

#include <cstddef>
#include <cstdint>
#include <istream>
#include <string>

namespace PluginsOperationsDetail {
/*!
 * \brief Reads one length-prefixed plugin field into a QString without relying
 * on fixed buffers or trailing NUL bytes.
 * \param stream Source stream positioned at the field payload.
 * \param dataSize Declared payload length to read, in bytes.
 * \return The UTF-8 payload up to the first embedded NUL byte; short reads
 * return the bytes that were actually available.
 */
inline QString readPluginString(std::istream &stream, const uint16_t dataSize) {
  if (dataSize == 0)
    return {};

  std::string fieldData(dataSize, '\0');
  stream.read(fieldData.data(), static_cast<std::streamsize>(fieldData.size()));
  fieldData.resize(static_cast<std::size_t>(stream.gcount()));

  const auto nulPosition = fieldData.find('\0');
  if (nulPosition != std::string::npos)
    fieldData.resize(nulPosition);

  return QString::fromUtf8(fieldData.data(),
                           static_cast<int>(fieldData.size()));
}
} // namespace PluginsOperationsDetail
