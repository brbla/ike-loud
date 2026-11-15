# 0010: Plánovač umí přesuny (podle modelu SyncMoves)

Datum: 2026-11-15 · Milník 0

## Rozhodnutí

`ikeloud_sync::build_plan` páruje soubory podle identity a přesun přenáší
jako přesun. Pravidla odpovídají modelu `tla/SyncMoves.tla` (rozhodnutí 0006).

- **Identita:** lokálně inode, na serveru ID dokumentu; společný stav (SQLite)
  drží obojí a ví, který lokální soubor je který dokument.
- **Uložení s novým inode.** Editory jako LibreOffice, vim nebo gedit ukládají
  tak, že zapíšou nový soubor a přejmenují ho přes starý; soubor tím dostane
  nový inode. Když inode ze společného stavu zmizí a na jeho cestě stojí
  soubor s neznámým inode, je to tentýž soubor. Bez toho by každé uložení
  dokumentu skončilo konfliktní kopií. Soubor s inode, který společný stav
  zná pod jiným jménem, se takto nepřiřadí.
- **Nejdřív obsah, pak cesta**, jako v modelu: přesunutý a upravený soubor se
  v jednom běhu nahraje, v dalším přesune.
- **Kolize cest:** iCloud vyhrává; na serveru se nic neodsouvá. Lokální soubor,
  který stojí v cestě, dostane název konfliktní kopie.
- **Přejmenování na jinou velikost písmen** („foto.jpg“ → „Foto.jpg“) je přesun
  téhož dokumentu; bezpečnostní vrstva ho dřív chybně blokovala jako kolizi
  názvů a měla na to i chybný test. Opraveno.

## Co našel simulátor disku v testech

Testy každý plán přehrají na simulovaném lokálním disku a selžou, když
přesun bere neexistující soubor nebo přepisuje jiný, nebo když stažení
míří na obsazené místo bez povolení přepsat. Hned při prvním běhu našel
tři chyby, všechny opravené:

1. Soubor odsunutý kvůli kolizi mohl plánovač zpracovat podruhé pod starou
   cestou, kde už stál jiný soubor, a přesunout ten. Teď všechny lokální
   přesuny jdou přes jedno místo, které vede aktuální cestu každého souboru.
2. U kolize názvů na Linuxu (`a.txt` a `A.txt`) se dokument ze serveru stáhl
   přes jeden z nich. Teď se kolidující názvy přeskočí na obou stranách.
3. Odsouvaný soubor se hledal pod cestou ze serveru (`b`) místo skutečné
   lokální (`B`).

## Otevřené otázky pro milník 0

- **Mění se etag při přejmenování?** Pokud ano, přejmenování na iPhonu bude
  vypadat i jako změna obsahu: soubor se zbytečně stáhne, a pokud byl zároveň
  upraven na Linuxu, vznikne konfliktní kopie navíc. Bezpečné, ale zbytečné.
- **Zůstává ID dokumentu stejné při přesunu mezi složkami?** Model to
  předpokládá.

## Známá omezení

- Výměna názvů (a↔b) a kolize se dokončí až v dalším běhu; nic se při tom
  nepřepíše ani nesmaže (test `swap_on_icloud`).
- Akce `MoveLocal` nenese inode, takže vykonavatel ověří jen existenci zdrojové
  cesty. Až vznikne vykonavatel, má před přesunem ověřit i inode.
