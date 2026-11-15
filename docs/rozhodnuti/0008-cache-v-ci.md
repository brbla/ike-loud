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
- Nástroje z cache CI ověří: jejich verze musí odpovídat připnutým.
- `CARGO_INCREMENTAL=0`: v CI se nepoužívá inkrementální překlad.
- Akce GitHubu jsou připnuté na commit (`actions/checkout` v7.0.1,
  `actions/cache` v6.1.0), ne na pohyblivý tag.
