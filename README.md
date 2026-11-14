# ike loud

Neoficiální klient pro iCloud Drive na Linuxu. Soubory z iCloudu se objeví jako běžná složka v domovském adresáři a změny se propíšou na iPhone a Mac. Nastavení zvládne i člověk, který nikdy neotevřel terminál.

> **Stav:** projekt je ve fázi zadání. Zatím neexistuje kód ani instalovatelná verze. Zadání a plán realizace jsou v [ike-loud-zadani-a-plan.md](ike-loud-zadani-a-plan.md), schéma architektury v [architektura.mmd](architektura.mmd) a přehled podkladů v [PODKLADY.md](PODKLADY.md).

## Proč

Apple klienta iCloudu pro Linux nevydává. Kdo má na Linuxu placený iCloud, zatím otevírá soubory přes web icloud.com, nebo si nastaví rclone a systemd. ike loud má fungovat jako Dropbox nebo OneDrive: jednou se přihlásíte a pak už jen používáte složku **iCloud**.

## Hlavní zásady

Data na iCloudu jsou důležitější než rychlost synchronizace. Když si aplikace není jistá, nic nezmění a zeptá se.

- **iCloud je zdroj pravdy.** Při pochybnosti vyhraje verze na serveru a lokální verze se odloží stranou.
- **Nic se nepřepisuje naslepo.** Před každým zápisem aplikace ověří verzi souboru. Pokud se mezitím změnil, vytvoří konfliktní kopii.
- **Mazání jde vrátit.** Soubory smazané na Linuxu skončí na iCloudu v „Nedávno smazaných“.
- **Hromadné změny mají brzdu.** Když by synchronizace najednou smazala nebo přepsala hodně souborů, zastaví se a počká na potvrzení.
- **Každá akce je v deníku.** V deníku je vidět, co se kdy nahrálo, stáhlo a smazalo.

## Co umí první verze (plán)

- přihlášení Apple účtem s dvoufázovým ověřením a uložení relace do systémové klíčenky
- podpora účtů s pokročilou ochranou dat (ADP)
- obousměrná synchronizace vybraných složek a režim jen pro čtení
- řešení konfliktů, koš, brzda hromadných změn, deník
- okno stavu, oznámení, ikona v liště
- rozhraní česky a anglicky, další jazyky doplní překladatelé

Přijde později: soubory na vyžádání, ikony stavu ve správci souborů, více účtů, iCloud Fotky a sdílené složky.

Mimo rozsah: kalendář a kontakty (ty pokrývá CalDAV/CardDAV v GNOME a KDE), Poznámky, Klíčenka, Najdi a verze pro Windows a macOS.

## Technologie

Celé to bude v Rustu. Synchronizační démon poběží jako uživatelská služba a GUI v GTK4 a libadwaita s ním bude mluvit přes D-Bus. Chybějící oficiální dokumentaci API nahradí znalosti z backendu `iclouddrive` v [rclone](https://rclone.org/iclouddrive/) a z knihovny [pyicloud](https://pypi.org/project/pyicloud/).

Bezpečnostní vrstvu hlídají testy, model checking (TLA+ a Kani) a deterministická simulace. Distribuovat se bude hlavně přes Flatpak.

## Překlady

Texty aplikace jsou v souborech [`i18n/<jazyk>/ikeloud.ftl`](i18n/) ve formátu [Fluent](https://projectfluent.org/). Nový jazyk přidáte tak, že zkopírujete složku `i18n/en` pod kódem svého jazyka (například `i18n/de`) a přeložíte texty napravo od `=`. Programovat k tomu nemusíte.

## Upozornění

Nejde o produkt společnosti Apple a projekt s Applem nijak nesouvisí. Aplikace používá neoficiální webové API iCloudu a Apple ho může kdykoli změnit. iCloud a iCloud Drive jsou ochranné známky společnosti Apple Inc.
