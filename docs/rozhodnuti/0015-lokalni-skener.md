# 0015: Lokální skener a crate pro hashe

Datum: 2026-11-15 · Milník 1 (bez Apple účtu)

## Rozhodnutí

**`ikeloud-local`** prochází synchronizovanou složku a každý běžný soubor
popíše tak, jak ho potřebuje plánovač:

- **identita:** zařízení, inode a čas vzniku. Čas vzniku dává `statx` přes
  standardní knihovnu (`Metadata::created`); kde ho souborový systém
  nezapisuje, je `None` a platí opatrná pravidla z 0012;
- **otisk obsahu:** SHA-256, čtený po blocích, takže velký soubor nezabere
  paměť;
- **jak dlouho se soubor nezměnil:** od pozdější z obou změn, obsahu (mtime)
  i metadat (ctime, třeba přejmenování). Čas v budoucnosti znamená „právě
  teď“.

Jen standardní knihovna, žádný `unsafe`.

**Soubor změněný během čtení** (jiná velikost, mtime nebo identita po
přečtení) dostane dobu ustálení nula. Jeho otisk může patřit napůl zapsanému
souboru a plánovač takový soubor nechá být (0012). Nenahraje se tedy
rozepsaná verze.

**Co se nesynchronizuje, se hlásí**, nic se nezahodí potichu:

- symbolický odkaz se nenásleduje, protože může vést ven ze složky;
- zařízení, roura a socket;
- název mimo UTF-8;
- položka, která během skenu zmizela nebo nejde přečíst.

**Chybějící, nebo nečitelná složka zastaví celý sken** (`ScanError`).
Prázdný výsledek by vypadal jako smazání všech souborů. Platí to i pro
nečitelnou podsložku: její soubory by jinak vypadaly smazané a poslaly by se
na iCloudu do koše. Synchronizace pak stojí, dokud uživatel složku
nezpřístupní. Jemnější chování („nech tuto podsložku na obou stranách
být“) může přijít později. Bezpečné je už teď.

**`ikeloud-hash`** je jediný obal knihoven `sha2` a `pbkdf2`. Používá ho
přihlášení (SRP) i skener. Pravidlo jednoho obalu na knihovnu by jinak
nedovolilo, aby `sha2` volaly dva crate. Testy jsou vektory ze standardů
(FIPS 180-2, RFC 7914).

## Ověřeno na skutečném souborovém systému

Testy v `crates/ikeloud-local/tests/scan.rs` spojují skener s plánovačem:

- přejmenování do složky je přesun na serveru, ne smazání a nové nahrání;
- uložení editorem („nový soubor a přejmenování přes starý“) je úprava;
- smazaný soubor jde do koše na iCloudu, i když vedle vzniknou nové
  soubory, a nic se nenahraje přes jeho dokument.

Jestli souborový systém inode smazaného souboru hned znovu přidělí, je jeho
volba: tmpfs v testech to neudělal. Recyklaci vyčerpávajícím způsobem
pokrývá model `SyncIdentity` (0014) a simulace.
