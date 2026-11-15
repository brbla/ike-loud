# 0006: Přesuny a přejmenování v TLA+ modelu

Datum: 2026-11-15 · Milník 0

## Výsledek

Model `tla/SyncMoves.tla` rozšiřuje `Sync.tla` o identitu souboru (inode,
ID dokumentu) a cestu. Synchronizace přenáší přesun jako přesun a kolize
cest řeší bez přepsání a bez ztráty, pokud server vynucuje etag.

| Konfigurace | Výsledek |
| --- | --- |
| `SyncMoves.cfg` (2 soubory, 3 cesty, 3 změny, bez pádu) | prošlo, 27 242 stavů; běží v CI |
| `SyncMoves_big.cfg` (3 identity, 1 pád) | viz níže |

Kontrolované vlastnosti:

- `NoLoss`: synchronizace nezničí obsah, který není nikde jinde.
- `UniquePaths`: na žádné straně nejsou dva soubory na jedné cestě.
- `PureMovesKeepNothing`: když uživatel jen přesouvá, nic neskončí v koši,
  v historii ani v konfliktní kopii.
- `Converge`: po ustálení jsou obě strany stejné včetně cest.

Kontrola, že model není bezzubý: přesun ze serveru bez odsunutí lokálního
souboru a lokální přesun bez ustoupení porušily `UniquePaths`.

## Pravidla, která model ověřil

- Párování podle identity; obsah a cesta se porovnávají zvlášť. Nejdřív se
  řeší obsah a existence (tabulka ze zadání), potom cesta.
- Přesun na obou stranách na různá místa: vyhrává cesta z iCloudu.
- Kolize cest, iCloud vyhrává a na serveru se nikdy nic neodsouvá:
  - přesun nebo nový soubor ze serveru míří na lokálně obsazenou cestu:
    lokální soubor, který tam stojí, dostane název konfliktní kopie;
  - lokální přesun nebo nový soubor míří na cestu obsazenou na serveru:
    lokální soubor dostane název konfliktní kopie.
- Stejný obsah na obou stranách na různých cestách (bez společného stavu)
  je přesun: lokální soubor se přesune na cestu z iCloudu.

## Co model našel

První verze se zacyklila: soubor se stejným obsahem na obou stranách, ale
na různých cestách, se donekonečna „přijímal“ a společný stav se nikdy
neuložil. Oprava je poslední pravidlo výše.

## Známé chování

Výměna názvů dvou souborů na Linuxu (a↔b) skončí tak, že jeden soubor
dostane název konfliktní kopie: lokální přesun na cestu, kterou na serveru
ještě drží druhý soubor, ustoupí. Data se neztratí a obě strany se shodnou.
Výměna názvů na iPhonu se přenese správně (lokální soubor se odsune
a pak přesune). Pokud se lokální výměna ukáže jako častá, řešení je
dočasný název i na serveru; model ho pak musí pokrýt.

## Co dál

Plánovač v `ikeloud-sync` zatím přesuny nepozná (0004). Až je bude umět,
musí odpovídat tomuto modelu.
