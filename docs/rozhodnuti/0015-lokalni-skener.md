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

## Lokální akce (`LocalDisk`)

Provádí lokální akce plánu a těsně před každou ověří její předpoklad
(kontrakt `Effects`, 0012):

- **přesun** se nejdřív zkontroluje, že na zdrojové cestě je pořád týž
  soubor. Pak se vytvoří pevný odkaz na cíl a smaže původní jméno. Pevný
  odkaz existující cíl nikdy nepřepíše, takže obsazený cíl atomicky selže.
  Na souborových systémech bez pevných odkazů následuje přejmenování po
  kontrole, že cíl je volný;
- **smazání** jde do koše systému podle freedesktop.org specifikace (soubor
  a `.trashinfo`, volné jméno se rezervuje exkluzivním vytvořením info
  souboru), a jen když jde pořád o týž soubor se stejným obsahem: obsah se
  znovu zahashuje. Úprava provedená po sestavení plánu se nikdy nevyhodí;
- **stažení** se zapíše do dočasného souboru (`.ikeloud-….part`; skener ho
  ignoruje), zapíše se na disk (`sync_all`) a teprve potom:
  - na volné místo pevným odkazem, takže soubor, který se tam mezitím
    objevil, se nepřepíše;
  - přes existující soubor jen tehdy, když je to pořád verze z plánu.
    Ta se nejdřív uloží do lokální historie (pevný odkaz, nebo kopie mezi
    souborovými systémy) a pak se nahradí atomickým přejmenováním, takže
    na cestě je vždy celá stará, nebo celá nová verze.

Mezi kontrolou a činem zůstává okno v řádu mikrosekund. Záchrannou sítí pro
něj je lokální historie.

**Známá nepřesnost:** `DeletionDate` v koši má být v místním čase, ale
standardní knihovna časová pásma nezná, takže se zapisuje UTC. Správce
souborů může ukázat čas smazání o pár hodin vedle.

**Pokrytí** `ikeloud-local` a `ikeloud-hash` je 90 % řádků. Nepokryté jsou
chybové cesty: plný disk, souborový systém bez pevných odkazů, časy před
rokem 1970. Na ty je potřeba vkládání poruch disku, a proto tyto crate zatím
nejsou v bráně 95 %, která platí pro `ikeloud-safety` a `ikeloud-sync`.
