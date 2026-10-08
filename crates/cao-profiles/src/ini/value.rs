//! INI values and the `QVariant` conversions CAO's C++ code relies on.

/// One INI value, as QSettings holds it after reading a file.
///
/// Numbers and bools are not typed: QSettings reads every scalar as a string and
/// converts it when asked, so they are [`Value::String`]. The getters copy
/// `QVariant`'s conversions, including its leniency: text that does not parse reads
/// as `0` or `false` rather than as an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// `@Invalid()`, an invalid `QVariant`. A missing key reads as this too, and Qt
    /// writes an empty list this way.
    Invalid,
    /// A string (`QString`). Qt reads `true`, `3` and `2104533975.04` as strings.
    String(String),
    /// A list split on unquoted commas (`QStringList`, or a `QVariantList` when an
    /// element is `@`-encoded). Elements are never lists themselves.
    List(Vec<Value>),
    /// Any other `@…)` encoding (`@Variant(…)`, `@String(…)`, `@ByteArray(…)`, …),
    /// kept as Qt's unescaped text so it is written back verbatim. Qt writes a
    /// one-element list as `@Variant(…)`; [`Value::to_int_list`] decodes that shape.
    Encoded(String),
}

/// What a missing key reads as.
pub(crate) static INVALID: Value = Value::Invalid;

/// `QDataStream` type ids (Qt_4_0) that a one-element list can hold.
const TYPE_INT: u32 = 2;
const TYPE_UINT: u32 = 3;
const TYPE_LONGLONG: u32 = 4;
const TYPE_ULONGLONG: u32 = 5;
const TYPE_VARIANT_LIST: u32 = 9;
const TYPE_STRING: u32 = 10;

impl Value {
    /// The value Qt's `setValue` stores for a list of integers.
    ///
    /// Qt writes an empty list as `@Invalid()` and a one-element `QVariantList` as a
    /// `@Variant(…)` QDataStream blob holding one `Int`; longer lists are plain
    /// `a, b, c`. The C++ build reads all three back, so a list written here survives
    /// a round trip through either build.
    pub fn int_list(values: &[i32]) -> Self {
        match values {
            [] => Self::Invalid,
            [value] => {
                let mut blob = Vec::with_capacity(16);
                for word in [TYPE_VARIANT_LIST, 1, TYPE_INT] {
                    blob.extend_from_slice(&word.to_be_bytes());
                }
                blob.extend_from_slice(&value.to_be_bytes());
                // The blob is a Latin-1 string to Qt: one char per byte.
                let text: String = blob.iter().map(|&byte| char::from(byte)).collect();
                Self::Encoded(format!("@Variant({text})"))
            }
            _ => Self::List(
                values
                    .iter()
                    .map(|value| Self::String(value.to_string()))
                    .collect(),
            ),
        }
    }

    /// `QVariant::toString()`: a string as is, anything else as `""`.
    ///
    /// `@String(…)` is a string too. Lists read from a file always have two or more
    /// elements, which Qt converts to `""`.
    pub fn to_qstring(&self) -> String {
        self.as_str().unwrap_or_default().to_owned()
    }

    /// `QVariant::toBool()`: a string is false only when it is empty, `0` or `false`
    /// in any case, so `no` and `1` are true. Anything that is not a string is false.
    pub fn to_bool(&self) -> bool {
        self.as_str().is_some_and(|text| {
            let lower = text.to_lowercase();
            !(lower.is_empty() || lower == "0" || lower == "false")
        })
    }

    /// `QVariant::toLongLong()`: a base-10 integer with surrounding whitespace allowed,
    /// or `0` when the text does not parse.
    pub fn to_i64(&self) -> i64 {
        self.as_str().map_or(0, parse_i64)
    }

    /// `QVariant::toInt()`: [`Value::to_i64`] truncated to 32 bits, as Qt's
    /// `int(qlonglong)` cast does, so `4294967297` reads as `1`.
    pub fn to_i32(&self) -> i32 {
        self.to_i64() as i32
    }

