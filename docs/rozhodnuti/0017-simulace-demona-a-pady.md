# 0017: Simulace celého démona, pády a výpadky

Datum: 2026-11-15 · Milník 2 (bez Apple účtu) · Navazuje na 0011, 0012 a 0016

## Rozhodnutí

`crates/ikeloud-daemon/tests/simulation.rs` pouští skutečný běh démona
(skener, plánovač, bezpečnostní vrstva, vykonavatel, lokální akce, databáze)
nad skutečnými soubory na disku proti falešnému iCloudu, v náhodných, ale
opakovatelných scénářích:

- uživatel pracuje na obou stranách: upravuje, ukládá novým souborem,
  vytváří, maže, přesouvá. Dělá to i **během běhu**, před každou operací se
  serverem, a občas upraví **právě ten soubor, který se stahuje**;
- **výpadek serveru** uprostřed běhu;
- **pád démona** po provedení akcí a před uložením společného stavu. Běh
  skončí po první části (`start_run`) a druhá část (`ExecutedRun::finish`)
  se nezavolá.

Simulace kontroluje tyto vlastnosti:

- **žádná ztráta:** obsah, který synchronizace zničí, musí být jinde.
  Výjimkou je jen to, co zničil sám uživatel. Počítá se i obsah, který
  uživatel vytvořil během běhu;
- **do koše nebo historie jen synchronizovaný obsah:** co synchronizace
  přesune do koše nebo historie, muselo někdy být na obou stranách;
- **shoda:** po ustálení jsou obě strany stejné a další běh nic nedělá;
- **žádné zbytečné kopie:** po ustálení není stejný obsah na jedné straně
  dvakrát. Každý obsah vzniká jen jednou, takže dvojí výskyt znamená kopii,
  kterou synchronizace udělala zbytečně.

V CI běží 200 seedů za pár sekund; `SIM_SEEDS=5000` pro hlubší hledání.
Při 5 000 seedech proběhne zhruba 47 tisíc běhů, 5,5 tisíce výpadků,
5,5 tisíce pádů a 14 tisíc zásahů uživatele během běhů.

## Co simulace našla (opraveno)

1. **Pád po akcích zapomněl, co akce dokázaly.** Další běh pak bral soubory
   z rozpracovaného běhu jako „nové na obou stranách“ a dělal z nich
   konfliktní kopie. **Oprava:** každá provedená akce se hned zapíše do
   databáze (`StateDb::record`), a to záznam do deníku i přenos, který
   dokládá shodu stran. Schéma má verzi 2 s tabulkou `pending` a migrací
   z verze 1. Na konci běhu se čekající přenosy promítnou do společného
   stavu a smažou.
2. **Čekající přenosy se uplatnily až na konci dalšího běhu**, takže jeho
   plán je ještě neznal. **Oprava:** po pádu se čekající přenosy promítnou
   do společného stavu hned na začátku běhu, před plánem.
3. **Nový dokument, který uživatel hned po nahrání přesunul**, dostal ve
   společném stavu cestu ze serveru. Další běh pak usoudil, že soubor
   přesunul uživatel lokálně, a vrátil ho zpět. **Oprava:** přenos nese
   cestu, kde na serveru proběhl (`Transfer::path`), a u nového dokumentu
   rozhoduje ona.
4. **Dopočet stavu serveru po výpadku neznal verze, které stáhl.** Uživatel
   mezitím soubor na iCloudu změnil a přenos se pak nedal doložit.
   **Oprava:** dopočet bere verzi ze stažení.
5. **Položka společného stavu si držela cestu, kterou žádná její strana
   nedrží**, i když se obsah změnil nebo jedna strana zmizela. Jiný
   dokument si pak tu cestu vzal a vznikla kolize. **Oprava:** `next_base`
   ukládá vždy cestu, kterou drží živá strana, nezávisle na obsahu
   (cesta je samostatná složka jako v `NewBase` modelu TLA+).

Zbývá okno v řádu mikrosekund mezi dokončením akce a jejím zápisem do
databáze. Kdyby démon spadl právě v něm, další běh může jeden soubor vidět
jako nový na obou stranách a udělat z něj konfliktní kopii. Data se
neztratí; bez kontrolního součtu ze serveru (otázka milníku 0) to nejde
poznat.

## Ověření simulace ručními mutacemi

Simulace odhalí vrácení každé z těchto chyb:

| Mutace | Výsledek |
|---|---|
| stažení přes soubor bez kontroly verze a bez historie | ztráta obsahu (seed 29) |
| stažení přes soubor bez kontroly verze (s historií) | do historie jde nesynchronizovaný obsah (seed 29) |
| koš bez kontroly verze | do koše jde nesynchronizovaný obsah (seed 284) |
| bez dopočtu stavu serveru po výpadku | zbytečná kopie (seed 15) |
| bez obnovy čekajících přenosů po pádu | zbytečná kopie (seed 1) |
| akce se nezapisují hned | zbytečná kopie (seed 0) |

Při psaní simulace se dvakrát ukázalo, že sama něco nehlídala, a obojí je
opravené. Uživatel zasahoval jen před zápisy, ne před stahováním. Kontrola
ztrát počítala jen obsah ze začátku běhu.

**Známá mez:** přesun bez kontroly, že jde o týž soubor, simulace neodhalí.
Následek je špatně umístěný soubor, který další běh srovná, ne ztráta dat.
Kontrolu přesunu proto hlídají jen testy `LocalDisk`.
