//! The leftovers rules of the output-tree comparator (#467, #491).
//!
//! Staging leaves control files behind each Mod Root's `.cao-staging`, and
//! their bytes differ between any two runs even when both did the same thing:
//!
//! - `ownership.manifest` names the side's own absolute Mod Root, a Run ID and
//!   random nonces, so it is compared semantically: same version, each side's
//!   root naming the Mod Root it sits in, and the same records once Run IDs
//!   and nonces become placeholders.
//! - `owner.lock` is compared for presence only.
//!
//! `.caobad` and `.bak` files need no rule: they must be byte-identical, which
//! the comparator's default already demands. Staging paths are normalised by
//! the comparator itself before pairing. [`crate::rules::ParityRules`] routes
//! files to these rules.

use std::path::Path;

use crate::HarnessError;
use crate::case::read_file;
use crate::normalise::staging_placeholders;
use crate::tree::{ArtifactRule, RuleOutcome};

/// `owner.lock` carries no content: being present on both sides is enough.
pub struct PresenceRule;

impl ArtifactRule for PresenceRule {
    fn name(&self) -> &'static str {
        "Staging Lock Presence"
    }

    fn compare(&self, _oracle: &Path, _rust: &Path) -> Result<RuleOutcome, HarnessError> {
        Ok(RuleOutcome::Equivalent)
    }
}

/// `ownership.manifest` compared by meaning rather than bytes.
pub struct ManifestRule;

impl ArtifactRule for ManifestRule {
    fn name(&self) -> &'static str {
        "Ownership Manifest Semantics"
    }

    fn compare(&self, oracle: &Path, rust: &Path) -> Result<RuleOutcome, HarnessError> {
        let read = |path: &Path| -> Result<Result<Manifest, String>, HarnessError> {
            let text = String::from_utf8_lossy(&read_file(path)?).into_owned();
            Ok(Manifest::parse(&text)
                .and_then(|manifest| manifest.check_root(path).map(|()| manifest)))
        };
        let (oracle, rust) = match (read(oracle)?, read(rust)?) {
            (Ok(oracle), Ok(rust)) => (oracle, rust),
            (Err(error), _) => return Ok(RuleOutcome::Different(format!("oracle side: {error}"))),
            (_, Err(error)) => return Ok(RuleOutcome::Different(format!("Rust side: {error}"))),
        };
        let (oracle, rust) = (oracle.normalised(), rust.normalised());
        Ok(if oracle == rust {
            RuleOutcome::Equivalent
        } else {
            RuleOutcome::Different(format!(
                "the oracle's manifest normalises to {oracle:?}, the Rust side's to {rust:?}"
            ))
        })
    }
}

/// One parsed `CAO-STAGING` manifest.
#[derive(Debug, PartialEq, Eq)]
pub struct Manifest {
    pub version: u32,
    pub root: String,
    pub run_id: String,
    pub child: String,
    /// `(kind, name)` in file order; kind is `D`, `F` or `S`.
    pub records: Vec<(char, String)>,
}