    /// `QVariant::toUInt()`: an unsigned 64-bit parse truncated to 32 bits. A negative
    /// number does not parse, so it reads as `0`.
    pub fn to_u32(&self) -> u32 {
        self.as_str()
            .map_or(0, |text| text.trim().parse::<u64>().unwrap_or(0) as u32)
    }

    /// `QVariant::toDouble()`: the C-locale number with surrounding whitespace allowed,
    /// or `0.0` when the text does not parse.
    pub fn to_f64(&self) -> f64 {
        self.as_str()
            .map_or(0.0, |text| text.trim().parse().unwrap_or(0.0))
    }

    /// `QVariant::toList()` with each element converted by `toInt()`, which is how CAO
    /// reads `texturesUnwantedFormats`.
    ///
    /// A comma list converts each element; `@Invalid()` and a missing key give `[]`;
    /// a `@Variant(…)` `QVariantList` of integers or strings is decoded, and one Qt
    /// cannot finish reading gives `[]` as it does in Qt.
    ///
    /// **Deviation 12:** a plain scalar such as `texturesUnwantedFormats=85` reads as
    /// `[85]`, where Qt reads `[]`. An empty scalar still reads as `[]`.
    pub fn to_int_list(&self) -> Vec<i32> {
        match self {
            Self::Invalid => Vec::new(),
            Self::List(elements) => elements.iter().map(Self::to_i32).collect(),
            Self::Encoded(raw) if raw.starts_with("@Variant(") => {
                decode_variant_list(&raw["@Variant(".len()..]).unwrap_or_default()
            }
            _ => match self.as_str() {
                Some(text) if !text.trim().is_empty() => vec![self.to_i32()],
                _ => Vec::new(),
            },
        }
    }

    /// The text Qt converts from: a plain string, or the inside of `@String(…)`.
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            Self::Encoded(raw) => raw.strip_prefix("@String(")?.strip_suffix(')'),
            Self::Invalid | Self::List(_) => None,
        }
    }
}

/// See [`Value::to_i64`].
fn parse_i64(text: &str) -> i64 {
    text.trim().parse().unwrap_or(0)
}

/// Decodes the payload of `@Variant(…)` as a `QVariantList` of integers, the shape
/// Qt writes for a one-element list. `payload` is Qt's unescaped text, one Latin-1
/// char per byte; the trailing `)` is ignored because the stream stops before it.
///
/// Returns `None` when the payload is not a `QVariantList` at all. A list whose
/// elements Qt would fail to read decodes to `[]`, because Qt's QDataStream clears
/// a container on a short read. Element types CAO never writes (anything but the
/// integer types and `QString`) also give `[]`.
fn decode_variant_list(payload: &str) -> Option<Vec<i32>> {
    // `QString::toLatin1` turns anything above U+00FF into `?`.
    let bytes: Vec<u8> = payload
        .encode_utf16()
        .map(|unit| u8::try_from(unit).unwrap_or(b'?'))
        .collect();
    let mut stream = Stream(&bytes);
    if stream.u32()? != TYPE_VARIANT_LIST {
        return None;
    }
    let mut read_elements = || -> Option<Vec<i32>> {
        let count = stream.u32()?;
        let mut elements = Vec::new();
        for _ in 0..count {
            // Qt_4_0 streams carry no null flag after the type id.
            let element = match stream.u32()? {
                TYPE_INT => stream.u32()? as i32,
                TYPE_UINT => stream.u32()? as i32,
                TYPE_LONGLONG | TYPE_ULONGLONG => stream.u64()? as i32,
                TYPE_STRING => parse_i64(&stream.qstring()?) as i32,
                _ => return None,
            };
            elements.push(element);
        }
        Some(elements)
    };
    Some(read_elements().unwrap_or_default())
}

/// A big-endian QDataStream reader over a byte slice.
struct Stream<'a>(&'a [u8]);

