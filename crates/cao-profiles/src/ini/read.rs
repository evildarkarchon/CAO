//! The reading half of Qt 5.15's QSettings IniFormat (`qsettings.cpp`).
//!
//! Each function ports the Qt routine it names, over bytes, so that hand-edited files
//! read the way Qt reads them. The recorded deviations (#476, items 13 and 14) are
//! marked where they enter.

use super::value::Value;
use super::{FormatError, FormatErrorKind, IniFile};

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

/// One logical line found by [`read_line`]: `data[start..end]`, with the first
/// unquoted `=` at `equals`.
struct Line {
    start: usize,
    end: usize,
    equals: Option<usize>,
}

/// Parses a whole file. Port of `readIniFile` and `readIniSection`, done in one pass:
/// Qt splits the file into sections first and parses each lazily, which gives the
/// same keys in the same order.
pub(super) fn parse(data: &[u8]) -> IniFile {
    // Qt reads Latin-1 unless the file starts with a UTF-8 BOM. Qt 5.15 also leaves
    // the BOM bytes in its root section, which then reads with FormatError and can
    // mangle the first key; skipping the BOM avoids both.
    // Deviation 14: a file without a BOM is UTF-8 when the whole file is valid UTF-8.
    let (mut pos, utf8) = match data.strip_prefix(UTF8_BOM) {
        Some(_) => (UTF8_BOM.len(), true),
        None => (0, std::str::from_utf8(data).is_ok()),
    };

    let mut ini = IniFile::new();
    // The current section as a key prefix: "" for [General], else "Name/".
    let mut prefix = String::new();
    // Qt merges sections that differ only in case under the first spelling it saw.
    let mut section_spellings: Vec<String> = Vec::new();

    while let Some(line) = read_line(data, &mut pos) {
        let text = &data[line.start..line.end];
        if text[0] == b'[' {
            let name = match text.iter().position(|&byte| byte == b']') {
                Some(close) => &text[1..close],
                None => {
                    ini.record_error(data, line.start, FormatErrorKind::UnclosedSection);
                    &text[1..]
                }
            };
            let name = name.trim_ascii();
            prefix = if name.eq_ignore_ascii_case(b"general") {
                String::new()
            } else if name.eq_ignore_ascii_case(b"%general") {
                // `[%General]` is a real group named General, spelled as written.
                format!("{}/", latin1(&name[1..]))
            } else {
                format!("{}/", unescape_key(name))
            };
            match section_spellings
                .iter()
                .find(|seen| seen.to_lowercase() == prefix.to_lowercase())
            {
                Some(seen) => prefix.clone_from(seen),
                None => section_spellings.push(prefix.clone()),
            }
            continue;
        }

        let Some(equals) = line.equals else {
            if text[0] != b';' {
                ini.record_error(data, line.start, FormatErrorKind::MissingEquals);
            }
            continue;
        };
        let mut key_end = equals;
        while key_end > line.start && matches!(data[key_end - 1], b' ' | b'\t') {
            key_end -= 1;
        }
        let key = format!("{prefix}{}", unescape_key(&data[line.start..key_end]));
        let value = match unescape_value(&data[equals + 1..line.end], utf8) {
            Unescaped::Single(units) => string_to_value(String::from_utf16_lossy(&units)),
            Unescaped::List(elements) => string_list_to_value(
                elements
                    .iter()
                    .map(|units| String::from_utf16_lossy(units))
                    .collect(),
            ),
        };
        ini.insert(key, value);
    }
    ini
}

/// `charTraits` Space: tab, LF, CR and space.
fn is_space(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | b'\r' | b' ')
}

/// `charTraits` Special: the bytes [`read_line`] stops on.
fn is_special(byte: u8) -> bool {
    matches!(byte, b'\n' | b'\r' | b'"' | b';' | b'=' | b'\\')
}

/// Skips a comment from `*i` to the end of its line, then any whitespace after it.
fn skip_comment(data: &[u8], i: &mut usize) {
    while *i < data.len() && data[*i] != b'\n' && data[*i] != b'\r' {
        *i += 1;
    }
    while *i < data.len() && is_space(data[*i]) {
        *i += 1;
    }
}

