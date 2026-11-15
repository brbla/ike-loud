# 0012: Identita lokálního souboru, předpoklady akcí a nepřátelská simulace

Datum: 2026-11-15 · Milník 0 (předem z milníku 2) · Navazuje na 0011

Nezávislá revize našla v plánovači a bezpečnostní vrstvě chyby, které
simulace z 0011 nemohla odhalit, protože svět v ní byl příliš hodný. Tento
záznam popisuje opravy a jak je teď hlídá simulace.

## Rozhodnutí

### Identita lokálního souboru: zařízení, inode a čas vzniku

`LocalId { device, inode, birth_ns }` (v `ikeloud-safety`, protože ji nesou
akce). Linux přidělí inode smazaného souboru dalšímu novému souboru;
párování podle samotného inode by pak nový nesouvisející soubor nahrálo přes
dokument smazaného. Čas vzniku (`statx`) je od sebe odliší.

Na souborových systémech bez času vzniku se rozhoduje podle pravidla
v `crates/ikeloud-sync/src/pairing.rs`. Platí jen tam, kde to bez historie
rozhodnout nejde:

- soubor se stejným inode je týž soubor, jen když si ponechal cestu nebo
  obsah. Přejmenovaný a zároveň upravený soubor se bere jako nový: stará kopie
  jde do koše na iCloudu a nový se nahraje. Nic se neztratí, jen historie
  dokumentu;
- soubor, jehož inode zná jiná položka společného stavu, se nikdy nebere jako
  „uložený novým souborem a přejmenováním“. Nejasný případ nesmí přepsat
  dokument.

Plánovač i `next_base` párují **stejnou funkcí** (`LocalIndex::find`), takže
se nemohou rozejít.

### Kde stál lokální soubor: `local_path` ve společném stavu

Společný stav má vedle `path` (co naposledy platilo na obou stranách) také
`local_path`, tedy kde byl lokální soubor při poslední synchronizaci. Liší se,
dokud se lokální přesun nepřenese. Editor, který ukládá novým souborem
a přejmenováním, nechá nový soubor na `local_path`. Rozpoznání takového
uložení musí hledat tam a **přesně** (po NFC, ale s velikostí písmen),
`a.txt` vedle smazaného `A.txt` je jiný soubor.

Když jsou strany zrovna na různých místech, uloží `next_base` takovou cestu,
kterou soubor ještě drží: starou, dokud tam je soubor na serveru, jinak
lokální. Cestu, kterou nikdo nedrží, si může vzít jiný soubor a ten by se pak
vydával za tento.

### Předpoklady lokálních akcí

Server chrání etag. Lokální akce teď nesou, co plánovač viděl, a efekty to
ověří těsně před provedením:

| Akce | Předpoklad |
|---|---|
| `Upload`, `UploadConflictCopy`, `MoveLocal` | na cestě je pořád týž soubor (`LocalId`) |
| `TrashLocal`, `Download` přes lokální soubor | týž soubor **a** týž obsah (`LocalVersion`) |
| `Download` na volné místo | místo je pořád volné |

Úprava mezi plánem a provedením tak nikdy neskončí v koši nebo v historii,
akce se přeskočí a další běh z ní udělá konfliktní kopii.

### Soubory, které synchronizace nechává být

V jednom běhu se nic neděje se soubory:

- v kolizi názvů (jako dosud);
- **které se ještě zapisují** (nezměněné kratší dobu než ustálení), a to na
  obou stranách. Dřív se odkládalo jen nahrání a konfliktní kopie mohla
  vzniknout ze souboru, do kterého editor zrovna zapisoval. Plánovač je hlásí
  v `Plan.busy`; bezpečnostní vrstva navíc odkládá i `UploadConflictCopy`;
- **přejmenovaných na název, který se nesynchronizuje**
  (`report.odt` → `report.odt.part`). Dřív se poslaly na iCloudu do koše.

### Název konfliktní kopie

Plánovač zkusí nejvýš 1 000 názvů (`MAX_COPY_NAME_ATTEMPTS`) a odmítne
název v jiné složce nebo ignorovaný. Překlad, který nepoužívá `$number`, dřív
zacyklil plánovač až do 4 miliard pokusů; teď se soubor nechá být a nahlásí
v `Plan.no_copy_name`.

### Potvrzení hromadné změny je seznam souborů

