# 0009: Stav synchronizace v SQLite

Datum: 2026-11-15 · Milník 0 (předem z milníku 1, nepotřebuje účet)

## Rozhodnutí

Nový crate `ikeloud-state` ukládá poslední společný stav každého souboru
a deník akcí do SQLite. Je jediným obalem knihovny `rusqlite`; její typy
z crate neodcházejí. Plánovač (`ikeloud-sync`) zůstává čistá logika bez I/O.

- **Jedna transakce na běh.** `StateDb::commit` zapíše změny společného
  stavu i záznamy deníku najednou: buď všechno, nebo nic. Po pádu démon
  pokračuje od posledního potvrzeného stavu.
- **WAL a `synchronous=FULL`.** Potvrzená transakce přežije i výpadek proudu.
- **Klíč podle normalizovaného názvu** (NFC, malá písmena), stejně jako
  plánovač: „Foto.jpg“ a „foto.jpg“ jsou jeden záznam.
- **Kontroly v databázi** (`CHECK`, tabulky `STRICT`): prázdná cesta, otisk
  nebo etag se neuloží. Test ověřuje, že chyba uprostřed commitu neuloží
  ani dřívější změny téhož commitu.
- **Verze schématu** v `PRAGMA user_version`. Databázi zapsanou novější
  verzí aplikace starší verze odmítne otevřít a nesáhne na ni.
- **Deník** má stálé názvy druhů záznamu (`uploaded`, `moved-local`…);
  test hlídá, že se nezmění, protože jsou v databázích uživatelů. Čas
  dodává volající, modul hodiny nečte.

## Přibalená SQLite a zranitelnost

Zadání chce SQLite přibalenou v pevné verzi (`bundled`). Nejdřív jsem zvolil
`rusqlite` 0.40.2 jako starší a usazenou verzi. `cargo deny` ale našel
RUSTSEC-2026-0459 (CVE-2026-119375): SQLite před 3.54.0 může vrátit
neinicializovanou paměť, když otevře databázi s podvrženým souborem `-wal`.
Projekt teď používá `rusqlite` 0.41.1 se SQLite 3.54.1, kde je chyba
opravená. Kontrola zranitelností v CI tedy funguje, jak má.

## Dodavatelský řetězec

Nové závislosti: `rusqlite`, `libsqlite3-sys` (zdrojový kód SQLite v C),
`cc`, `pkg-config` a drobné pomocné knihovny. Všechny jsou ve výjimkách
`cargo vet` se zdůvodněním. Audit `libsqlite3-sys` znamená hlavně ověřit,
že přibalený zdrojový kód SQLite odpovídá oficiálnímu vydání 3.54.1.