/// Port of `readIniLine`: finds the next logical line from `*pos` and moves `*pos`
/// past it. Returns `None` at the end of the data.
///
/// A line ends at CR or LF outside double quotes, so a quoted value can span physical
/// lines, and `\` escapes the next byte (backslash-newline continues a line). A `;`
/// starts a comment at the start of a line and ends the line anywhere else outside
/// quotes.
fn read_line(data: &[u8], pos: &mut usize) -> Option<Line> {
    let len = data.len();
    let mut in_quotes = false;
    let mut equals = None;

    let mut start = *pos;
    while start < len && is_space(data[start]) {
        start += 1;
    }

    let mut i = start;
    'line: while i < len {
        // Deviation 13: a `#` at the start of a line is a comment, like `;`. Handling
        // it here, before quotes and escapes, keeps a `"` or `;` in it from leaking.
        if i == start && data[i] == b'#' {
            skip_comment(data, &mut i);
            start = i;
            continue;
        }

        let mut ch = data[i];
        while !is_special(ch) {
            i += 1;
            if i == len {
                break 'line;
            }
            ch = data[i];
        }

        i += 1;
        match ch {
            b'=' => {
                if !in_quotes && equals.is_none() {
                    equals = Some(i - 1);
                }
            }
            b'\n' | b'\r' => {
                if i == start + 1 {
                    start += 1;
                } else if !in_quotes {
                    i -= 1;
                    break 'line;
                }
            }
            b'\\' => {
                if i < len {
                    let escaped = data[i];
                    i += 1;
                    // CRLF and LFCR count as one line terminator.
                    if i < len && matches!((escaped, data[i]), (b'\n', b'\r') | (b'\r', b'\n')) {
                        i += 1;
                    }
                }
            }
            b'"' => in_quotes = !in_quotes,
            _ => {
                // `;`
                if i == start + 1 {
                    skip_comment(data, &mut i);
                    start = i;
                } else if !in_quotes {
                    i -= 1;
                    break 'line;
                }
            }
        }
    }

    *pos = i;
    (i > start).then_some(Line {
        start,
        end: i,
        equals,
    })
}

/// Decodes bytes as Latin-1, one char per byte.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// Port of `iniUnescapedKey`. Keys are always Latin-1, even in a UTF-8 file: `\`
/// becomes `/`, `%XX` a Latin-1 char and `%UXXXX` a UTF-16 unit. A malformed `%`
/// escape is kept as a literal `%`.
fn unescape_key(key: &[u8]) -> String {
    let mut units: Vec<u16> = Vec::with_capacity(key.len());
    let mut i = 0;
    while i < key.len() {
        let ch = key[i];
        if ch == b'\\' {
            units.push(u16::from(b'/'));
            i += 1;
            continue;
        }
        if ch != b'%' || i == key.len() - 1 {
            units.push(u16::from(ch));
            i += 1;
            continue;
        }

        let (first_digit, digits) = if key[i + 1] == b'U' {
            (i + 2, 4)
        } else {
            (i + 1, 2)
        };
        let unit = key
            .get(first_digit..first_digit + digits)
            .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))
            .and_then(|hex| u16::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match unit {
            Some(unit) => {
                units.push(unit);
                i = first_digit + digits;
            }
            None => {
                units.push(u16::from(b'%'));
                i += 1;
            }
        }
    }
    String::from_utf16_lossy(&units)
}

/// A value after [`unescape_value`], in UTF-16 units so that `\x` escapes of
/// surrogate halves pair up.
enum Unescaped {
    Single(Vec<u16>),
    List(Vec<Vec<u16>>),
}

/// The simple escapes `\a \b \f \n \r \t \v \" \? \' \\`.
fn simple_escape(ch: u8) -> Option<u16> {
    Some(u16::from(match ch {
        b'a' => 0x07,
        b'b' => 0x08,
        b'f' => 0x0C,
        b'n' => b'\n',
        b'r' => b'\r',
        b't' => b'\t',
        b'v' => 0x0B,
        b'"' | b'?' | b'\'' | b'\\' => ch,
        _ => return None,
    }))
}

/// Removes trailing spaces and tabs from `text`, but not below `limit`
/// (`iniChopTrailingSpaces`).
fn chop_trailing_spaces(text: &mut Vec<u16>, limit: usize) {
    while text.len() > limit && matches!(text.last(), Some(&0x20 | &0x09)) {
        text.pop();
    }
}

