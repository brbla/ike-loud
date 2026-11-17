# Czech – ike loud texts (Čeština).
#
# Fluent format: https://projectfluent.org/
# New language: copy the folder i18n/en to i18n/<language code> (e.g. i18n/de)
# and translate the text to the right of “=”. Do not change the keys on the left.

## Application

app-name = ike loud
app-description = Neoficiální klient pro iCloud Drive

## Sizes

size-bytes = { $value } B
size-kb = { $value } kB
size-mb = { $value } MB
size-gb = { $value } GB
size-tb = { $value } TB
# Character between the integer and the decimal part of a number.
decimal-separator = ,

## Sync is stopped

rejected-local-folder-unavailable =
    Složka iCloud teď není dostupná, proto synchronizace stojí. Soubory na iCloudu jsou v bezpečí. Pokud je složka na externím disku, připojte ho.
rejected-suspicious-remote-listing =
    iCloud vrátil prázdný seznam souborů, i když tam soubory mají být. Synchronizace proto stojí a nic se nemaže. Zkusí to znovu za chvíli.
rejected-conflict =
    { $count ->
        [one] Jeden soubor se mezitím změnil i na jiném zařízení.
        [few] { $count } soubory se mezitím změnily i na jiném zařízení.
       *[other] { $count } souborů se mezitím změnilo i na jiném zařízení.
    } Nic se nepřepsalo; synchronizace to za chvíli vyřeší a obě verze zachová.
rejected-mass-change =
    { $changes ->
        [0] {""}
       *[other] Synchronizace by během dne smazala nebo přepsala { $changes ->
            [one] jeden soubor
            [few] { $changes } soubory
           *[other] { $changes } souborů
        } z { $total }.{" "}
    }{ $moves ->
        [0] {""}
       *[other] Synchronizace by během dne přesunula { $moves ->
            [one] jeden soubor
            [few] { $moves } soubory
           *[other] { $moves } souborů
        } z { $total }.{" "}
    }Než bude pokračovat, zkontrolujte prosím seznam souborů a potvrďte, že je to v pořádku.
rejected-icloud-space =
    Na iCloudu není dost místa: potřeba { $needed }, volno { $free }. Synchronizace stojí a nic se nemaže. Uvolněte místo na iCloudu nebo si ho dokupte.
rejected-disk-space =
    Na tomto počítači není dost místa: potřeba { $needed }, volno { $free }. Synchronizace stojí a nic se nemaže. Uvolněte místo na disku.

## A file is waiting

deferred-read-only = Zatím se jen stahuje. { $reason }
deferred-still-being-written = Soubor se ještě ukládá. Nahraje se, až bude hotový.
deferred-ignored = Dočasný soubor programu; na iCloud se nenahrává.
deferred-case-collision =
    Na iCloudu už je „{ $name }“. iCloud nerozlišuje velká a malá písmena, proto soubor přejmenujte.
deferred-name-char = Název obsahuje znak, který iCloud nepřijme. Soubor přejmenujte.
deferred-name-too-long = Název je pro iCloud příliš dlouhý. Soubor přejmenujte.

## Why it is only downloading

read-only-settings = Zápis na iCloud je vypnutý v nastavení.
read-only-first-download = Zápis na iCloud se zapne, až se dokončí a zkontroluje první stažení.
read-only-unknown-api = iCloud odpověděl neočekávaně. Pro jistotu se na něj nezapisuje, dokud nevyjde opravená verze.
read-only-kill-switch = Apple změnil iCloud. Pro jistotu se na něj nezapisuje, dokud nevyjde opravená verze.

## File names

# Name of a conflict copy. $stem is the name without extension, $extension
# includes the dot (may be empty), $date looks like 2026-11-15, $number
# is 1 for the first copy that day. Do not use the slash “/”.
conflict-copy-name =
    { $stem } ({ $number ->
        [1] konflikt Linux { $date }
       *[other] konflikt Linux { $date } { $number }
    }){ $extension }

## App window and setup (ikeloud-gui)

