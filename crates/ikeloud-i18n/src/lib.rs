//! Translations of ike loud.
//!
//! Code never holds user-facing text, only a [`Message`]: a key and
//! arguments. The texts live in Fluent files `i18n/<language>/ikeloud.ftl`.
//! Czech and English are built into the program; another language is added
//! as a folder with a file (see [`Translations::with_dir`]), without
//! rebuilding the program. Missing text falls back to English.
//!
//! This crate is the single wrapper of the Fluent libraries.

mod conflict_copy;
mod language;
mod messages;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

pub use language::{languages_from_env, languages_from_vars};
pub use messages::ToMessage;

/// Name of the translation file in every language folder.
pub const TRANSLATION_FILE: &str = "ikeloud.ftl";

/// Fallback language for missing text.
pub const FALLBACK_LANGUAGE: &str = "en";

/// Translations built into the program: (language code, file content).
pub const BUILTIN: &[(&str, &str)] = &[
    ("cs", include_str!("../../../i18n/cs/ikeloud.ftl")),
    ("en", include_str!("../../../i18n/en/ikeloud.ftl")),
];

/// Value of a message argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// Integer; also selects plural forms.
    Number(u64),
    /// Text that is not translated (a file name).
    Text(String),
    /// Size in bytes; formatted per language ("1,5 GB").
    Size(u64),
    /// Nested message, translated in the same language.
    Message(Box<Message>),
}

/// A message for the user: a key from the translation file and arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Message key in the `.ftl` file.
    pub key: &'static str,
    /// Named arguments (`{ $name }` in the text).
    pub args: Vec<(&'static str, Value)>,
}

impl Message {
    /// A message without arguments.
    pub fn new(key: &'static str) -> Self {
        Self {
            key,
            args: Vec::new(),
        }
    }

    /// Adds an argument.
    #[must_use]
    pub fn arg(mut self, name: &'static str, value: Value) -> Self {
        self.args.push((name, value));
        self
    }
}

/// Error while loading translations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// The language code is not valid (e.g. a folder name).
    InvalidLanguageCode(String),
    /// A translation file cannot be read.
    Read {
        /// Which file.
        path: PathBuf,
        /// Error description from the system.
        error: String,
    },
    /// A translation file has a syntax error.
    Syntax {
        /// Language code.
        language: String,
        /// Error descriptions from the parser.
        errors: Vec<String>,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLanguageCode(code) => write!(f, "invalid language code: {code}"),
            Self::Read { path, error } => write!(f, "cannot read {}: {error}", path.display()),
            Self::Syntax { language, errors } => {
                write!(f, "error in translation {language}: {}", errors.join("; "))
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// Error while translating one message in one language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// The key is missing in the translation.
    MissingKey(&'static str),
    /// The text could not be built (missing argument, broken reference).
    Format {
        /// Message key.
        key: &'static str,
        /// Error descriptions from Fluent.
        errors: Vec<String>,
    },
}

/// Loaded translations for a chosen order of languages.
pub struct Translations {
    /// Bundles in order of preference; the last is always the fallback.
    bundles: Vec<FluentBundle<FluentResource>>,
}

impl fmt::Debug for Translations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Translations")
            .field("languages", &self.languages())
            .finish()
    }
}

impl Translations {
    /// Translations from the built-in languages only.
    ///
    /// `requested` is the user's order of languages (e.g. from
    /// [`languages_from_env`]); unknown languages are skipped.
    pub fn new(requested: &[String]) -> Result<Self, LoadError> {
        Self::from_sources(requested, builtin_sources())
    }

    /// Translations from the built-in languages and from folder `dir`.
    ///
    /// Each subfolder `dir/<language code>/ikeloud.ftl` adds a language or
    /// replaces the built-in translation of the same language.
    pub fn with_dir(requested: &[String], dir: &Path) -> Result<Self, LoadError> {
        let mut sources = builtin_sources();
        sources.extend(read_dir(dir)?);
        Self::from_sources(requested, sources)
    }

    fn from_sources(
        requested: &[String],
        sources: BTreeMap<String, String>,
    ) -> Result<Self, LoadError> {
        let available: Vec<&str> = sources.keys().map(String::as_str).collect();
        let order = language::choose(requested, &available);
        let mut bundles = Vec::new();
        for code in order {
            let Some(text) = sources.get(&code) else {
                continue;
            };
            bundles.push(build_bundle(&code, text.clone())?);
        }
        Ok(Self { bundles })
    }

    /// Languages in the order text is looked up.
    pub fn languages(&self) -> Vec<String> {
        self.bundles
            .iter()
            .filter_map(|b| b.locales.first().map(ToString::to_string))
            .collect()
    }

    /// Translates a message. If the text is missing or broken in the first
    /// language, the next one is used; if it is missing everywhere, the key
    /// is returned.
    pub fn text(&self, message: &Message) -> String {
        self.bundles
            .iter()
            .find_map(|b| self.format(b, message).ok())
            .unwrap_or_else(|| message.key.to_string())
    }

    /// Translates a message in the first language only, without fallback.
    /// For tests and for checking that translations are complete.
    pub fn text_exact(&self, message: &Message) -> Result<String, FormatError> {
        match self.bundles.first() {
            Some(b) => self.format(b, message),
            None => Err(FormatError::MissingKey(message.key)),
        }
    }

