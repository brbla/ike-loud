# 0020: TLA+ modely jmen a brzdy hromadných změn

Datum: 2026-11-16 · Milník 1 a 2 (bez Apple účtu)

## Rozhodnutí

Přibyly dva modely. Každý má i konfiguraci s úmyslnou chybou, která musí
selhat. Kdyby model chybu přestal nacházet, už by nic nehlídal (stejně jako
`SyncIdentityInodeOnly`, 0014). CI pouští obě strany.

| Model | Konfigurace | Výsledek |
|---|---|---|
| `SyncNames.tla` | `SyncNames.cfg` | projde (37 470 stavů, 15 s) |
| | `SyncNamesExact.cfg` | musí selhat: `NoRenameChurn` |
| `SyncBrake.tla` | `SyncBrake.cfg` | projde (1,5 milionu stavů, 1 min) |
| | `SyncBrakeConfirmFirst.cfg` | musí selhat: `NoNeedlessQuestion` |
| | `SyncBrakeDropConfirmation.cfg` | musí selhat: `NoNeedlessQuestion` |
| | `SyncBrakeRecordAny.cfg` | musí selhat: `NoSilentMassDeletion` |

## Model jmen (`SyncNames`)

Rozšiřuje `SyncMoves` o čtyři zápisy jména: „a“ v NFC, „a“ rozložené (NFD,
jak ho zapíše Mac), „A“ a „b“. Linux rozlišuje všechny čtyři. iCloud nerozlišuje
tvar ani velikost písmen, takže „a“, rozložené „a“ a „A“ jsou tam jedno jméno.
Model sleduje pravidla z `names.rs` a `plan.rs`:

- místa se porovnávají po NFC (`same_place`): přejmenování jen ve velikosti
  písmen je přejmenování, změna jen tvaru není;
- jména na serveru se hlídají podle klíče a vytvářejí v NFC;
- lokální zápis jména zůstává (rozhodnutí z 2026-11-15);
- lokální kolizi („a“ a „A“ v jedné složce) synchronizace nechá být na obou
  stranách, dokud ji nevyřeší uživatel;
- soubor, který se má uložit lokálně, uvolní místo jinému souboru se stejným
  klíčem podle klíče, takže synchronizace sama kolizi nikdy nevytvoří.

Vlastnosti:
- žádná ztráta obsahu;
- jedinečná jména na obou stranách;
- čisté přesuny nic neukládají stranou;
- žádné přejmenovávání sem a tam mezi NFD a NFC (`NoRenameChurn`);
- po ustálení se strany shodují, pokud uživatel nenechal lokální kolizi.

Model při psaní ukázal jednu věc o sobě, ne o kódu. Když iPhone změní jen
tvar jména, společný stav musí převzít nový zápis i bez akce. Kód to dělá,
protože `next_base` běží po každém běhu. Model to proto musí dělat také.

Chyby v kódu model nenašel. Varianta, která porovnává přesný zápis místo
místa, okamžitě vyrobí nekonečné přejmenovávání.

## Model brzdy (`SyncBrake`): tři nálezy, opraveno

Model brzdu sleduje v čase. Uživatel maže a obnovuje soubory, běhy počítají
smazání v okně brzdy z deníku a mohou skončit v půlce (výpadek). Uživatel
potvrzuje přesně ten seznam, který viděl.

Vlastnosti:
- **`NoSilentMassDeletion`:** v žádném okně od posledního použitého
  potvrzení neprojde víc nepotvrzených smazání, než je limit, ani rozložením
  do více běhů;
- **`NoNeedlessQuestion`:** brzda se nikdy neptá jen na soubory, které
  uživatel už potvrdil, ani kvůli tomu, že započítala potvrzená smazání.

Plán, který po potvrzení přibral další soubor, se ptá znovu. To je záměr:
potvrzení platí jen pro vypsané soubory.

### 1. Potvrzení se zapisovalo před potvrzené akce

Deník se čte od nejnovějšího záznamu a zastaví se u potvrzení. Potvrzené
akce byly v deníku až za ním, takže se započítaly do dalšího okna. Po
potvrzeném smazání 55 souborů se brzda ptala i na smazání jediného dalšího
souboru v tentýž den. Rozhodnutí 0016 přitom říká, že se potvrzení zapíše
„po provedení“.

**Oprava:** `MassChangeConfirmed` se zapisuje až po provedení akcí. Pád
mezi akcemi a zápisem způsobí jen to, že se potvrzené změny započítají
ještě jednou, tedy opatrnější chování. Test celého běhu
(`a_mass_deletion_waits_for_the_user`) na starém kódu selže.

### 2. Potvrzení se zapsalo i za běh, který nepokrývalo (bezpečnost)

Démon předával potvrzení dalšímu běhu a ten ho zapsal do deníku, kdykoli
nějaké dostal. Model našel tento postup:

1. Uživatel smaže tři soubory a brzda se zeptá.
2. Uživatel dva soubory obnoví, smaže jiný a otázku potvrdí.
3. Běh, který potvrzení nepokrývá, ale je pod limitem, ho přesto zapíše
   a okno začne znovu.
4. Další smazání projdou bez dotazu.

Pomalé hromadné mazání tak mohlo brzdu obejít.

**Oprava:** bezpečnostní vrstva vrací `ApprovedPlan::confirmed`. Je pravda
jen tehdy, když potvrzení pokrývá všechny změny plánu. Démon zapíše
potvrzení jen v tom případě (tabulkový test
`a_plan_is_confirmed_only_if_the_confirmation_covers_it`).

### 3. Výpadek spotřeboval potvrzení

Démon zahodil potvrzení po každém běhu, který prošel bezpečnostní vrstvou,
i když běh skončil v půlce na výpadku. Na zbytek potvrzených smazání se pak
brzda zeptala znovu.

**Oprava:** potvrzení platí, dokud ho nespotřebuje běh, který proběhl celý
(bez `stopped`).

## Meze modelů

- Brzda je modelovaná pro smazání. Přepsání se počítají stejným počítadlem.
  Přesuny mají vlastní počítadlo se stejnou logikou (`check_mass_change`
  volá totéž pro obě), takže zvláštní model nemají.
- Okraj brzdy (absolutní limit a podíl ze synchronizovaných souborů)
  dokazuje Kani pro všechny vstupy (0013). Model používá malá čísla.
- Model jmen předpokládá, že lokální souborový systém rozlišuje velikost
  písmen (ext4, btrfs). Složka na exFAT nebo NTFS by potřebovala vlastní
  rozbor.
- Že iCloud jména neliší ve tvaru ani velikosti písmen, je předpoklad,
  který ověří milník 0.