gui-signin-title = Přihlášení k iCloudu
gui-signin-description = Použijte svůj Apple účet. ike loud si heslo neukládá: pamatuje si jen přihlášení.
gui-account = Apple účet (e-mail)
gui-password = Heslo
gui-signin = Přihlásit
gui-code-title = Ověřovací kód
gui-code-description = Apple poslal šestimístný kód na telefonní číslo končící { $phone }.
gui-code = Kód
gui-verify = Ověřit
gui-resend = Poslat nový kód
gui-code-sent = Nový kód je na cestě.
gui-folder-title = Kam ukládat soubory
gui-folder-description = Do této složky se stáhne iCloud Drive. Musí být nová nebo prázdná: ike loud nikdy nesmíchá iCloud se soubory, které už tam jsou.
gui-folder = Složka
gui-folder-choose = Vybrat…
gui-folder-done = Začít stahovat
gui-folder-not-a-folder = Tohle je soubor, ne složka.
gui-folder-overlaps = Tato složka patří samotnému ike loud. Vyberte jinou.
gui-folder-error = Tuto složku nejde použít.
gui-error-credentials = Špatný Apple účet nebo heslo.
gui-error-code = Špatný nebo prošlý kód.
gui-error-too-many = Příliš mnoho pokusů. Chvíli počkejte a zkuste to znovu.
gui-error-no-phone = Apple účet nemá důvěryhodné telefonní číslo pro kód.
gui-error-offline = Nejde se spojit s iCloudem. Zkontrolujte připojení k internetu.
gui-error-drive-not-set-up = iCloud Drive ještě není založený. Jednou ho otevřete na icloud.com a pak se přihlaste znovu.
gui-error-drive-unavailable = Tento Apple účet nemá iCloud Drive.
gui-error-other = Přihlášení se nepovedlo: { $detail }
gui-open-folder = Otevřít složku
gui-pause = Pozastavit
gui-resume = Pokračovat
gui-sign-out = Odhlásit
gui-show = Zobrazit ike loud
gui-quit = Ukončit
gui-state-starting = Spouští se
gui-state-idle = Aktuální
gui-state-syncing = Synchronizuje
gui-state-needs-confirmation = Čeká na vaše potvrzení
gui-state-signed-out = Odhlášeno
gui-state-offline = Bez připojení
gui-state-error = Vyžaduje pozornost
gui-state-paused = Pozastaveno
gui-state-idle-detail = Souborů na iCloudu: { $files }. Staženo od spuštění: { $downloaded }.
gui-state-syncing-detail = Staženo od spuštění: { $downloaded }.
gui-state-signed-out-detail = Přihlášení vypršelo. Pro pokračování se přihlaste znovu.
gui-state-offline-detail = Zkusí to znovu samo.
gui-state-paused-detail = Dokud nebudete pokračovat, nic se nestahuje.
gui-settings = Nastavení
gui-folder-change = Změnit…
gui-folder-change-description = Kam se stahuje iCloud Drive. Nová složka musí být nová nebo prázdná; ve staré soubory zůstanou.
gui-folder-changed = Složka je změněná. iCloud Drive se do ní stahuje.
gui-account-section = Účet
gui-cancel = Zrušit
gui-folder-will-be-emptied =
    { $count ->
        [one] Ve složce je 1 položka. Nejdřív se přesune do koše, aby ve složce byl jen iCloud.
        [few] Ve složce jsou { $count } položky. Nejdřív se přesunou do koše, aby ve složce byl jen iCloud.
       *[other] Ve složce je { $count } položek. Nejdřív se přesunou do koše, aby ve složce byl jen iCloud.
    }
gui-folder-trash-and-start = Přesunout do koše a začít
gui-folder-protected = Tuto složku nejde pro iCloud vyprázdnit: je to domovská složka nebo jedna z vašich běžných složek (Dokumenty, Plocha…), nebo nějakou obsahuje. Vyberte novou složku.
gui-folder-nested = Nová složka nemůže být uvnitř současné ani ji obsahovat.
gui-folder-trash-failed = Přesun do koše se nepovedl: { $detail }
gui-change-heading = Změnit složku?
gui-change-body = iCloud Drive se znovu celý stáhne do složky { $folder }.
gui-change-old = Současná složka { $old } může zůstat, jak je, nebo jít do koše (odtud ji půjde obnovit).
gui-change-keep-old = Změnit, starou ponechat
gui-change-trash-old = Změnit, starou do koše
gui-read-only = Zatím jen čtení: na iCloud se nikdy nic nezapíše.
