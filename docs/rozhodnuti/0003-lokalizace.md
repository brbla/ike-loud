# 0003: Lokalizace přes klíče a soubory Fluent

Datum: 2026-11-15 · Milník 0

## Rozhodnutí

Kód nikde nedrží text pro uživatele. Místo textu vrací typ (například
`Zamitnuti::HromadnaZmenaCekaNaPotvrzeni`) a crate `ikeloud-i18n` z něj
udělá `Zprava`, tedy klíč a argumenty. Text ke klíči je v souboru
`i18n/<jazyk>/ikeloud.ftl` ve formátu [Fluent](https://projectfluent.org/).

- Čeština a angličtina jsou zabudované v programu.
- Nový jazyk = nová složka `i18n/<kód>/ikeloud.ftl`. Program ji načte
  ze složky s překlady (`Preklady::s_adresarem`) bez nového sestavení.
  Pokud chce jazyk dostat do programu natrvalo, přidá se do `ZABUDOVANE`.
- Chybějící text se vezme z angličtiny, chybějící klíč se ukáže jako klíč.
- Rozbitý soubor jazyka, který si uživatel nevybral, aplikaci nezastaví.
- Jazyk se vybírá jako u gettextu: `LANGUAGE`, pak `LC_ALL`, `LC_MESSAGES`,
  `LANG`; `cs_CZ.UTF-8` vybere `cs`.

## Proč Fluent

Prostý soubor `klíč = text` nezvládne tvary množného čísla. Čeština jich
má tři („1 soubor, 2 soubory, 5 souborů“) a jiné jazyky ještě víc. Fluent
je vybírá podle pravidel CLDR pro každý jazyk a překladatel je píše přímo
do textu. Je to formát Mozilly, čitelný bez nástrojů. Gettext umí množná
čísla taky, ale má horší podporu v Rustu a vyžaduje překlad souborů .po.

## Hranice

- `ikeloud-safety` zůstává bez závislostí. Převod jejích typů na klíče je
  v `ikeloud-i18n/src/zpravy.rs`.
- `ikeloud-i18n` je jediný obal knihoven `fluent-bundle` a `unic-langid`
  (`dependency-policy.toml`), verze jsou připnuté přesně.

## Testy hlídají

- Všechny zabudované jazyky mají stejnou sadu klíčů.
- Každá varianta, kterou může uživatel vidět, má v každém jazyce text bez
  chyby a bez náhrady z jiného jazyka. V souborech nejsou nepoužité klíče.
- České a anglické tvary množného čísla, formát velikostí („1,5 GB“
  / „1.5 GB“), vnořené zprávy, nový jazyk ze složky, náhrada z angličtiny
  a chybné soubory.

## Dodavatelský řetězec

Fluent je první cizí závislost, takže se zapnul vendoring (`vendor/`,
`.cargo/config.toml`) a `cargo vet` podle zadání. CI sestavuje s
`--locked --offline`, takže použije jen uložené zdroje.

Po importu veřejných auditů (Mozilla, Google, Bytecode Alliance) je 7
knihoven prověřených a 18 ve výjimkách (`supply-chain/config.toml`).
Výjimky se budou zmenšovat vlastními audity; nová závislost už výjimku
nedostane bez zdůvodnění.
