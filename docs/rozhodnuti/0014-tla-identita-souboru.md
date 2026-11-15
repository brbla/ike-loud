# 0014: TLA+ model identity lokálního souboru

Datum: 2026-11-15 · Navazuje na 0006 a 0012

## Proč

Modely `Sync.tla` a `SyncMoves.tla` znají identitu každého souboru přesně.
Skutečný engine ji nezná: vidí inode, čas vzniku (když ho souborový systém
zapisuje), cestu a obsah. Linux přiděluje inode smazaného souboru dalšímu
novému souboru. Kritická chyba z revize (0012) vznikla právě tady a modely ji
nemohly najít.

## Rozhodnutí

`tla/SyncIdentity.tla` odděluje **pravdu**, tedy který soubor uživatel myslí,
od toho, **co vidí engine**. Souborový systém v něm recykluje inody. Uživatel
mezi synchronizacemi upravuje, přesouvá, maže, vytváří a ukládá novým
souborem (editor zapíše nový soubor a přejmenuje ho přes starý). Engine
páruje podle pravidel z `crates/ikeloud-sync/src/pairing.rs`:

1. podle identity: soubor se stejným inode, pokud je to týž soubor
   (stejný čas vzniku; bez času vzniku si ponechal cestu nebo obsah);
2. jinak podle místa: soubor přesně tam, kde byl lokální soubor položky,
   o kterém žádná položka neví, že by mohl být její.

**Vlastnost `NoForeignPairing`:** dokument se nikdy nespáruje s jiným
souborem než se svým. Výjimkou je soubor, který podle zadání svůj soubor
nahrazuje: stojí přesně tam, kde byl, a není (pokud to jde poznat) souborem
jiného dokumentu. Kontroluje se každé spárování, ne jen nahrání. Kromě toho
model hlídá, že žádný soubor nepatří dvěma dokumentům.

Konfigurace:

| | Čas vzniku | Výsledek |
|---|---|---|
| `SyncIdentity.cfg` | ano | projde, 56 tisíc stavů, 40 s |
| `SyncIdentityNoBirth.cfg` | ne | projde, 57 tisíc stavů, 48 s |
| `SyncIdentityInodeOnly.cfg` | chyba z revize: párování jen podle inode | **musí selhat** |

CI kontroluje i třetí konfiguraci. Když model chybu z revize přestane
nacházet, přestal něco hlídat, a build selže.

## Ověření modelu ručními mutacemi

Model najde každou z těchto chyb v pravidlech:

- párování jen podle inode, tedy chyba z revize. Protipříklad: soubor 1 se
  smaže, soubor 2 se uloží novým souborem, dostane inode 1 a spáruje se
  s dokumentem 1;
- uložení novým souborem bez podmínky „o souboru nikdo jiný neví“
  (obě varianty);
- bez času vzniku párování podle inode bez podmínky „stejná cesta nebo
  obsah“.

Při psaní modelu dvakrát selhala i samotná kontrola, a pokaždé to byla chyba
modelu, ne kódu:

- po legitimním nahrazení zůstala dokumentu stará „pravda“, takže každá další
  úprava vypadala jako cizí přepsání;
- výjimka pro nahrazení byla příliš široká a omlouvala i soubor jiného
  dokumentu. Mutace se pak neprojevila. Teprve po zúžení výjimky ji model
  chytil.

## Hranice

Kolo synchronizace je v modelu nedělitelné a server je jen tam, kam se
naposledy nahrálo. Souběh uživatele s prováděním plánu, přesuny na serveru
a cesty, které se rozejdou (`local_path`, chyby 2 a 3 z 0012), pokrývá
deterministická simulace na skutečném kódu, ne tento model.
