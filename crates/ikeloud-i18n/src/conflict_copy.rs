//! Translated name of a conflict copy.

use ikeloud_safety::RelPath;

use crate::{Message, Translations, Value};

impl Translations {
    /// Name of the conflict copy of `path` in the same folder.
    ///
    /// `date` looks like `2026-11-15`, `number` starts at 1 (higher when the
    /// name is taken). A slash coming from a translation is replaced by
    /// a hyphen so the copy stays in the same folder.
    pub fn conflict_copy_name(&self, path: &RelPath, date: &str, number: u32) -> RelPath {
        let (stem, extension) = split_extension(path.name());
        let message = Message::new("conflict-copy-name")
            .arg("stem", Value::Text(stem.to_string()))
            .arg("extension", Value::Text(extension.to_string()))
            .arg("date", Value::Text(date.to_string()))
            .arg("number", Value::Number(u64::from(number)));
        let name = self.text(&message).replace('/', "-");
        match path.parent() {
            "" => RelPath::new(name),
            parent => RelPath::new(format!("{parent}/{name}")),
        }
    }
}

/// `report.odt` → (`report`, `.odt`); the hidden file `.bashrc` has no extension.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (name.get(..i).unwrap_or(name), name.get(i..).unwrap_or("")),
        _ => (name, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::split_extension;

    #[test]
    fn extensions() {
        let cases: &[(&str, (&str, &str))] = &[
            ("report.odt", ("report", ".odt")),
            ("archive.tar.gz", ("archive.tar", ".gz")),
            ("README", ("README", "")),
            (".bashrc", (".bashrc", "")),
            (".config.toml", (".config", ".toml")),
            ("end.", ("end", ".")),
        ];
        for (name, expected) in cases {
            assert_eq!(split_extension(name), *expected, "{name}");
        }
    }
}
