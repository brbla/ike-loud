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
    Synchronizace by najednou smazala nebo přepsala { $count ->
        [one] jeden soubor
        [few] { $count } soubory
       *[other] { $count } souborů
    } z { $total }. Než bude pokračovat, potvrďte prosím, že je to v pořádku.
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
deferred-name-empty = Název souboru nebo složky je prázdný. Soubor přejmenujte.
deferred-name-dot = Název „.“ ani „..“ iCloud nepřijme. Soubor přejmenujte.
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