/// Port of `iniUnescapedStringList`, a goto state machine in Qt. `'normal` is Qt's
/// `StNormal` (it resets the trailing-space chop limit), `skip_spaces` its
/// `StSkipSpaces`, and `break 'end` its `goto end`, which skips the final chop.
///
/// Leading spaces and tabs are skipped; trailing ones are chopped unless quoted.
/// `"` toggles quoting and is removed, and the spaces after a closing quote are
/// skipped. An unquoted `,` makes the value a list. `\x` and octal escapes are greedy
/// and accumulate into one 16-bit unit; any escape Qt does not know is dropped with
/// its backslash.
fn unescape_value(s: &[u8], utf8: bool) -> Unescaped {
    let to = s.len();
    let mut is_list = false;
    let mut list: Vec<Vec<u16>> = Vec::new();
    let mut in_quoted = false;
    let mut current_quoted = false;
    let mut result: Vec<u16> = Vec::new();
    let mut i = 0;
    let mut skip_spaces = true;

    'end: {
        'normal: loop {
            if skip_spaces {
                while i < to && (s[i] == b' ' || s[i] == b'\t') {
                    i += 1;
                }
                skip_spaces = false;
            }
            let mut chop_limit = result.len();

            while i < to {
                match s[i] {
                    b'\\' => {
                        i += 1;
                        if i >= to {
                            break 'end;
                        }
                        let ch = s[i];
                        i += 1;
                        if let Some(escaped) = simple_escape(ch) {
                            result.push(escaped);
                            continue 'normal;
                        }
                        let greedy = if ch == b'x' {
                            if i >= to {
                                break 'end;
                            }
                            s[i].is_ascii_hexdigit().then_some((0, 16))
                        } else if (b'0'..=b'7').contains(&ch) {
                            Some((u16::from(ch - b'0'), 8))
                        } else {
                            if (ch == b'\n' || ch == b'\r')
                                && i < to
                                && (s[i] == b'\n' || s[i] == b'\r')
                                && s[i] != ch
                            {
                                i += 1;
                            }
                            // Any other escaped byte is dropped with its backslash.
                            None
                        };
                        if let Some((mut escaped, radix)) = greedy {
                            // Accumulates into a 16-bit unit, as Qt's char16_t does.
                            loop {
                                let digit = s.get(i).and_then(|&b| char::from(b).to_digit(radix));
                                match digit {
                                    Some(digit) => {
                                        escaped = (escaped << if radix == 16 { 4 } else { 3 })
                                            + digit as u16;
                                        i += 1;
                                    }
                                    None => {
                                        result.push(escaped);
                                        if i >= to {
                                            break 'end;
                                        }
                                        continue 'normal;
                                    }
                                }
                            }
                        }
                        chop_limit = result.len();
                    }
                    b'"' => {
                        i += 1;
                        current_quoted = true;
                        in_quoted = !in_quoted;
                        if !in_quoted {
                            skip_spaces = true;
                            continue 'normal;
                        }
                    }
                    b',' if !in_quoted => {
                        if !current_quoted {
                            chop_trailing_spaces(&mut result, chop_limit);
                        }
                        is_list = true;
                        list.push(std::mem::take(&mut result));
                        current_quoted = false;
                        i += 1;
                        skip_spaces = true;
                        continue 'normal;
                    }
                    _ => {
                        let mut j = i + 1;
                        while j < to && !matches!(s[j], b'\\' | b'"' | b',') {
                            j += 1;
                        }
                        // The chunk stops only at ASCII bytes, so it never splits a
                        // UTF-8 sequence.
                        if utf8 {
                            result.extend(String::from_utf8_lossy(&s[i..j]).encode_utf16());
                        } else {
                            result.extend(s[i..j].iter().map(|&byte| u16::from(byte)));
                        }
                        i = j;
                    }
                }
            }
            if !current_quoted {
                chop_trailing_spaces(&mut result, chop_limit);
            }
            break 'end;
        }
    }

    if is_list {
        list.push(result);
        Unescaped::List(list)
    } else {
        Unescaped::Single(result)
    }
}

/// Port of `stringToVariant` for the forms CAO can meet.
///
/// `@Invalid()` is [`Value::Invalid`] and `@@x` is the string `@x`. Qt's other
/// encodings ending in `)` are kept verbatim as [`Value::Encoded`]. Anything else,
/// including an unknown `@Foo(…)`, is a string, which Qt writes back as `@@Foo(…)`.
fn string_to_value(text: String) -> Value {
    if text.starts_with('@') {
        if text.ends_with(')') {
            const ENCODINGS: [&str; 7] = [
                "@ByteArray(",
                "@String(",
                "@Variant(",
                "@DateTime(",
                "@Rect(",
                "@Size(",
                "@Point(",
            ];
            if text == "@Invalid()" {
                return Value::Invalid;
            }
            if ENCODINGS.iter().any(|prefix| text.starts_with(prefix)) {
                return Value::Encoded(text);
            }
        }
        if let Some(literal) = text.strip_prefix('@').filter(|rest| rest.starts_with('@')) {
            return Value::String(literal.to_owned());
        }
    }
    Value::String(text)
}

/// Port of `stringListToVariantList`: each element goes through
/// [`string_to_value`]. In Qt only a list with a single-`@` element becomes a
/// `QVariantList`, but a plain element converts the same either way.
fn string_list_to_value(elements: Vec<String>) -> Value {
    Value::List(elements.into_iter().map(string_to_value).collect())
}

impl IniFile {
    /// Keeps the first format error, with its 1-based physical line.
    fn record_error(&mut self, data: &[u8], offset: usize, kind: FormatErrorKind) {
        if self.format_error.is_none() {
            let line = 1 + data[..offset].iter().filter(|&&byte| byte == b'\n').count();
            self.format_error = Some(FormatError { line, kind });
        }
    }
}
