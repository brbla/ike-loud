# 0004: Plánovač synchronizace

Datum: 2026-11-15 · Milník 0 (předem z milníku 2, protože nepotřebuje účet)

## Rozhodnutí

`ikeloud_sync::sestavit_plan` porovná tři stavy a vrátí akce podle
rozhodovací tabulky ze zadání. Logika odpovídá akci `Plan` v `tla/Sync.tla`.

- Změnu na Linuxu pozná podle otisku obsahu, na iCloudu podle etagu,
  nikdy podle času.
- Soubory páruje podle klíče: NFC a malá písmena. Název pro server se bere
  ze serveru, když tam soubor je; nový soubor se nahraje v NFC.
- Soubory, které mají na stejné straně stejný klíč (`a.txt` a `A.txt` na
  Linuxu), se vynechají a ohlásí jako kolize. Nic se s nimi neděje, dokud
  je uživatel nepřejmenuje.
- Ignorované soubory (`.~lock.*#`, `*.swp`, `*.part`, `.DS_Store`)
  plánovač vůbec nevidí; `ikeloud-safety` je hlídá znovu.
- Konflikt (změna na obou stranách): lokální soubor se přejmenuje na
  konfliktní kopii, ta se nahraje jako nový soubor a verze z iCloudu se
  stáhne na původní místo. Nic se nepřepisuje.
- Název konfliktní kopie dodá volající přes `ikeloud-i18n`
  (klíč `konfliktni-kopie`), protože ho uživatel vidí. Když je název
  obsazený, zkusí se další pořadí.
- Chybějící složky na serveru se vytvoří před nahráním, mělčí dřív.

## Známá omezení

- **Nový soubor na obou stranách je vždy konflikt.** Bez kontrolního
  součtu ze serveru nejde poznat, že je obsah stejný. Jestli server součet
  vrací, je otázka milníku 0; pokud ano, shodný obsah se jen zapíše do
  společného stavu.
- **Přesuny a přejmenování** plánovač zatím pozná jen jako smazání
  a nový soubor. Zadání chce přesun podle inode a ID dokumentu; doplní se
  spolu s rozšířením TLA+ modelu. Do té doby brzda hromadných změn zachytí
  velký přesun jako hromadné mazání.
- Prázdné složky se nesynchronizují.
