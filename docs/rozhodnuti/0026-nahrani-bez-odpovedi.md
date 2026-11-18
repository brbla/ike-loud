# 0026: Nahrání bez odpovědi

Datum: 2026-11-18 · Stav: přijato

## Problém

Při prvním nahrávání testovacích dat (1 462 souborů) vznikly přes noc
konfliktní kopie souborů, které se nikdo nedotkl. Žurnál ukázal vzorec:
běh skončil chybou (iCloud zpomaluje, `503`, vypršení spojení), další běh
po půlhodinovém čekání soubor přesunul na konfliktní kopii a stáhl
„novou“ verzi z iCloudu se stejným obsahem.

Nahrání má tři kroky: slot (`upload/web`), obsah, dokument
(`update/documents`). Když odpověď na třetí krok nedorazí, dokument na
iCloudu přesto může vzniknout. Démon o něm neví, další běh vidí soubor
nový na obou stranách a jádro podle zadání (iCloud vyhrává, bez
kontrolního součtu nelze obsah porovnat) udělá konfliktní kopii.

## Zjištění (naostro, testovací účet)

- Odpověď kroku 2 nese `singleFile.fileChecksum`; CloudKit má u záznamu
  obsahu totéž pole (`fileContent.value.fileChecksum`). Pro obsah známé
  fotky shodné (`AQxS5l6Q…`), bez soli: stejný obsah dává stejný součet.
  Není to prostý SHA-1 ani SHA-256 souboru.
- `document_id` ze slotu je i ID výsledného dokumentu (ve výpisu i v
  CloudKitu).

## Rozhodnutí

1. Selže-li krok 3 nejednoznačně (bez odpovědi, `5xx`, nečitelná
   odpověď), API vrátí `UploadUnconfirmed { document_id, checksum }`.
   Jasné odmítnutí (`409`, přihlášení) zůstává, jak bylo.
2. Démon to uloží do tabulky `unconfirmed` (schéma v3): cesta, kam se
   nahrávalo, dokument, místní soubor, hash nahraného obsahu, součet
   iCloudu, čas. Běh se zastaví jako při výpadku.
3. Další běh hned po výpisu: je-li ve výpisu (z CloudKitu) ten dokument
   se stejným součtem, nahrání se stalo. Záznam se v jedné transakci
   změní na zaznamenaný transfer (`pending`) a zápis do žurnálu, přesně
   jako by odpověď dorazila a démon pak spadl. Dál platí zavedená obnova
   po pádu: transfer jde do společného stavu před plánováním.
   - Transfer nese cestu, kam se nahrávalo, ne dnešní: přesun na
     iCloudu mezitím se tak pozná jako přesun (soubor se přesune i
     lokálně).
   - Dokument s jiným obsahem: nahrání se nestalo (nebo ho někdo
     přepsal), záznam se zahodí.
   - Dokument ve výpisu není: záznam čeká nejvýš den, pak se zahodí.
   - Výpis bez součtů (procházení složek místo CloudKitu) nic
     nerozhoduje: dopadne to jako dřív, konfliktní kopií.

Jádro, safety vrstva ani modely TLA+ se nemění: démon jen doplní důkaz,
který by jinak přinesla odpověď. Transfer platí, jen pokud se od něj
nezměnila žádná strana (pravidlo společného stavu). Soubor upravený
v minutě mezi ztracenou odpovědí a dalším během proto skončí jako
konfliktní kopie. Nic se neztratí.

## Ověření

- Testy celých běhů proti falešnému iCloudu: nový soubor, nová verze,
  přesun na iCloudu po ztracené odpovědi (bez konfliktní kopie), úprava
  po ztracené odpovědi a výpis bez součtů (konfliktní kopie, nic
  ztraceno).
- Simulace démona přidává ztracené odpovědi (každý pátý běh s poruchami)
  a výpisy bez součtů (každý osmý, ne když čeká nepotvrzené nahrání).
- Schéma v3: test, že záznamy přežijí restart a vyřízení je atomické.
