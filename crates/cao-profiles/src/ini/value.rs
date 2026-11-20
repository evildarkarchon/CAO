//! INI values and the `QVariant` conversions that read them as QSettings did.

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
    /// kept as Qt's unescaped text so it is written back verbatim. Qt wrote a
    /// one-element list as `@Variant(…)`; [`Value::list_entries`] decodes that shape.
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
    /// The value to store for a list of integers: `@Invalid()` for an empty list,
    /// as Qt writes one, and otherwise the plain `a, b, c`.
    ///
    /// A one-element list is written as the plain number, which the reader takes
    /// as a one-element list (deviation 12). Qt wrote it as an opaque
    /// `@Variant(…)` blob instead, which only a QSettings reader understands;
    /// that shape is still read, from files Qt saved, but never written.
    pub fn int_list(values: &[i32]) -> Self {
        match values {
            [] => Self::Invalid,
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

    /// Each entry of a list value, unconverted, so a strict reader can check
    /// every entry at full width and quote it as written.
    ///
    /// A comma list gives its elements, a plain scalar gives itself (deviation
    /// 12; Qt reads `[]`), and `@Invalid()`, a missing key or an empty scalar
    /// give `[]`. A `@Variant(…)` list, the shape Qt wrote for a one-element
    /// list, gives its entries, integers as their decimal text, untruncated.
    ///
    /// Returns `None` where Qt would read `[]` from something that is not
    /// empty: a `@Variant(…)` that is not a `QVariantList` of
    /// integers or strings, or is cut short, and any other `@…)` encoding but
    /// `@String(…)`. Reading those as empty would let the next save overwrite
    /// the user's value with nothing.
    pub fn list_entries(&self) -> Option<Vec<Value>> {
        match self {
            Self::Invalid => Some(Vec::new()),
            Self::List(elements) => Some(elements.clone()),
            Self::Encoded(raw) if raw.starts_with("@Variant(") => {
                decode_variant_list(&raw["@Variant(".len()..])
            }
            _ => match self.as_str() {
                Some(text) if text.trim().is_empty() => Some(Vec::new()),
                Some(_) => Some(vec![self.clone()]),
                None => None,
            },
        }
    }

    /// The text after `key=` that a file holds for this value, escaped as CAO
    /// writes it, such as `85, 86` or `@Variant(\0\0\0\t…)`. A message quoting
    /// a rejected value uses it, since [`Value::to_qstring`] gives `""` for a
    /// list or an encoded value.
    pub fn to_written(&self) -> String {
        super::write::value_text(self)
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
/// Each element is a [`Value::String`]: a `QString` as is, an integer as its
/// decimal text at full width, so the caller picks how to convert it.
///
/// Returns `None` when the payload is not a `QVariantList`, when Qt would fail to
/// read its elements (Qt's QDataStream then clears the container, so Qt itself
/// reads `[]`), or when an element has a type CAO never writes (anything but the
/// integer types and `QString`). [`Value::list_entries`] reports all of these.
fn decode_variant_list(payload: &str) -> Option<Vec<Value>> {
    // `QString::toLatin1` turns anything above U+00FF into `?`.
    let bytes: Vec<u8> = payload
        .encode_utf16()
        .map(|unit| u8::try_from(unit).unwrap_or(b'?'))
        .collect();
    let mut stream = Stream(&bytes);
    if stream.u32()? != TYPE_VARIANT_LIST {
        return None;
    }
    let count = stream.u32()?;
    let mut elements = Vec::new();
    for _ in 0..count {
        // Qt_4_0 streams carry no null flag after the type id.
        let element = match stream.u32()? {
            // The bits as Qt's `QVariant::toInt()` would see them: `Int` signed,
            // `UInt` unsigned, and the 64-bit types at their own signedness.
            TYPE_INT => (stream.u32()? as i32).to_string(),
            TYPE_UINT => stream.u32()?.to_string(),
            TYPE_LONGLONG => (stream.u64()? as i64).to_string(),
            TYPE_ULONGLONG => stream.u64()?.to_string(),
            TYPE_STRING => stream.qstring()?,
            _ => return None,
        };
        elements.push(Value::String(element));
    }
    Some(elements)
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
