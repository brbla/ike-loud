# 0016: Démon a jeden běh synchronizace

Datum: 2026-11-15 · Milník 1 a 2 (bez Apple účtu)

## Rozhodnutí

`ikeloud_daemon::sync_once` je jeden běh synchronizace:

1. **sken** lokální složky (0015). Chybějící složka zastaví běh jako
   `LocalFolderUnavailable`, nečitelná jako `ScanFailed`. Nic se nezmění;
2. **výpis serveru** přes rozhraní `Remote`. Když ho nejde získat, běh skončí
   a nic se nezmění;
3. **plán** ze tří stavů, se společným stavem z databáze;
4. **posouzení** bezpečnostní vrstvou. Kontext doplní démon: počty souborů,
   etagy z výpisu a změny v okně brzdy, spočítané z deníku za posledních
   24 hodin, ale jen od posledního potvrzení hromadné změny;
5. **provedení** schválených akcí. Lokální akce dělá `LocalDisk`, serverové
   `Remote`. Zápisy na server berou `ApprovedAction`, takže nic neschváleného
   se na server nedostane. Nahrávání počítá otisk přesně z odeslaných bajtů;
6. **nový společný stav** podle `next_base` z nového skenu obou stran. Ukládá
   se v jedné transakci s deníkem.

Server je za rozhraním `Remote`. Teď ho v testech naplňuje falešný iCloud,
později skutečný klient.

## Výpadek serveru uprostřed běhu (nalezeno testem)

Když server během běhu vypadne a po běhu nejde znovu vypsat, první verze
neuložila nový společný stav. Další běh pak bral soubory nahrané před
výpadkem jako „nové na obou stranách“ a dělal z nich konfliktní kopie: 7
souborů místo 5. Data se neztratila, ale vznikly duplikáty.

**Oprava:** místo chybějícího výpisu se použije poslední výpis s tím, co běh
prokazatelně udělal. Nahrání jsou doložená přenosy (dokument a etag). Kdyby
se předpověď lišila od skutečnosti, další skutečný výpis to ukáže jako
změnu na iCloudu a ta se zpracuje běžným způsobem, s předchozí verzí
v historii. Když nejde znovu přečíst lokální složka, uloží se jen deník:
starý společný stav je bezpečný, stav z neúplného skenu ne.

## Hromadná změna

Potvrzení uživatele se předá dalšímu běhu. Po provedení se do deníku zapíše
`MassChangeConfirmed`, od kterého se okno brzdy počítá znovu.

## Zákaz core dumpů

Rozhodnutí 0013 chtělo, aby démon zakázal ukládání své paměti při pádu.
Z Rustu to jde jen přes `prctl` nebo `setrlimit`, tedy přes `libc`
a `unsafe` kód, který workspace zakazuje (`unsafe_code = "forbid"`). Zákaz
proto nastavuje služba: `LimitCORE=0` v `packaging/systemd/ikeloud.service`.
Efekt je stejný a kód zůstává bez `unsafe`. Pro Flatpak se to vyřeší
s balíčkem.

## Testy

`crates/ikeloud-daemon/tests/sync.rs`: celé běhy se skutečným skenerem,
plánovačem, bezpečnostní vrstvou, vykonavatelem, lokálními akcemi
a databází proti falešnému iCloudu, který:

- vynucuje etagy a volná jména;
- nerozlišuje velikost písmen;
- nahrává jen do existujících složek;
- maže do „Nedávno smazaných“;
- umí vypadnout po N zápisech.

Testy pokrývají:

- první synchronizaci;
- úpravy oběma směry s historií;
- smazání do košů na druhé straně;
- konflikt;
- přejmenování jako přesun;
- výpadek uprostřed běhu;
- hromadné smazání s potvrzením;
- chybějící složku;
- nedostupný server.

Ověřuje se i to, že po ustálení je další běh prázdný.
