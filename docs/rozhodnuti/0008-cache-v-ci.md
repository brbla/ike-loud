# 0008: Cache v CI jen pro to, co se nezměnilo

Datum: 2026-11-15 · rozhodnutí vlastníka projektu

## Rozhodnutí

CI používá cache jen pro věci, které se nezměnily. Jakmile se změní cokoli,
na čem cachovaný obsah závisí, cache se nepoužije a vše se sestaví znovu.
Náš vlastní kód se testuje vždy v aktuální podobě.

| Co | Klíč cache (změna čehokoli = nová cache) |
| --- | --- |
| přeložené závislosti z `vendor/` (`target/`) | `Cargo.lock`, všechny `Cargo.toml`, `.cargo/config.toml`, `rust-toolchain.toml`, kontrolní součty všech knihoven ve `vendor/`, verze `rustc`, `RUSTFLAGS` |
| `cargo-deny`, `cargo-audit`, `cargo-vet` | jejich připnuté verze a verze `rustc`, kterou se překládají |
| Java a TLC | otisk `scripts/tlc.sh` (obsahuje verze a kontrolní součty) |

- **Náš kód se do cache nedostane.** Skript `scripts/ci-clean-workspace.sh`
  smaže artefakty našich crates hned po obnovení cache i před jejím uložením.
  Cargo je proto při každém běhu přeloží znovu; ověřeno lokálně (po vyčištění
  se překládá jen 7 našich crates, 40 závislostí zůstane).
- **Jen přesné klíče.** Žádné `restore-keys`, takže se nikdy neobnoví starší
  cache s jiným obsahem.
- **Výsledky se necachují.** Testy, clippy, TLC, `cargo deny`, `cargo audit`
  a `cargo vet` běží pokaždé. Databáze zranitelností se stahuje pokaždé.
- **Pevný čas změny souborů ve `vendor/`.** Cargo u závislostí s build
  skriptem pozná změnu podle času změny souborů a čerstvý `checkout` má
  všechny soubory nové. Bez toho se i při zásahu cache znovu překládalo
  21 závislostí (ověřeno lokálně i v CI). Obsah `vendor/` hlídá klíč cache,
  takže pevný čas je tam bezpečný; naše zdrojové soubory si skutečné časy
  nechávají.
- Nástroje z cache CI ověří: jejich verze musí odpovídat připnutým.
- `CARGO_INCREMENTAL=0`: v CI se nepoužívá inkrementální překlad.
- Akce GitHubu jsou připnuté na commit (`actions/checkout` v7.0.1,
  `actions/cache` v6.1.0), ne na pohyblivý tag.

## Doplněk: otisk vstupů jako testovaná součást (2026-11-15)

Na návrh vlastníka projektu je invalidace cache součástí testů a má vlastní
skill (`.claude/skills/ci-cache/SKILL.md`).

- **Otisk počítá jeden skript**, `scripts/ci-cache-key.sh`; CI klíč nepočítá
  samo. Vstupy: obraz runneru, celé `rustc -vV`, `cargo -V`, verze C překladače
  a `ar`, všechny proměnné prostředí podle vzorů (`CARGO_*`, `RUST*`, `CC*`,
  `CFLAGS*`, `TARGET_*`, `PKG_CONFIG*`, `SQLITE*` …), `Cargo.lock`, manifesty,
  `.cargo/config.toml`, `rust-toolchain.toml` a součty všech knihoven ve
  `vendor/`.
- **Otisk je testovaný** (`scripts/test-cache-key.sh`, běží v CI): změna
  každého vstupu musí otisk změnit; změna našeho kódu, dokumentace nebo
  nesouvisející proměnné ho změnit nesmí. Test sám najde všechny proměnné,
  které sledují build skripty ve `vendor/`.
- **Čerstvost se kontroluje v každém běhu** (`scripts/check-fresh.py`): CI
  selže, pokud se některý náš crate nepřeložil z aktuálních zdrojů.
- Obě kontroly jsou ověřené i opačně: po vyřazení vstupu z otisku test
  selže, bez vyčištění našich crates kontrola čerstvosti selže.

Proč ne „otisk našich zdrojáků“: výsledek překladu závisí i na překladači,
jeho přepínačích, systémovém C překladači a proměnných prostředí. Původní
klíč neobsahoval verzi `gcc` (kterou se překládá přibalená SQLite) ani
proměnné jako `SQLITE_MAX_VARIABLE_NUMBER`, které čtou build skripty; obojí
teď otisk obsahuje a test to hlídá. Náš vlastní kód se necachuje vůbec,
protože jeho překlad trvá sekundy a spolehlivě čerstvý je jen bez cache.