impl Manifest {
    /// Parses the documented grammar: whitespace-separated fields, with every
    /// string field quoted in C++ `std::quoted` style.
    ///
    /// # Errors
    /// A description of the first malformed field.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut tokens = Tokens(text.chars().peekable());
        if tokens.bare()? != "CAO-STAGING" {
            return Err("the signature is not CAO-STAGING".into());
        }
        let version = number(&tokens.bare()?)?;
        let root = tokens.quoted()?;
        let run_id = tokens.quoted()?;
        let child = tokens.quoted()?;
        let count = number(&tokens.bare()?)? as usize;
        let mut records = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let kind = tokens.bare()?;
            let [kind] = kind.chars().collect::<Vec<_>>()[..] else {
                return Err(format!("`{kind}` is not a record kind"));
            };
            records.push((kind, tokens.quoted()?));
        }
        if tokens.skip_whitespace() {
            return Err("there is data after the last record".into());
        }
        Ok(Self {
            version,
            root,
            run_id,
            child,
            records,
        })
    }

    /// Checks the recorded root names the Mod Root this manifest sits in:
    /// `<root>/.cao-staging/ownership.manifest`.
    fn check_root(&self, manifest: &Path) -> Result<(), String> {
        let mod_root = manifest
            .parent()
            .and_then(Path::parent)
            .ok_or("the manifest has no Mod Root")?;
        let generic = |text: &str| {
            let text = text.replace('\\', "/");
            text.strip_prefix("//?/")
                .unwrap_or(&text)
                .to_ascii_lowercase()
        };
        if generic(&self.root) == generic(&mod_root.to_string_lossy()) {
            Ok(())
        } else {
            Err(format!(
                "the manifest names the Mod Root `{}`, but sits in `{}`",
                self.root,
                mod_root.display()
            ))
        }
    }

    /// The facts both builds must agree on: the version, the run child and
    /// the records, with Run IDs and nonces as placeholders.
    fn normalised(&self) -> (u32, String, Vec<(char, String)>) {
        let name = |text: &str| staging_placeholders(text, &self.run_id);
        let records = self
            .records
            .iter()
            .map(|(kind, path)| (*kind, name(path)))
            .collect();
        (self.version, name(&self.child), records)
    }
}

fn number(token: &str) -> Result<u32, String> {
    token
        .parse()
        .map_err(|_| format!("`{token}` is not a number"))
}

/// A cursor over manifest text.
struct Tokens<'a>(std::iter::Peekable<std::str::Chars<'a>>);

impl Tokens<'_> {
    /// Skips whitespace, reporting whether anything follows.
    fn skip_whitespace(&mut self) -> bool {
        while self
            .0
            .next_if(|character| character.is_whitespace())
            .is_some()
        {}
        self.0.peek().is_some()
    }

    /// The next unquoted token.
    fn bare(&mut self) -> Result<String, String> {
        if !self.skip_whitespace() {
            return Err("the manifest ends early".into());
        }
        let mut token = String::new();
        while let Some(character) = self.0.next_if(|character| !character.is_whitespace()) {
            token.push(character);
        }
        Ok(token)
    }

    /// The next quoted string, unescaping `\"` and `\\`.
    fn quoted(&mut self) -> Result<String, String> {
        if !self.skip_whitespace() || self.0.next() != Some('"') {
            return Err("a string field is not quoted".into());
        }
        let mut value = String::new();
        loop {
            match self.0.next() {
                Some('"') => return Ok(value),
                Some('\\') => value.push(self.0.next().ok_or("a quoted field is truncated")?),
                Some(character) => value.push(character),
                None => return Err("a quoted field is truncated".into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_grammar_with_escapes() {
        let text = "CAO-STAGING 3\n\"C:/m/A \\\"q\\\" \\\\b\"\n\"r\" \"run-r-n\"\n2\nD \"run-r-n\"\nS \"t/x.dds\"\n";
        let manifest = Manifest::parse(text).unwrap();
        assert_eq!(manifest.version, 3);
        assert_eq!(manifest.root, "C:/m/A \"q\" \\b");
        assert_eq!(manifest.run_id, "r");
        assert_eq!(
            manifest.records,
            [('D', "run-r-n".to_owned()), ('S', "t/x.dds".to_owned())]
        );
    }

    #[test]
    fn rejects_bare_strings_short_record_lists_and_trailing_data() {
        assert!(Manifest::parse("CAO-STAGING 3\nC:/m\n\"r\" \"c\"\n0\n").is_err());
        assert!(Manifest::parse("CAO-STAGING 3\n\"C:/m\"\n\"r\" \"c\"\n2\nD \"c\"\n").is_err());
        assert!(Manifest::parse("CAO-STAGING 3\n\"C:/m\"\n\"r\" \"c\"\n0\nextra").is_err());
    }
}
