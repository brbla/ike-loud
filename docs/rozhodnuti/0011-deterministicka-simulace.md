# 0011: Deterministická simulace synchronizace

Datum: 2026-11-15 · Milník 0 (předem z milníku 2)

## Rozhodnutí

`crates/ikeloud-sync/tests/simulation.rs` pouští skutečný plánovač
a skutečnou bezpečnostní vrstvu proti simulovanému světu: lokální disk,
server a náhodné akce uživatele na obou stranách (úpravy, uložení s novým
inode, vytvoření, smazání, přesuny, názvy lišící se jen velikostí písmen).
Kontroluje vlastnosti z TLA+ modelů na skutečném kódu:

- **žádná ztráta:** krok synchronizace nikdy nezničí obsah, který není
  nikde jinde (na jedné ze stran, v koši, v historii, v konfliktní kopii);
- **jedinečné názvy na serveru;**
- **shoda:** když uživatel přestane, obě strany se srovnají a další plán je
  prázdný; výjimkou jsou soubory v kolizi názvů, které synchronizace
  záměrně nechává být.

Každý běh je deterministický podle seedu a chyba vypíše seed i celý průběh.
V CI běží 500 seedů v každé variantě; `SIM_SEEDS=10000` pro hlubší hledání.
Simulace hlídá i sama sebe: aspoň polovina běhů musí dojít ke kontrole
shody a vynechané soubory nesmí převážit. Při 10 000 seedech v každé
variantě dojde ke shodě 99,9 % běhů a porovná se přes 21 000 souborů.

Obě varianty běží zvlášť: server při přejmenování etag mění / nemění
(otázka pro milník 0).

## Co simulace našla

1. **Přesun souboru do kolize názvů ho smazal na iCloudu.** Soubor přejmenovaný
   lokálně na `A`, když vedle existovalo `a`, plánovač neviděl, a protože
   zmizel ze své cesty, poslal ho na iCloudu do koše. Obsah se neztratil
   (koš), ale uživatel nic nemazal. Opraveno: soubor v kolizi se nikdy
   netváří jako smazaný; test v `plan.rs`.
2. **Nekonečná smyčka konfliktních kopií** (dvakrát, z různých příčin). Pokaždé
   šlo o to, jak se po běhu aktualizuje společný stav:
   - společný stav se musí ukládat **po složkách** (obsah, když se obsah shoduje;
     cesta, když se cesta shoduje), jako `NewBase` v TLA+ modelu, ne jen celé
     shody;
   - aktualizace musí vycházet z toho, **co vykonavatel udělal**: které
     přenosy proběhly a jaký inode a etag z nich vzešel. Po konfliktu se
     verze z iCloudu stáhne na původní lokální cestu, zatímco dokument je na
     serveru jinde; dohledat ji jde jen podle provedeného přenosu;
   - párování musí používat stejná pravidla jako plánovač (normalizovaný název).

## Co z toho plyne pro vykonavatele

Logika aktualizace společného stavu teď žije v simulátoru jako oracle. Pro
produkt musí být **v `ikeloud-sync` vedle plánovače** a sdílet s ním pravidla
párování; vykonavatel jí předá, co udělal (přenosy, nové inody a etagy).
Dvě kopie těchto pravidel by se rozešly, simulace to ukázala dvakrát.
Je to první úkol, až vznikne vykonavatel.
