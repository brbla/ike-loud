# 0022: Aplikace v čistém GTK 4, ikona v liště a démon jen pro čtení

Datum: 2026-11-17 · Milník 1

## Rozhodnutí

- **Čisté GTK 4 bez libadwaita** (`gtk4` 0.11, požadavek GTK 4.12 kvůli
  Ubuntu 24.04 v CI). Původně libadwaita (16. 11.); 17. 11. vlastník
  rozhodl, že aplikace má vypadat systémově. libadwaita motiv systému
  záměrně ignoruje a na KDE vypadá jako GNOME; čisté GTK bere motiv
  systému (Breeze na KDE, Adwaita na GNOME). Výběr složky je dialog
  systému přes portál (KDE na Plasmě, GNOME na GNOME), potvrzení jsou
  `gtk::AlertDialog` v motivu systému. Titulek okna kreslí na Waylandu
  GTK (ve stylu motivu), ne KWin. Plně nativní KDE by znamenalo druhé
  rozhraní v Qt: ne.
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
  změnou a účet s odhlášením. Změna složky chce potvrzení: iCloud se do
  nové složky stáhne znovu celý a stará složka zůstane, nebo jde do koše
  (podle volby). Stav synchronizace patří ke staré složce a zahodí se:
  jinak by soubory chybějící v nové složce vypadaly jako smazané. Jen pro
  čtení se tím iCloudu nic nestane; až se bude zapisovat, bude změna
  složky potřebovat vlastní postup.
- **Neprázdná složka** (v průvodci i při změně): po potvrzení s počtem
  položek se její obsah přesune do koše (freedesktop.org přes GIO, jde
  obnovit), aby ve složce byl jen iCloud. Nikdy se nemaže natrvalo a
  nikdy se nevyprazdňuje domovská složka, standardní složky uživatele
  (Dokumenty, Plocha, …) ani složka, která některou obsahuje
  (`ikeloud_config::protected_folder`). Nová složka nesmí ležet uvnitř
  současné ani ji obsahovat.
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

- Úloha `check` instaluje `libgtk-4-dev` před výpočtem klíče cache; klíč
  obsahuje verzi a příznaky `gtk4` z `pkg-config` a proměnné
  `SYSTEM_DEPS_*` (SCHEMA 6).
- Nových 50 knihoven ve výjimkách `cargo vet` (0007). S libadwaita
  odešly `libadwaita`, `libadwaita-sys` a druhá verze `system-deps`
  (a její výjimka v `deny.toml`).
