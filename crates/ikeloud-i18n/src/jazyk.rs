//! Výběr jazyka podle prostředí a podle dostupných překladů.

use crate::ZALOZNI_JAZYK;

/// Jazyky, které si uživatel nastavil v systému, v pořadí přednosti.
pub fn jazyky_z_prostredi() -> Vec<String> {
    let promenna = |nazev: &str| std::env::var(nazev).ok();
    jazyky_z_promennych(
        promenna("LANGUAGE").as_deref(),
        promenna("LC_ALL").as_deref(),
        promenna("LC_MESSAGES").as_deref(),
        promenna("LANG").as_deref(),
    )
}

/// Jazyky z hodnot proměnných prostředí (čistá funkce kvůli testům).
///
/// Pořadí jako u gettextu: seznam `LANGUAGE` (oddělený `:`), pak první
/// nastavená z `LC_ALL`, `LC_MESSAGES`, `LANG`. Hodnoty `C` a `POSIX`
/// jazyk neurčují. `cs_CZ.UTF-8@euro` se převede na `cs-CZ`.
pub fn jazyky_z_promennych(
    language: Option<&str>,
    lc_all: Option<&str>,
    lc_messages: Option<&str>,
    lang: Option<&str>,
) -> Vec<String> {
    let mut jazyky: Vec<String> = Vec::new();
    let mut pridej = |hodnota: &str| {
        if let Some(kod) = normalizuj(hodnota)
            && !jazyky.contains(&kod)
        {
            jazyky.push(kod);
        }
    };
    if let Some(seznam) = language {
        seznam.split(':').for_each(&mut pridej);
    }
    if let Some(locale) = [lc_all, lc_messages, lang]
        .into_iter()
        .flatten()
        .find(|v| !v.is_empty())
    {
        pridej(locale);
    }
    jazyky
}

fn normalizuj(locale: &str) -> Option<String> {
    let bez_modifikatoru = locale.split('@').next().unwrap_or_default();
    let bez_kodovani = bez_modifikatoru.split('.').next().unwrap_or_default();
    if bez_kodovani.is_empty() || bez_kodovani == "C" || bez_kodovani == "POSIX" {
        return None;
    }
    Some(bez_kodovani.replace('_', "-"))
}

/// Vybere z dostupných jazyků pořadí podle požadovaných. U každého
/// požadavku bere přesnou shodu, jinak shodu hlavního jazyka (`cs-CZ` → `cs`).
/// Na konec vždy přidá záložní jazyk.
pub(crate) fn vyber(pozadovane: &[String], dostupne: &[&str]) -> Vec<String> {
    let hlavni = |kod: &str| kod.split('-').next().unwrap_or(kod).to_ascii_lowercase();
    let mut vybrane: Vec<String> = Vec::new();
    for pozadavek in pozadovane {
        let presna = dostupne.iter().find(|d| d.eq_ignore_ascii_case(pozadavek));
        let podle_hlavniho = || dostupne.iter().find(|d| hlavni(d) == hlavni(pozadavek));
        if let Some(kod) = presna.or_else(podle_hlavniho)
            && !vybrane.iter().any(|v| v == kod)
        {
            vybrane.push((*kod).to_string());
        }
    }
    if !vybrane.iter().any(|v| v == ZALOZNI_JAZYK) {
        vybrane.push(ZALOZNI_JAZYK.to_string());
    }
    vybrane
}

#[cfg(test)]
mod tests {
    use super::{jazyky_z_promennych, vyber};

    #[test]
    fn promenne_prostredi() {
        type Pripad<'a> = (&'a str, [Option<&'a str>; 4], &'a [&'a str]);
        let pripady: &[Pripad<'_>] = &[
            ("nic nenastaveno", [None, None, None, None], &[]),
            ("LANG", [None, None, None, Some("cs_CZ.UTF-8")], &["cs-CZ"]),
            (
                "LC_ALL má přednost",
                [None, Some("de_DE.UTF-8"), None, Some("cs_CZ.UTF-8")],
                &["de-DE"],
            ),
            (
                "LC_MESSAGES před LANG",
                [None, None, Some("en_GB.UTF-8"), Some("cs_CZ.UTF-8")],
                &["en-GB"],
            ),
            (
                "prázdné LC_ALL se přeskočí",
                [None, Some(""), None, Some("cs_CZ.UTF-8")],
                &["cs-CZ"],
            ),
            (
                "LANGUAGE seznam a pak LANG",
                [Some("sk:cs"), None, None, Some("en_US.UTF-8")],
                &["sk", "cs", "en-US"],
            ),
            (
                "modifikátor",
                [None, None, None, Some("cs_CZ.UTF-8@euro")],
                &["cs-CZ"],
            ),
            ("C nic neurčuje", [None, None, None, Some("C.UTF-8")], &[]),
            ("POSIX nic neurčuje", [None, Some("POSIX"), None, None], &[]),
            (
                "bez duplicit",
                [Some("cs_CZ"), None, None, Some("cs_CZ.UTF-8")],
                &["cs-CZ"],
            ),
        ];
        for (popis, [language, lc_all, lc_messages, lang], ocekavano) in pripady {
            assert_eq!(
                jazyky_z_promennych(*language, *lc_all, *lc_messages, *lang),
                ocekavano
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                "{popis}"
            );
        }
    }

    #[test]
    fn vyber_jazyka() {
        let dostupne = ["cs", "en", "pt-BR"];
        let pripady: &[(&str, &[&str], &[&str])] = &[
            ("nic → záložní", &[], &["en"]),
            ("cs-CZ → cs", &["cs-CZ"], &["cs", "en"]),
            ("přesná shoda", &["pt-BR"], &["pt-BR", "en"]),
            ("hlavní jazyk pt-PT → pt-BR", &["pt-PT"], &["pt-BR", "en"]),
            ("neznámý jazyk", &["ja-JP"], &["en"]),
            (
                "pořadí a bez duplicit",
                &["sk", "cs", "cs-CZ", "en-US"],
                &["cs", "en"],
            ),
            ("velká písmena", &["CS"], &["cs", "en"]),
        ];
        for (popis, pozadovane, ocekavano) in pripady {
            let pozadovane: Vec<String> = pozadovane.iter().map(ToString::to_string).collect();
            assert_eq!(
                vyber(&pozadovane, &dostupne),
                ocekavano
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                "{popis}"
            );
        }
    }
}
