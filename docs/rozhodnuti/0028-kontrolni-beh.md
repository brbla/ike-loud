# 0028: Kontrolní běh

Datum: 2026-11-19 · Stav: přijato (milník 3)

## Problém

Zadání chce „kontrolní běh na pozadí: porovnání kontrolních součtů
lokálně a na serveru, hlášení rozdílů“ a brána milníku 3 počítá
„30 dní zápisů bez neshody v kontrolním běhu“. Synchronizace sama
neshodu nenajde: soubor, který se od posledního společného stavu
nezměnil ani lokálně (stejný hash), ani na iCloudu (stejná verze), pro
ni je v pořádku, i když obsah obou stran není stejný (poškozené nahrání,
chyba na straně iCloudu, chyba v našem kódu, tichá změna na disku bez
nového času).

Rozhodnutí 0026 zjistilo, že iCloud u každého souboru uvádí
`fileChecksum` (`01` + 20 bajtů), ale jeho výpočet jsme neznali: „není
to prostý SHA-1 ani SHA-256 souboru“. Bez něj by šlo porovnávat jen
velikost, nebo obsah znovu stahovat.

## Zjištění (naostro, testovací účet)

Součet je SHA-1 obsahu s pevnou předponou, jak ji známe z MMCS:

```
fileChecksum = base64( 0x01 ‖ SHA-1( "com.apple.XattrObjectSalt\0com.apple.DataObjectSalt\0" ‖ obsah ) )
```

Ověřeno offline na všech 1 466 synchronizovaných souborech testovacího
účtu (prázdný soubor až 31,7 MB, text, fotky, binární data): shoda
u všech, žádná výjimka. Tři z nich jsou známé odpovědi v testech
(`ikeloud-hash/tests/vectors.rs`). U větších souborů může iCloud
počítat jinak (dělení na kousky); zatím jsme to neviděli. Kdyby to tak
bylo, kontrola ohlásí rozdíl, nic nezmění, a uvidíme to v deníku.

## Rozhodnutí

1. **Co se porovnává.** Soubor společného stavu, u kterého iCloud ve
   výpisu uvádí verzi společného stavu a známe její součet (z CloudKitu
   nebo z odpovědi na nahrání). Lokální soubor se přečte jednou a spočítá
   se SHA-256 i součet iCloudu (`ikeloud_hash::content_sums`).
   - Lokální obsah jiný než ve společném stavu: změna, kterou přenese
     synchronizace. Kontrola ho přeskočí.
   - Soubor zmizel, je to jiný soubor (inode) nebo se během čtení
     změnil: přeskočí se.
   - Jinak: součet sedí → v pořádku; nesedí → **neshoda**.
2. **Jen čte a hlásí.** Neshoda se zapíše do deníku (`check-differs`),
   do stavu pro GUI a do logu služby; GUI pošle oznámení. Nic se
   nepřepisuje, na disku ani na iCloudu: která strana je správná, nevíme.
   Kontrola proto nepotřebuje bezpečnostní vrstvu. Stejná neshoda (stejná
   verze, stejný obsah) se hlásí jednou; nová verze, která zase nesedí, je
   nové hlášení.
3. **Kdy.** Po klidném běhu (nic neprovedeno, nic se nezapisuje), po
   částech: nejvýš 1 GiB nebo 10 sekund, pak synchronizace pokračuje.
   Nejdřív soubory nikdy neověřené (nové a změněné verze, tedy každý zápis
   za pár minut), pak nejstarší výsledky. Každý soubor znovu po 7 dnech.
4. **Stav.** Tabulka `verified` (schéma v6): dokument, verze, hash, čas,
   výsledek. Výsledek platí, jen dokud společný stav má tutéž verzi a
   obsah; záznamy dokumentů, které ze společného stavu zmizely, se mažou.
   Výsledky i zápis do deníku jsou v jedné transakci.
5. **Bez součtů** (výpis procházením složek místo CloudKitu) se nedá
   ověřit nic; GUI pak o kontrole nic neříká.

## Co to zatím neumí

- Soubor změněný na disku tak, že sken nic nepozná (stejná velikost,
  časy i inode, např. vadný disk), má jiný hash než společný stav, a
  kontrola ho tedy bere jako lokální změnu a přeskočí. Najde ho až
  synchronizace po denním vymazání paměti hashů, a to jako změnu, kterou
  nahraje. Rozlišit vadný disk od úpravy by chtělo uložený čas a velikost
  ve společném stavu.
- Obsah na iCloudu ověřujeme jen jeho vlastním součtem, nestahujeme ho.

## Ověření

- `sha1` 0.11.0 (RustCrypto, stejná rodina a závislosti jako `sha2`),
  jen přes obal `ikeloud-hash`; přečtený rozdíl, výjimka v `cargo vet`.
- Testy celých běhů proti falešnému iCloudu, který teď počítá součet jako
  iCloud: vše sedí; jiný obsah pod stejnou verzí se ohlásí jednou a nic se
  nezmění; nová verze se ověří hned; neodeslaná lokální změna se nehlásí;
  části podle bajtů; zapomenutí smazaných; výpis bez součtů; nahrazený
  soubor.
- Smyčka démona: kontrola jen po klidném běhu.
- Simulace démona: po každém ustálení musí kontrola ověřit každý soubor
  (u světů s CloudKitem) a nenajít žádnou neshodu. 10 000 scénářů.
- Schéma v6: výsledky přežijí restart, uložení je atomické.
