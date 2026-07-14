//! Versioned fork-owned state documents with compatible-field retention.

use std::collections::BTreeMap;

pub(super) const CURRENT_SCHEMA_VERSION: u64 = 2;

/// A decoded state document whose unrecognized compatible fields remain intact.
#[derive(Clone, Debug)]
pub(super) struct StateDocument {
    fields: BTreeMap<String, String>,
    migrated: bool,
}

impl StateDocument {
    /// Decodes one strict UTF-8 key/value document and applies ordered migrations.
    pub(super) fn decode(bytes: &[u8]) -> Result<Self, DocumentError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| DocumentError::Corrupt("state document is not valid UTF-8"))?;
        let mut fields = BTreeMap::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let (key, value) = line.split_once('=').ok_or(DocumentError::Corrupt(
                "state document contains a malformed line",
            ))?;
            if key.is_empty() {
                return Err(DocumentError::Corrupt(
                    "state document contains an empty field name",
                ));
            }
            fields.insert(key.to_owned(), value.to_owned());
        }

        let version = schema_version(&fields)?;
        if version > CURRENT_SCHEMA_VERSION {
            return Err(DocumentError::NewerSchema {
                found: version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        if version == 0 {
            return Err(DocumentError::Corrupt(
                "schema version zero is not supported",
            ));
        }

        let mut document = Self {
            fields,
            migrated: false,
        };
        while document.schema_version()? < CURRENT_SCHEMA_VERSION {
            document.migrate_one()?;
            document.migrated = true;
        }
        Ok(document)
    }

    /// Constructs a current-schema document from known fields.
    pub(super) fn current(fields: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut document = BTreeMap::from([(
            "schema_version".to_owned(),
            CURRENT_SCHEMA_VERSION.to_string(),
        )]);
        document.extend(fields);
        Self {
            fields: document,
            migrated: false,
        }
    }

    /// Reports whether one or more ordered migrations changed this document.
    pub(super) const fn was_migrated(&self) -> bool {
        self.migrated
    }

    /// Returns a compatible field without consuming the retained document.
    pub(super) fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    /// Updates one known field while retaining every unrecognized compatible field.
    pub(super) fn set(&mut self, key: &str, value: impl Into<String>) {
        self.fields.insert(key.to_owned(), value.into());
    }

    /// Encodes deterministic UTF-8 key/value lines for durable fork-owned storage.
    pub(super) fn encode(&self) -> Vec<u8> {
        let mut encoded = String::new();
        if let Some(version) = self.fields.get("schema_version") {
            encoded.push_str("schema_version=");
            encoded.push_str(version);
            encoded.push('\n');
        }
        for (key, value) in &self.fields {
            if key == "schema_version" {
                continue;
            }
            encoded.push_str(key);
            encoded.push('=');
            encoded.push_str(value);
            encoded.push('\n');
        }
        encoded.into_bytes()
    }

    /// Applies exactly one forward migration so skipped versions cannot be hidden.
    fn migrate_one(&mut self) -> Result<(), DocumentError> {
        match self.schema_version()? {
            1 => {
                self.fields
                    .insert("schema_version".to_owned(), "2".to_owned());
                Ok(())
            }
            _ => Err(DocumentError::Corrupt(
                "state document has no ordered migration path",
            )),
        }
    }

    /// Returns the validated integer schema revision.
    fn schema_version(&self) -> Result<u64, DocumentError> {
        schema_version(&self.fields)
    }
}

/// Stable decode categories mapped to application-owned failures by the adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DocumentError {
    /// Bytes or required structural fields are malformed.
    Corrupt(&'static str),
    /// The document belongs to a later application version and must remain untouched.
    NewerSchema { found: u64, supported: u64 },
}

/// Extracts a required positive integer schema revision.
fn schema_version(fields: &BTreeMap<String, String>) -> Result<u64, DocumentError> {
    fields
        .get("schema_version")
        .ok_or(DocumentError::Corrupt(
            "state document has no schema_version",
        ))?
        .parse()
        .map_err(|_| DocumentError::Corrupt("state document schema_version is not an integer"))
}