`MassChangeSummary.files` je seznam dotčených souborů: co se s každým stane
a v jaké verzi (etag nebo otisk obsahu). Uživatel tak vidí, co potvrzuje.
Potvrzení platí pro přesně tyto soubory v těchto verzích. Plán, který zničí
jen jejich část, projde; cokoli dalšího nebo jinou verzi potvrzovat znovu.
Dřív to byl otisk celého plánu včetně velikosti a doby ustálení, takže po
novém skenu potvrzení nikdy neplatilo.

### Typované chyby databáze stavu

`StateError::NameTaken`, `LocalFileTaken` a `InvalidValue` místo textu
z SQLite. Kontroluje je Rust v téže transakci; schéma totéž hlídá jako
pojistku.

### Sdílené pomocné funkce testů: `ikeloud-testkit`

Testy pěti crate stavěly soubory, položky a přenosy vlastními kopiemi
funkcí. Teď je dělá jeden crate. `dependency-policy.toml` ho značí
`test_only` a `scripts/check-deps.py` hlídá, že je jen dev závislost.

## Nepřátelská simulace

Simulace z 0011 teď:

- **recykluje inody** smazaných souborů, ve variantě s časem vzniku i bez něj;
- nechá **uživatele jednat během provádění plánu** (mezi akcemi);
- používá názvy v **NFD** (jak je zapíše Mac) i NFC;
- má soubory, **které se ještě zapisují**.

Kromě žádné ztráty, jedinečných názvů a shody kontroluje dvě nové vlastnosti:

- **odložit se smí jen synchronizovaný obsah:** co synchronizace přesune do
  koše nebo do historie, muselo někdy být na obou stranách. Změna, kterou
  druhá strana neviděla, musí zůstat živým souborem;
- **přepsat se smí jen verze téhož souboru:** každý obsah patří k jedné
  „linii“ (vznik, pak úpravy). Výjimkou je jen to, co žádný sken neodliší od
  uložení novým souborem a přejmenováním. Simulace to počítá podle pravidla
  ze zadání, ne kódem, který testuje.

Simulace hlídá i sama sebe: v každé variantě musí opravdu recyklovat inody,
uživatel musí zasahovat do běhů a některé akce musí selhat na předpokladu.
Při 5 000 seedech v každé ze tří variant dojde ke shodě 99,6–99,7 % běhů.
Inody se recyklují zhruba 7 000–9 000krát, uživatel zasáhne do běhu asi
10 000krát a 2 500–3 000 akcí se přeskočí kvůli změněnému předpokladu.

**Kontrola, že simulace chyby opravdu najde (ruční mutace):**

- párování jen podle inode (chyba z revize): selžou testy `next_base`
  a plánovače a všechny tři varianty simulace do seedu 114;
- efekty bez lokálních předpokladů: všechny varianty selžou u seedu 10
  (do historie se odložil obsah, který nikdy nebyl na serveru).

V CI běží 500 seedů v každé variantě. Kvůli tomu se `ikeloud-sync`,
`ikeloud-safety` a `unicode-normalization` i v debug buildu překládají
s optimalizací. Simulace pak trvá 3 s místo 30 s.

## Co simulace našla po zpřísnění (opraveno)

1. **Uložení novým souborem se párovalo bez ohledu na velikost písmen.** Po
   smazání `A` se jiný soubor `a` vydával za jeho novou verzi a nahrál se přes
   cizí dokument.
2. **`next_base` ponechávala cestu, kterou soubor už nedržel.** Když si ji
   vzal jiný dokument a soubor se pak smazal, nový soubor na té cestě se
   vydával za smazaný.
3. **Rozpoznání uložení novým souborem hledalo na společné cestě, ne tam, kde
   byl lokální soubor.** Proto vzniklo `local_path`.
4. **Bez času vzniku se nejednoznačný inode bral jako uložení novým
   souborem.** Teď se bere jako nový soubor.

## Rozhodnutí uživatele (2026-11-15)

- **Názvy v NFD:** lokální název zůstává, jak je na disku. Na server jde
  v NFC a páruje se po normalizaci. Synchronizace uživateli nic
  nepřejmenovává.
- **Brzda hromadných změn:** bude počítat kumulativně, například za
  24 hodin, aby zachytila i pomalé mazání rozložené do více běhů. Přesuny
  bude počítat zvlášť a v dotazu ukáže seznam souborů. Seznam už existuje
  (viz výše), zbytek je další krok.
