//! The writing half of Qt 5.15's QSettings IniFormat (`qsettings.cpp`), unchanged
//! from Qt so the C++ build reads every file Rust writes.
//!
//! Output is pure ASCII: anything else is `\x`- or `%`-escaped, as Qt writes when no
//! INI codec is set.

use super::value::Value;
use super::{Entry, IniFile};

/// Port of `writeIniFile`. Root keys go under `[General]` and a real `General` group
/// under `[%General]`. Sections keep the order their first key had and keys their
/// own order, so keys read from a file stay where they were and new ones are
/// appended. Lines end in CRLF, with a blank line between sections.
pub(super) fn serialize(ini: &IniFile) -> Vec<u8> {
    // Sections in order of first appearance, keyed by their exact spelling as Qt's
    // QMap<QString, …> is.
    let mut sections: Vec<(&str, Vec<&Entry>)> = Vec::new();
    for entry in &ini.entries {
        let section = entry.key.split_once('/').map_or("", |(section, _)| section);
        match sections.iter_mut().find(|(name, _)| *name == section) {
            Some((_, entries)) => entries.push(entry),
            None => sections.push((section, vec![entry])),
        }
    }

    let mut out = Vec::new();
    for (index, (section, entries)) in sections.iter().enumerate() {
        if index != 0 {
            out.extend_from_slice(b"\r\n");
        }
        let mut header = Vec::new();
        escape_key(section, &mut header);
        if header.is_empty() {
            out.extend_from_slice(b"[General]");
        } else if header.eq_ignore_ascii_case(b"general") {
            out.extend_from_slice(b"[%General]");
        } else {
            out.push(b'[');
            out.extend_from_slice(&header);
            out.push(b']');
        }
        out.extend_from_slice(b"\r\n");

        for entry in entries {
            let key = match entry.key.split_once('/') {
                Some((_, key)) => key,
                None => &entry.key,
            };
            escape_key(key, &mut out);
            out.push(b'=');
            match &entry.value {
                Value::List(elements) if elements.is_empty() => {
                    out.extend_from_slice(b"@Invalid()");
                }
                Value::List(elements) => {
                    for (i, element) in elements.iter().enumerate() {
                        if i != 0 {
                            out.extend_from_slice(b", ");
                        }
                        escape_string(&variant_to_string(element), &mut out);
                    }
                }
                scalar => escape_string(&variant_to_string(scalar), &mut out),
            }
            out.extend_from_slice(b"\r\n");
        }
    }
    out
}

/// Port of `variantToString` for a scalar. A string starting with `@` gains a second
/// `@`, and one holding NUL is written as `@String(…)`.
fn variant_to_string(value: &Value) -> String {
    match value {
        Value::Invalid => "@Invalid()".to_owned(),
        Value::String(text) if text.contains('\0') => format!("@String({text})"),
        Value::String(text) if text.starts_with('@') => format!("@{text}"),
        Value::String(text) => text.clone(),
        Value::Encoded(raw) => raw.clone(),
        // Lists hold scalars only; Qt would write a nested list as a QDataStream blob,
        // which nothing in CAO produces.
        Value::List(_) => "@Invalid()".to_owned(),
    }
}

/// Port of `iniEscapedKey`: `[A-Za-z0-9_.-]` as is, `/` as `\`, other units up to
/// 0xFF as `%XX` and the rest as `%UXXXX`, uppercase hex.
fn escape_key(key: &str, out: &mut Vec<u8>) {
    for unit in key.encode_utf16() {
        match unit {
            0x2F => out.push(b'\\'),
            _ if u8::try_from(unit)
                .is_ok_and(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte)) =>
            {
                out.push(unit as u8);
            }
            0..=0xFF => out.extend_from_slice(format!("%{unit:02X}").as_bytes()),
            _ => out.extend_from_slice(format!("%U{unit:04X}").as_bytes()),
        }
    }
}

/// Port of `iniEscapedString` with no codec.
///
/// Control characters get named escapes or `\x`, and so does every UTF-16 unit at or
/// above 0x7F (lowercase hex, unpadded). After `\x…` or `\0`, a following hex-digit
/// character is hex-escaped too, so Qt's greedy reader stops in the right place. The
/// value is quoted when it holds `;`, `,` or `=`, or starts or ends with a space.
fn escape_string(text: &str, out: &mut Vec<u8>) {
    let start = out.len();
    let mut needs_quotes = false;
    let mut escape_next_if_digit = false;

    for unit in text.encode_utf16() {
        if matches!(unit, 0x3B | 0x2C | 0x3D) {
            needs_quotes = true;
        }
        let is_hex_digit = u8::try_from(unit).is_ok_and(|byte| byte.is_ascii_hexdigit());
        if escape_next_if_digit && is_hex_digit {
            out.extend_from_slice(format!("\\x{unit:x}").as_bytes());
            continue;
        }
        escape_next_if_digit = false;

        match unit {
            0x00 => {
                out.extend_from_slice(b"\\0");
                escape_next_if_digit = true;
            }
            0x07 => out.extend_from_slice(b"\\a"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x0C => out.extend_from_slice(b"\\f"),
            0x0A => out.extend_from_slice(b"\\n"),
            0x0D => out.extend_from_slice(b"\\r"),
            0x09 => out.extend_from_slice(b"\\t"),
            0x0B => out.extend_from_slice(b"\\v"),
            0x22 | 0x5C => {
                out.push(b'\\');
                out.push(unit as u8);
            }
            0..=0x1F | 0x7F.. => {
                out.extend_from_slice(format!("\\x{unit:x}").as_bytes());
                escape_next_if_digit = true;
            }
            _ => out.push(unit as u8),
        }
    }

    let written = &out[start..];
    if needs_quotes || written.first() == Some(&b' ') || written.last() == Some(&b' ') {
        out.insert(start, b'"');
        out.push(b'"');
    }
}
