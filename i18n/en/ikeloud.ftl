# English – ike loud texts.
#
# Fluent format: https://projectfluent.org/
# New language: copy the folder i18n/en to i18n/<language code> (e.g. i18n/de)
# and translate the text to the right of “=”. Do not change the keys on the left.

## Application

aplikace-nazev = ike loud
aplikace-popis = Unofficial client for iCloud Drive

## Sizes

velikost-bajty = { $hodnota } B
velikost-kb = { $hodnota } kB
velikost-mb = { $hodnota } MB
velikost-gb = { $hodnota } GB
velikost-tb = { $hodnota } TB
# Character between the integer and the decimal part of a number.
oddelovac-desetin = .

## Sync is stopped

zamitnuti-lokalni-slozka-nedostupna =
    The iCloud folder is not available right now, so syncing has stopped. Your files on iCloud are safe. If the folder is on an external drive, connect it.
zamitnuti-podezrely-vypis-serveru =
    iCloud returned an empty file list although there should be files. Syncing has stopped and nothing is deleted. It will try again shortly.
zamitnuti-konflikt =
    { $pocet ->
        [one] One file was also changed on another device in the meantime.
       *[other] { $pocet } files were also changed on another device in the meantime.
    } Nothing was overwritten; syncing will sort it out shortly and keep both versions.
zamitnuti-hromadna-zmena =
    Syncing would delete or overwrite { $pocet ->
        [one] one file
       *[other] { $pocet } files
    } out of { $celkem } at once. Please confirm this is OK before it continues.
zamitnuti-malo-mista-icloud =
    There is not enough space on iCloud: { $potreba } needed, { $volno } free. Syncing has stopped and nothing is deleted. Free up space on iCloud or buy more.
zamitnuti-malo-mista-disk =
    There is not enough space on this computer: { $potreba } needed, { $volno } free. Syncing has stopped and nothing is deleted. Free up disk space.

## A file is waiting

odlozeno-jen-cteni = Only downloading for now. { $duvod }
odlozeno-soubor-se-zapisuje = The file is still being saved. It will be uploaded once it is done.
odlozeno-ignorovany-soubor = Temporary program file; it is not uploaded to iCloud.
odlozeno-kolize-velikosti =
    iCloud already has “{ $s }”. iCloud does not distinguish upper and lower case, so please rename the file.
odlozeno-nazev-prazdna-cast = The file or folder name is empty. Please rename the file.
odlozeno-nazev-tecka = iCloud does not accept the name “.” or “..”. Please rename the file.
odlozeno-nazev-znak = The name contains a character iCloud does not accept. Please rename the file.
odlozeno-nazev-prilis-dlouhy = The name is too long for iCloud. Please rename the file.

## Why it is only downloading

jen-cteni-nastaveni = Writing to iCloud is turned off in settings.
jen-cteni-prvni-stazeni = Writing to iCloud turns on once the first download has finished and been checked.
jen-cteni-neznama-odpoved-api = iCloud answered unexpectedly. To be safe, nothing is written to it until a fixed version is out.
jen-cteni-vzdalena-pojistka = Apple changed iCloud. To be safe, nothing is written to it until a fixed version is out.
