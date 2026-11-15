//! Choosing the language from the environment and the available translations.

use crate::FALLBACK_LANGUAGE;

/// Languages the user set in the system, in order of preference.
pub fn languages_from_env() -> Vec<String> {
    let var = |name: &str| std::env::var(name).ok();
    languages_from_vars(
        var("LANGUAGE").as_deref(),
        var("LC_ALL").as_deref(),
        var("LC_MESSAGES").as_deref(),
        var("LANG").as_deref(),
    )
}

/// Languages from environment variable values (a pure function for tests).
///
/// Order as in gettext: the `LANGUAGE` list (separated by `:`), then the
/// first set one of `LC_ALL`, `LC_MESSAGES`, `LANG`. The values `C` and
/// `POSIX` do not name a language. `cs_CZ.UTF-8@euro` becomes `cs-CZ`.
pub fn languages_from_vars(
    language: Option<&str>,
    lc_all: Option<&str>,
    lc_messages: Option<&str>,
    lang: Option<&str>,
) -> Vec<String> {
    let mut languages: Vec<String> = Vec::new();
    let mut add = |value: &str| {
        if let Some(code) = normalize(value)
            && !languages.contains(&code)
        {
            languages.push(code);
        }
    };
    if let Some(list) = language {
        list.split(':').for_each(&mut add);
    }
    if let Some(locale) = [lc_all, lc_messages, lang]
        .into_iter()
        .flatten()
        .find(|v| !v.is_empty())
    {
        add(locale);
    }
    languages
}

fn normalize(locale: &str) -> Option<String> {
    let without_modifier = locale.split('@').next().unwrap_or_default();
    let without_encoding = without_modifier.split('.').next().unwrap_or_default();
    if without_encoding.is_empty() || without_encoding == "C" || without_encoding == "POSIX" {
        return None;
    }
    Some(without_encoding.replace('_', "-"))
}

/// Chooses the order of available languages from the requested ones. For
/// each request it takes an exact match, otherwise a match of the primary
/// language (`cs-CZ` → `cs`). The fallback language is always appended.
pub(crate) fn choose(requested: &[String], available: &[&str]) -> Vec<String> {
    let primary = |code: &str| code.split('-').next().unwrap_or(code).to_ascii_lowercase();
    let mut chosen: Vec<String> = Vec::new();
    for request in requested {
        let exact = available.iter().find(|a| a.eq_ignore_ascii_case(request));
        let by_primary = || available.iter().find(|a| primary(a) == primary(request));
        if let Some(code) = exact.or_else(by_primary)
            && !chosen.iter().any(|c| c == code)
        {
            chosen.push((*code).to_string());
        }
    }
    if !chosen.iter().any(|c| c == FALLBACK_LANGUAGE) {
        chosen.push(FALLBACK_LANGUAGE.to_string());
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::{choose, languages_from_vars};

    #[test]
    fn environment_variables() {
        type Case<'a> = (&'a str, [Option<&'a str>; 4], &'a [&'a str]);
        let cases: &[Case<'_>] = &[
            ("nothing set", [None, None, None, None], &[]),
            ("LANG", [None, None, None, Some("cs_CZ.UTF-8")], &["cs-CZ"]),
            (
                "LC_ALL wins",
                [None, Some("de_DE.UTF-8"), None, Some("cs_CZ.UTF-8")],
                &["de-DE"],
            ),
            (
                "LC_MESSAGES before LANG",
                [None, None, Some("en_GB.UTF-8"), Some("cs_CZ.UTF-8")],
                &["en-GB"],
            ),
            (
                "empty LC_ALL is skipped",
                [None, Some(""), None, Some("cs_CZ.UTF-8")],
                &["cs-CZ"],
            ),
            (
                "LANGUAGE list then LANG",
                [Some("sk:cs"), None, None, Some("en_US.UTF-8")],
                &["sk", "cs", "en-US"],
            ),
            (
                "modifier",
                [None, None, None, Some("cs_CZ.UTF-8@euro")],
                &["cs-CZ"],
            ),
            (
                "C names no language",
                [None, None, None, Some("C.UTF-8")],
                &[],
            ),
            (
                "POSIX names no language",
                [None, Some("POSIX"), None, None],
                &[],
            ),
            (
                "no duplicates",
                [Some("cs_CZ"), None, None, Some("cs_CZ.UTF-8")],
                &["cs-CZ"],
            ),
        ];
        for (name, [language, lc_all, lc_messages, lang], expected) in cases {
            assert_eq!(
                languages_from_vars(*language, *lc_all, *lc_messages, *lang),
                expected.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "{name}"
            );
        }
    }

    #[test]
    fn choosing() {
        let available = ["cs", "en", "pt-BR"];
        let cases: &[(&str, &[&str], &[&str])] = &[
            ("nothing → fallback", &[], &["en"]),
            ("cs-CZ → cs", &["cs-CZ"], &["cs", "en"]),
            ("exact match", &["pt-BR"], &["pt-BR", "en"]),
            (
                "primary language pt-PT → pt-BR",
                &["pt-PT"],
                &["pt-BR", "en"],
            ),
            ("unknown language", &["ja-JP"], &["en"]),
            (
                "order and no duplicates",
                &["sk", "cs", "cs-CZ", "en-US"],
                &["cs", "en"],
            ),
            ("upper case", &["CS"], &["cs", "en"]),
        ];
        for (name, requested, expected) in cases {
            let requested: Vec<String> = requested.iter().map(ToString::to_string).collect();
            assert_eq!(
                choose(&requested, &available),
                expected.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "{name}"
            );
        }
    }
}