    fn format(
        &self,
        bundle: &FluentBundle<FluentResource>,
        message: &Message,
    ) -> Result<String, FormatError> {
        let pattern = bundle
            .get_message(message.key)
            .and_then(|m| m.value())
            .ok_or(FormatError::MissingKey(message.key))?;
        let mut args = FluentArgs::new();
        for (name, value) in &message.args {
            let value: FluentValue<'_> = match value {
                Value::Number(n) => FluentValue::from(*n),
                Value::Text(t) => FluentValue::from(t.clone()),
                Value::Size(b) => FluentValue::from(self.size(bundle, *b)?),
                Value::Message(m) => FluentValue::from(self.format(bundle, m)?),
            };
            args.set(*name, value);
        }
        let mut errors = Vec::new();
        let text = bundle.format_pattern(pattern, Some(&args), &mut errors);
        if errors.is_empty() {
            Ok(text.into_owned())
        } else {
            Err(FormatError::Format {
                key: message.key,
                errors: errors.iter().map(ToString::to_string).collect(),
            })
        }
    }

    /// Size in SI units (1 kB = 1000 B), one decimal below 100 units.
    fn size(
        &self,
        bundle: &FluentBundle<FluentResource>,
        bytes: u64,
    ) -> Result<String, FormatError> {
        let (key, number) = size_in_units(bytes);
        let separator = self.format(bundle, &Message::new("decimal-separator"))?;
        let number = number.replace('.', &separator);
        self.format(bundle, &Message::new(key).arg("value", Value::Text(number)))
    }
}

/// Picks the unit and formats the number with a dot as decimal separator.
fn size_in_units(bytes: u64) -> (&'static str, String) {
    const UNITS: [&str; 4] = ["size-kb", "size-mb", "size-gb", "size-tb"];
    if bytes < 1000 {
        return ("size-bytes", bytes.to_string());
    }
    let mut key = "size-kb";
    let mut divisor: u64 = 1000;
    for (i, unit) in UNITS.iter().enumerate() {
        key = unit;
        divisor = 1000u64.pow(u32::try_from(i + 1).unwrap_or(1));
        if bytes / divisor < 1000 || i + 1 == UNITS.len() {
            break;
        }
    }
    // Tenths rounded down, so 999 999 B is not "1000 kB".
    let tenths = u128::from(bytes) * 10 / u128::from(divisor);
    let whole = tenths / 10;
    let rest = tenths % 10;
    let text = if whole >= 100 || rest == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{rest}")
    };
    (key, text)
}

fn builtin_sources() -> BTreeMap<String, String> {
    BUILTIN
        .iter()
        .map(|(code, text)| ((*code).to_string(), (*text).to_string()))
        .collect()
}

fn read_dir(dir: &Path) -> Result<BTreeMap<String, String>, LoadError> {
    let read_error = |path: &Path, e: std::io::Error| LoadError::Read {
        path: path.to_path_buf(),
        error: e.to_string(),
    };
    let mut sources = BTreeMap::new();
    let entries = std::fs::read_dir(dir).map_err(|e| read_error(dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| read_error(dir, e))?;
        let file = entry.path().join(TRANSLATION_FILE);
        if !file.is_file() {
            continue;
        }
        let code = entry.file_name().to_string_lossy().into_owned();
        code.parse::<LanguageIdentifier>()
            .map_err(|_| LoadError::InvalidLanguageCode(code.clone()))?;
        let text = std::fs::read_to_string(&file).map_err(|e| read_error(&file, e))?;
        sources.insert(code, text);
    }
    Ok(sources)
}

fn build_bundle(code: &str, text: String) -> Result<FluentBundle<FluentResource>, LoadError> {
    let language: LanguageIdentifier = code
        .parse()
        .map_err(|_| LoadError::InvalidLanguageCode(code.to_string()))?;
    let resource = FluentResource::try_new(text).map_err(|(_, errors)| LoadError::Syntax {
        language: code.to_string(),
        errors: errors.iter().map(ToString::to_string).collect(),
    })?;
    let mut bundle = FluentBundle::new_concurrent(vec![language]);
    // No invisible bidi isolation marks: GTK handles direction itself and
    // they would get in the way in a terminal.
    bundle.set_use_isolating(false);
    bundle
        .add_resource(resource)
        .map_err(|errors| LoadError::Syntax {
            language: code.to_string(),
            errors: errors.iter().map(ToString::to_string).collect(),
        })?;
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::size_in_units;

    #[test]
    fn sizes() {
        let cases: &[(u64, &str, &str)] = &[
            (0, "size-bytes", "0"),
            (999, "size-bytes", "999"),
            (1000, "size-kb", "1"),
            (1500, "size-kb", "1.5"),
            (1999, "size-kb", "1.9"),
            (99_950, "size-kb", "99.9"),
            (100_000, "size-kb", "100"),
            (999_999, "size-kb", "999"),
            (1_000_000, "size-mb", "1"),
            (123_456_789, "size-mb", "123"),
            (5_000_000_000, "size-gb", "5"),
            (2_500_000_000_000, "size-tb", "2.5"),
            (u64::MAX, "size-tb", "18446744"),
        ];
        for (bytes, key, text) in cases {
            assert_eq!(
                size_in_units(*bytes),
                (*key, (*text).to_string()),
                "{bytes} B"
            );
        }
    }
}