impl Stream<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (head, rest) = self.0.split_first_chunk::<N>()?;
        self.0 = rest;
        Some(*head)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_be_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.take().map(u64::from_be_bytes)
    }

    /// A `QString`: a byte length (`0xFFFFFFFF` for a null string), then UTF-16BE.
    fn qstring(&mut self) -> Option<String> {
        let length = self.u32()?;
        if length == u32::MAX {
            return Some(String::new());
        }
        let length = usize::try_from(length).ok()?;
        if length % 2 != 0 || length > self.0.len() {
            return None;
        }
        let (text, rest) = self.0.split_at(length);
        self.0 = rest;
        let (pairs, _) = text.as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|&pair| u16::from_be_bytes(pair)).collect();
        Some(String::from_utf16_lossy(&units))
    }
}

impl From<&str> for Value {
    fn from(text: &str) -> Self {
        Self::String(text.to_owned())
    }
}

impl From<String> for Value {
    fn from(text: String) -> Self {
        Self::String(text)
    }
}

impl From<bool> for Value {
    /// Qt writes bools as `true` and `false`.
    fn from(value: bool) -> Self {
        Self::String(if value { "true" } else { "false" }.to_owned())
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Self::String(value.to_string())
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Self::String(value.to_string())
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::String(value.to_string())
    }
}

impl From<f64> for Value {
    /// Qt writes doubles with `QString::number(d, 'g', QLocale::FloatingPointShortest)`.
    fn from(value: f64) -> Self {
        Self::String(format_double(value))
    }
}

/// `QString::number(d, 'g', QLocale::FloatingPointShortest)`, which `QVariant`
/// uses to write a double: the shortest digits that read back exactly, in fixed
/// notation unless the exponent form is shorter (`2e+09`, `1e-05`).
///
/// Port of the `DFSignificantDigits` branch of `QLocaleData::doubleToString` with
/// `decimalForm` and `exponentForm` (Qt 5.15 `qlocale.cpp`, `qlocale_tools.cpp`).
fn format_double(d: f64) -> String {
    if d.is_nan() {
        return "nan".to_owned();
    }
    if d.is_infinite() {
        return if d < 0.0 { "-inf" } else { "inf" }.to_owned();
    }

    // Rust's `{:e}` prints the same shortest round-trip digits Qt's double-conversion
    // does, as `d.ddde±x`. `decpt` is Qt's decimal-point position.
    let scientific = format!("{:e}", d.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("`{:e}` has an exponent");
    let mut digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let decpt = exponent
        .parse::<i32>()
        .expect("`{:e}` exponent is an integer")
        + 1;
    let length = digits.len() as i32;

    // Qt uses the exponent form when the fixed form would be longer.
    let mut cutoff = 6;
    if decpt > 0 {
        // 'e', the sign, and a two-digit exponent (three past e+100).
        cutoff = length + 4 + if decpt > 100 { 2 } else { 1 };
        if length > decpt {
            cutoff += 1;
        }
    }

    let text = if decpt != length && (decpt <= -4 || decpt > cutoff) {
        // exponentForm, with the exponent zero-padded to two digits as printf does.
        if digits.len() > 1 {
            digits.insert(1, '.');
        }
        let exponent = decpt - 1;
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{digits}e{sign}{:02}", exponent.abs())
    } else {
        // decimalForm, chopping trailing zeros.
        let mut decpt = decpt;
        if decpt < 0 {
            digits.insert_str(0, &"0".repeat(decpt.unsigned_abs() as usize));
            decpt = 0;
        } else if decpt > length {
            digits.push_str(&"0".repeat((decpt - length) as usize));
        }
        if (decpt as usize) < digits.len() {
            digits.insert(decpt as usize, '.');
        }
        if decpt == 0 {
            digits.insert(0, '0');
        }
        digits
    };

    // Qt never writes `-0`.
    if d < 0.0 { format!("-{text}") } else { text }
}
