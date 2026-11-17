# 0025: Rychlejší synchronizace (návrh)

Datum: 2026-11-17 · Stav: **návrh**, čeká na rozhodnutí vlastníka

## Co je pomalé (měřeno na testovacím účtu, 1 506 složek, 1 462 souborů)

| Operace | Čas |
| --- | --- |
| výpis 1 složky (`retrieveItemDetailsInFolders`) | 0,5 s |
| výpis celého stromu po jedné složce | ~20 min (před dávkami) |
| výpis 25 složek jedním dotazem | ~0,5 s; strom ~1–2 min |
| vytvoření 1 složky | 1,1–1,9 s |
| vytvoření 25 složek **jedním** dotazem | 28 s: server je zpracuje po jedné, dávka nepomáhá |
| 8 složek po jedné / 4 souběžně / 8 souběžně | 15,4 s / 7,8 s / polovina `503 RETRY` |
| nahrání malého souboru (3 dotazy) | 2,6–3,4 s |
| CloudKit `changes/zone`, celá zóna (1 091 záznamů) | 8 dotazů, 20 s |
| CloudKit `changes/zone` se sync tokenem, beze změn | **1 dotaz, 0 záznamů** |

Dnes každý běh (po místní změně a každých 5 minut) prochází celý strom na
iCloudu a zápisy jdou jeden po druhém. Při 1 500 složkách to znamená
desítky minut na výpis a hodiny na první nahrání.

## Odkud čerpáme

- **rclone** (`backend/iclouddrive`): `pacer` s exponenciálním čekáním
  při „zkus znovu“, souběžné přenosy (`--transfers 4`), mezipaměť ID složek
  (`dirCache`). Výpis má po jedné složce, stejně jako jsme měli my.
- **Nextcloud / ownCloud klient**: etag složky se mění, když se změní
  cokoli uvnitř; nezměněné podstromy se znovu nevypisují.
- **OneDrive pro Linux (abraunegg)**, **Dropbox**: „delta“ dotaz s tokenem
  vrací jen změny od posledně; plný výpis jen napoprvé a při ztrátě tokenu.
- **icloudpd** a rozbor iCloud Photos (robhooper.xyz, „syncToken“):
  soukromé CloudKit API (`ckdatabasews`) má `changes/zone` se sync
  tokenem; u Fotek srazilo synchronizaci ze ~75 dotazů na 1.
- **Syncthing**: hash obsahu rozhoduje, co se přenese; přejmenování se
  nepřenáší jako nový obsah.

## Návrh: tři kroky

### 1. Hned (malé, bezpečné)

- **`429/502/503/504` = dočasný výpadek** a opakování s rostoucím
  čekáním, ne „neznámá odpověď“. *Hotovo (tato změna).*
- **Souběžné zápisy a přenosy, nejvýš 4** (měřeno: 2× rychlejší, 8 už
  server odmítá). Jen akce, které na sobě nezávisí: složky předem a po
  úrovních, nahrávání a stahování různých souborů souběžně, přesuny a
  mazání dál po jedné v pořadí plánu. Každá akce si dál ověřuje své
  podmínky; deník se zapisuje po každé.
- **Čas v deníku = čas akce**, ne začátek běhu.

Odhad: první nahrání testovacích dat ~2× rychleji.

### 2. Delta z CloudKitu místo procházení stromu (velký zisk)

`POST {ckdatabasews}/database/1/com.apple.clouddocs/production/private/changes/zone`
se zónou `com.apple.CloudDocs`:

- záznamy `structure` (jméno v `encryptedBasename`, rodič v `parent`,
  přípona, časy, `basehash`) a `content` (velikost, `basehash`, obsah);
- **sync token**: další dotaz vrátí jen změny; beze změn 1 dotaz.

Démon si drží strom iCloudu v SQLite (záznamy podle ID, rodič, jméno,
etag/basehash) a po každém běhu jen dočte změny. Plný výpis přes
`drivews` zůstane: napoprvé, když token přestane platit, a jednou denně
jako kontrola, že strom z CloudKitu odpovídá skutečnosti (rozdíl = chyba
API, démon jen čte a hlásí; zadání: podivný výpis je chyba, ne smazání).

Rizika: neveřejné API (jako všechno, co používáme), formát záznamů
(smazání, přesuny, ADP s opravdu šifrovanými jmény) je potřeba ověřit
pokusy; `encryptedBasename` je bez ADP jen base64 jména. Model TLA+ a
simulace se nemění: jádro dostává stejný výpis, jen ho démon sestaví
jinak. Nová je jen vrstva „strom z delt“ a její test proti plnému výpisu.

Odhad: kontrola změn na iCloudu z minut na 1 dotaz; běh po místní změně
bez procházení stromu.

### 3. Hash obsahu z iCloudu (`basehash`)

Když se změní etag, ale ne `basehash` (přejmenování, přesun), nic se
nestahuje. Při prvním spuštění nad složkou se stejnými soubory (např.
po přeinstalaci) se dají soubory spárovat bez stahování, pokud se podaří
zjistit, jak `basehash` vzniká (pokus: hash známého souboru).

## Pořadí

1 → 2 → 3. Krok 1 je malý a pomůže hned; krok 2 je nejvíc práce
(odhadem 1–2 dny včetně pokusů a testů) a přinese nejvíc; krok 3 stojí
na kroku 2.
