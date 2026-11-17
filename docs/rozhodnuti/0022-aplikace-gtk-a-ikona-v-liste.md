# 0022: Aplikace v GTK 4, ikona v liště a démon jen pro čtení

Datum: 2026-11-17 · Milník 1

## Rozhodnutí

- **GTK 4 + libadwaita** (`gtk4` 0.11, `libadwaita` 0.9; rozhodnutí
  vlastníka 16. 11.). Požadavek na verze: GTK 4.12 a libadwaita 1.5,
  tedy co má Ubuntu 24.04 v CI. Novější widgety (např. `AdwSpinner`) se
  nepoužívají.
- **Ikona v liště vlastní nad GIO D-Bus** (`tray.rs`):
  StatusNotifierItem a menu `com.canonical.dbusmenu`. Knihovna `ksni`
  by přinesla `zbus` a asynchronní běhové prostředí; GIO je v GTK
  stejně. Bez hostitele lišty (GNOME bez rozšíření) ikona prostě není a
  stav ukazuje okno.
- **Saxofon** nakreslený pro projekt (`icons.rs`), stav jako barevný
  odznak: zelená fajfka (aktuální), modré šipky (synchronizuje), oranžový
  vykřičník (čeká na uživatele, odhlášeno), červený (chyba), šedá
  (bez připojení, pozastaveno). Ikony se zapisují do složky stavu a
  hostiteli lišty jdou jménem (`IconThemePath`) i v pixelech
  (`IconPixmap`).
- **Průvodce prvním spuštěním:** přihlášení (heslo, pak kód z SMS s
  možností poslat nový), složka (výchozí `~/iCloud`, jen nová nebo
  prázdná, ne vlastní složky aplikace: `ikeloud_config::check_folder`),
  pak start démona. Kódy e-mailem Apple neposílá (0021).
- **Nastavení** (ozubené kolečko v hlavičce): synchronizovaná složka se
  změnou a účet s odhlášením. Nová složka musí být nová nebo prázdná; ve
  staré soubory zůstanou. Stav synchronizace patří ke staré složce a
  zahodí se: jinak by soubory chybějící v nové složce vypadaly jako
  smazané a nestáhly by se. Jen pro čtení tím nic nezmizí; až se bude
  zapisovat, bude změna složky potřebovat vlastní postup.
- **Přihlašuje aplikace, synchronizuje démon.** Aplikace předá relaci
  souborem `$XDG_STATE_HOME/ikeloud/session.json` (0600); heslo se
  neukládá. Démon píše svůj stav do `…/ikeloud/status` (`key=value`,
  atomicky) a aplikace ho jednou za sekundu čte.
- **Démon spouští aplikace** jako svůj podproces; pozastavení ho ukončí.
  Ukončení je bezpečné kdykoli (transakční stav, stahování přes dočasné
  soubory). Zavření okna při ikoně v liště jen schová okno; „Ukončit“
  ukončí aplikaci, démon běží dál.
- **Démon jen pro čtení:** `SyncSetup::mode = Mode::ReadOnly(FirstDownload)`,
  bezpečnostní vrstva odloží každý zápis na server a `ICloudRemote::write`
  zápis odmítne i tak.

## Dočasné, než přijde balení (milník 1)

- Stav a ovládání souborem a podprocesem místo D-Bus rozhraní démona.
  D-Bus přijde se službou systemd / Background portálem; formát
  stavového souboru zůstane jako záloha pro diagnostiku.
- Relace v souboru místo klíčenky (Secret Service).
- Když se uživatel odhlásí a přihlásí jiným účtem, složka zůstává
  nastavená; průvodce by měl chtít novou. Zatím ne.

## CI

- Úloha `check` instaluje `libgtk-4-dev` a `libadwaita-1-dev` před
  výpočtem klíče cache; klíč obsahuje verze a příznaky z `pkg-config`
  (`gtk4`, `libadwaita-1`) a proměnné `SYSTEM_DEPS_*` (SCHEMA 5).
- `deny.toml` povoluje druhou verzi `system-deps` (7.0.8): žádá ji
  `libadwaita-sys` 0.9.2, jen při sestavení.
- Nových 53 knihoven ve výjimkách `cargo vet` (0007).
