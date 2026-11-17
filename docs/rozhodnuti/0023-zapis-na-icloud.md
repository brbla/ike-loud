# 0023: Zápis na iCloud: co server hlídá a co musí hlídat klient

Datum: 2026-11-17 · Milník 3 (příprava)

## Pokusy (testovací účet, složka `ikeloud-zapis-test`, pak do koše)

Volání podle webové aplikace icloud.com a rclone (`backend/iclouddrive`,
MIT). Výsledky:

| Operace | Volání | Podmínka verzí | Obsazené jméno |
| --- | --- | --- | --- |
| Nová složka | `drivews/createFolders` | – | položka `status: UNKNOWN`, nic nevznikne |
| Nový soubor | `docws/upload/web`, nahrání obsahu, `update/documents` s `command: add_file` | – | `allow_conflict: true`: tiše „a 2.txt“; **`allow_conflict: false`: 409 `UNIQUE_FIELD_FAILURE`, nic se nezmění** |
| Přepis obsahu | totéž, `document_id` existujícího dokumentu | `add_file`: pole `etag` **ignoruje**, přepíše vždy. **`modify_file` + `etag`: se zastaralou verzí 409 `OP_LOCK_FAILURE`, nic se nezmění**; bez `etag` přepíše vždy | – |
| Přesun | `drivews/moveItems` | **ano**: položka `status: ETAG_CONFLICT`, nic se nezmění | (ověří se) |
| Do koše | `drivews/moveItemsToTrash` | **ano**: `ETAG_CONFLICT` | – |
| Přejmenování | `drivews/renameItems` | **ne**: s vymyšlenou i se skutečnou starší verzí projde | server jméno tiše změní („a 3.txt“), nic nepřepíše |

Další zjištění:

- **iCloud rozlišuje velikost písmen**: „a 2.txt“, „A 2.TXT“ a „A 2.txt“
  mohou být vedle sebe. Zadání předpokládalo opak. Jádro jména dál
  porovnává bez ohledu na velikost písmen (`names::key`): takové varianty
  samo nevytvoří (Macy je obvykle neunesou) a existující nechá stranou
  jako kolizi. Opatrnější předpoklad je bezpečný.
- Etag souboru má tvar `X::Y`, složky `X`; po každé změně je nový. Etag
  nadřazených složek se mění se změnou uvnitř (0021).
- Přepis `modify_file` zachová ID dokumentu; web API nemá historii verzí,
  předchozí obsah na serveru zmizí.

## Rozhodnutí

- **Nahrání nového souboru** (i konfliktní kopie): vždy `allow_conflict:
  false`. Obsazené jméno je `RemoteError::Changed`; další běh rozhodne.
- **Přepis**: vždy `modify_file` s etagem, který zná jádro; typ v
  `ikeloud-api` jinou cestu nenabízí. `OP_LOCK_FAILURE` je `Changed`.
  Před přepisem démon stáhne současnou verzi ze serveru do lokální
  historie (zadání: před každým přepsáním i na serveru se předchozí
  verze uloží stranou na 30 dní).
- **Do koše a přesun**: s etagem; `ETAG_CONFLICT` je `Changed`.
- **Přejmenování**: server verzi nehlídá. Klient těsně před ním ověří
  etag čerstvým výpisem nadřazené složky a po něm porovná jméno v
  odpovědi s požadovaným; jiné jméno je `Changed` (server se vyhnul
  kolizi). Okno mezi ověřením a přejmenováním obsah neohrozí:
  přejmenování nic nepřepisuje ani nemaže.
- **Zápis je volitelný** (nastavení „Nahrávat na iCloud“, výchozí
  vypnuto), dokud nepadne brána milníku 3: 14 dní běhu, beta. Bez něj a
  do dokončení prvního stažení běží démon jen pro čtení.

## Ověřeno naostro (17. 11. 2026, testovací účet)

Démon s volbou „Nahrávat na iCloud“ v podsložce `ikeloud-obousmerne`:
nový soubor nahrán (i se složkou); úprava nahrána jako nová verze téhož
dokumentu a předchozí verze ze serveru uložena v lokální historii;
přejmenování přeneseno jako přejmenování; soubor přidaný na iCloudu
stažen; smazání na Linuxu přesunuto do koše iCloudu; úprava na obou
stranách zároveň: na iCloudu zůstala jeho verze a linuxová je vedle jako
„… (konflikt Linux 2026-11-17).txt“, nahraná i na iCloud.

Známá mezera: prázdné složky se po smazání nepřenášejí (ani jedním
směrem); zůstanou prázdné. Obsah to neohrožuje.

Režim démona se rozhoduje po každém běhu: do dokončení prvního stažení
(značka `first-download-done` ve složce stavu) a bez volby jen čtení.
