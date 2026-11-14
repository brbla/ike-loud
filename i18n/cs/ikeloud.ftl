# Čeština – texty ike loud.
#
# Formát Fluent: https://projectfluent.org/
# Nový jazyk: zkopírujte složku i18n/en do i18n/<kód jazyka> (např. i18n/de)
# a přeložte texty napravo od „=“. Klíče nalevo neměňte.

## Aplikace

aplikace-nazev = ike loud
aplikace-popis = Neoficiální klient pro iCloud Drive

## Velikosti

velikost-bajty = { $hodnota } B
velikost-kb = { $hodnota } kB
velikost-mb = { $hodnota } MB
velikost-gb = { $hodnota } GB
velikost-tb = { $hodnota } TB
# Znak mezi celou a desetinnou částí čísla.
oddelovac-desetin = ,

## Synchronizace stojí

zamitnuti-lokalni-slozka-nedostupna =
    Složka iCloud teď není dostupná, proto synchronizace stojí. Soubory na iCloudu jsou v bezpečí. Pokud je složka na externím disku, připojte ho.
zamitnuti-podezrely-vypis-serveru =
    iCloud vrátil prázdný seznam souborů, i když tam soubory mají být. Synchronizace proto stojí a nic se nemaže. Zkusí to znovu za chvíli.
zamitnuti-konflikt =
    { $pocet ->
        [one] Jeden soubor se mezitím změnil i na jiném zařízení.
        [few] { $pocet } soubory se mezitím změnily i na jiném zařízení.
       *[other] { $pocet } souborů se mezitím změnilo i na jiném zařízení.
    } Nic se nepřepsalo; synchronizace to za chvíli vyřeší a obě verze zachová.
zamitnuti-hromadna-zmena =
    Synchronizace by najednou smazala nebo přepsala { $pocet ->
        [one] jeden soubor
        [few] { $pocet } soubory
       *[other] { $pocet } souborů
    } z { $celkem }. Než bude pokračovat, potvrďte prosím, že je to v pořádku.
zamitnuti-malo-mista-icloud =
    Na iCloudu není dost místa: potřeba { $potreba }, volno { $volno }. Synchronizace stojí a nic se nemaže. Uvolněte místo na iCloudu nebo si ho dokupte.
zamitnuti-malo-mista-disk =
    Na tomto počítači není dost místa: potřeba { $potreba }, volno { $volno }. Synchronizace stojí a nic se nemaže. Uvolněte místo na disku.

## Soubor teď čeká

odlozeno-jen-cteni = Zatím se jen stahuje. { $duvod }
odlozeno-soubor-se-zapisuje = Soubor se ještě ukládá. Nahraje se, až bude hotový.
odlozeno-ignorovany-soubor = Dočasný soubor programu; na iCloud se nenahrává.
odlozeno-kolize-velikosti =
    Na iCloudu už je „{ $s }“. iCloud nerozlišuje velká a malá písmena, proto soubor přejmenujte.
odlozeno-nazev-prazdna-cast = Název souboru nebo složky je prázdný. Soubor přejmenujte.
odlozeno-nazev-tecka = Název „.“ ani „..“ iCloud nepřijme. Soubor přejmenujte.
odlozeno-nazev-znak = Název obsahuje znak, který iCloud nepřijme. Soubor přejmenujte.
odlozeno-nazev-prilis-dlouhy = Název je pro iCloud příliš dlouhý. Soubor přejmenujte.

## Proč se jen stahuje

jen-cteni-nastaveni = Zápis na iCloud je vypnutý v nastavení.
jen-cteni-prvni-stazeni = Zápis na iCloud se zapne, až se dokončí a zkontroluje první stažení.
jen-cteni-neznama-odpoved-api = iCloud odpověděl neočekávaně. Pro jistotu se na něj nezapisuje, dokud nevyjde opravená verze.
jen-cteni-vzdalena-pojistka = Apple změnil iCloud. Pro jistotu se na něj nezapisuje, dokud nevyjde opravená verze.
