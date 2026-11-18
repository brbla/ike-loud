# 0027: Synchronizace složek

Datum: 2026-11-18 · Stav: přijato (bod 3 ze šesti bodů vlastníka)

## Problém

Složky dnes vznikají jen jako vedlejší účinek souborů: na iCloudu se
založí před nahráním souboru, lokálně při stažení. Prázdná složka se
nepřenese, a smazaná složka zůstane na druhé straně prázdná (0023:
„známá mezera“). Uživatel smaže složku na Linuxu a na iPhonu ji dál vidí.

## Rozhodnutí

Složky mají vlastní třícestné porovnání jako soubory: místní složky ze
skenu (L), složky z výpisu iCloudu (R) a společný stav složek (B, nová
tabulka `base_folders`, klíčem normalizované jméno). Pro každou složku:

| L | R | B | Akce |
|---|---|---|------|
| ano | ano | – | nic (zapíše se do B) |
| ano | ne | ne | založit na iCloudu |
| ne | ano | ne | založit lokálně |
| ano | ne | ano | smazána na iCloudu → smazat lokálně, **jen prázdnou** |
| ne | ano | ano | smazána na Linuxu → do koše iCloudu, **jen prázdnou** |

Obsah složek rozhodují dál soubory podle zavedených pravidel: smazání
složky s obsahem se přenese tak, že se nejdřív přenesou smazání souborů
a složka se smaže v dalším běhu, až je prázdná. Soubor změněný na druhé
straně se podle pravidel souborů obnoví, složka pak prázdná není a
zůstane (iCloud vyhrává, obsah se neztratí).

„Prázdná“ znamená:

- **lokálně:** složka se maže voláním `rmdir`, které uspěje jen na
  opravdu prázdné složce (ani nesynchronizované soubory, ani skryté).
  Neprázdnost z principu nejde přehlédnout. Podsložky smazané z téhož
  důvodu se mažou od nejhlubší.
- **na iCloudu:** ve výpisu pod ní není žádný soubor a všechny její
  podsložky jsou také smazané lokálně. Do koše jdou od nejhlubší a
  těsně před každou démon složku na iCloudu znovu vypíše: cokoli v ní
  (i soubor přidaný po výpisu běhu) smazání zastaví. Koš je podmíněný
  verzí z tohoto čerstvého výpisu. Zda se verze složky mění s obsahem,
  nevíme, proto nestačí verze z výpisu běhu (našla to simulace démona:
  soubor přidaný během běhu odešel se složkou do koše).

Složka, kterou nejde smazat (není prázdná), zůstává v B, takže se
rozhodnutí v dalším běhu zopakuje, dokud se nevyrovná obsah. Složka
chybějící na obou stranách z B vypadne.

## Přechod ze starší databáze

Databáze z doby před synchronizací složek (schéma v4 přidá tabulky
`base_folders` a `folder_sync`) složky ještě nezná. Složka jen na jedné
straně tam nemusí být nová: může to být složka dřív smazaná na druhé
straně, která zůstala prázdná (známá mezera z 0023). První běh proto
složky jen zapíše (ty na obou stranách) a nic nezakládá ani nemaže; od
dalšího běhu platí tabulka výše. Nová synchronizace bez společného stavu
nemá co chránit a složky synchronizuje hned.

## Bezpečnost

- Nové akce jdou přes safety vrstvu jako ostatní: `CreateLocalFolder`,
  `RemoveLocalFolder`, `TrashRemoteFolder`. Smazání složek se počítají
  do brzdy hromadných změn. V režimu jen čtení se zápis na iCloud
  odloží. Chybějící nebo prázdná místní složka zastaví běh jako dosud.
- Místní složky se mažou bez koše: jsou prázdné, není co uchovat.
- Jména, která se nesynchronizují jako soubory (dočasné soubory editorů,
  `.part`, `.swp`, `.DS_Store`), se nesynchronizují ani jako složky.

Modely TLA+ složky nemodelují. Pravidla jsou množinová a každé mazání
je chráněné prázdností na straně, kde se maže. Ověřuje je simulace
démona s kroky uživatele „založ složku“ a „smaž
prázdnou složku“ na obou stranách.
