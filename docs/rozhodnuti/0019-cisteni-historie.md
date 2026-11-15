# 0019: Čištění lokální historie po 30 dnech; koše se nečistí

Datum: 2026-11-15 · Milník 1 a 2 (bez Apple účtu)

## Rozhodnutí

Zadání chce, aby se před každým přepsáním uložila předchozí verze stranou
na 30 dní. Démon proto jednou denně a při prvním běhu po startu smaže
z lokální historie verze uložené před více než 30 dny
(`LocalDisk::purge_history`, `Timing::keep_history`). Úklid probíhá ve
stejné denní údržbě jako přečtení všech souborů (0018).

- **Stáří podle jména, ne podle času souboru.** Každá uložená verze je
  složka `<sekundy>-<proces>-<pořadí>` pojmenovaná podle okamžiku uložení.
  Pevný odkaz si nese čas změny původního souboru, takže čas souboru o stáří
  verze nic neříká.
- **Jen vlastní složky.** Cokoli jiného v historii (jiné jméno, soubor,
  symbolický odkaz) zůstane. Symbolický odkaz se nesleduje.
- **Verze „z budoucnosti“ zůstává.** Když se hodiny vrátí, verze se
  nesmaže dřív, než je jí opravdu 30 dní.
- Verze, kterou nejde smazat, se zkusí znovu příští den. Chybějící složka
  historie znamená, že není co mazat.

Zbývá jedno riziko: hodiny nastavené daleko dopředu (vybitá baterie,
ručně) smažou historii předčasně. Na počítači bez spolehlivého času
nezávislé měřítko stáří není. Vzhledem k NTP to přijímáme.

## Koše se nečistí (oponuji zadání úkolu)

- **Koš iCloudu (Nedávno smazané)** čistí po 30 dnech Apple sám.
- **Systémový koš** (freedesktop Trash) patří uživateli, ne ike loud.
  Zadání říká jen to, že smazání z iCloudu jde do koše systému. Lhůtu
  30 dní dává jen lokální historii. Mazání z koše by navíc šlo proti
  nastavení desktopu: GNOME má vlastní volbu automatického vysypání a uživatel
  v koši čeká, že tam věci zůstanou, dokud ho sám nevysype. Kdyby ike loud
  koš čistil, mohl by smazat soubor, který uživatel teprve hledá. Rozlišit
  naše položky od cizích by šlo jen podle cesty v `.trashinfo`, ani to ale
  neznamená, že je uživatel nechce.

Když to bude potřeba, jde to přidat jako volbu v nastavení. Výchozí
chování bude koš nechat být.

## Mezera v zadání (k rozhodnutí v milníku 0)

Zadání chce předchozí verzi uložit stranou i před přepsáním na serveru.
Dnes se při nahrání nové verze přes starou na iCloudu stará verze nikam
neukládá. Lokálně je jen v případě, že ji uživatel nepřepsal (u úprav
v editoru ji přepsal). Řešení je stáhnout starou verzi do historie před
nahráním, což zdvojnásobí přenos, nebo spoléhat na verze, které drží iCloud,
pokud nějaké drží. To ukáže milník 0.

## Testy

- `ikeloud-local/tests/disk.rs`: verze staré 35, 20 a „−20“ dní; smaže se
  jen první. Cizí složka, soubor a symbolický odkaz ven zůstanou. Druhý úklid
  nic nesmaže. Hodiny před 30. dnem od epochy nesmažou nic. Chybějící
  historie projde bez chyby.
- Jednotkový test rozpoznání jmen složek historie.
- `ikeloud-daemon/tests/daemon.rs`: skutečný démon uklidí starou verzi při
  prvním běhu a pak až další den.
