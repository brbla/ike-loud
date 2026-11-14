# 0002: TLA+ model synchronizace

Datum: 2026-11-14 · Milník 0

## Výsledek

Rozhodovací tabulka ze zadání je bezpečná, **pokud server vynucuje zápis
podmíněný etagem**. Pokud ho nevynucuje, kontrola těsně před zápisem nestačí:
model našel ztrátu dat a lokální historie ji nezachrání.

Model je v `tla/Sync.tla`, spouští se `scripts/tlc.sh <cfg>`.

| Konfigurace | Server vynucuje etag | Výsledek |
| --- | --- | --- |
| `SyncEtag.cfg` (1 soubor, 3 změny, 1 pád) | ano | prošlo, 2 367 stavů |
| `SyncEtag_big.cfg` (2 soubory, 4 změny, 1 pád) | ano | prošlo včetně konvergence, 666 924 stavů, 10,5 min |
| `SyncNoEtag.cfg` | ne | `NoLoss` porušen po 6 krocích |

Kontrola, že model není bezzubý: když se u konfliktu vynechá konfliktní kopie,
TLC ztrátu najde.

## Protipříklad bez vynuceného etagu

1. Uživatel upraví soubor na Linuxu (obsah 1 → 2).
2. Démon přečte obě strany a naplánuje nahrání.
3. Démon ověří etag na serveru: sedí.
4. Uživatel mezitím uloží soubor na iPhonu (obsah 1 → 3).
5. Démon nahraje obsah 2. Obsah 3 zmizel: démon ho nikdy neviděl, takže ho
   lokální historie nemá, a přepsaná verze na serveru do koše nejde.

## Co z toho plyne

- Otázka „vynucuje server etag?“ z milníku 0 rozhoduje o bezpečnosti, ne jen
  o pohodlí. Bez testovacího účtu na ni nejde odpovědět.
- Když server etag nevynucuje, je potřeba jiné řešení než kontrola před
  zápisem. Kandidáti k ověření na API:
  - přepsání jako dvojice „nahrát nový soubor, pak přesunout starý do koše“,
    takže souběžná verze skončí v koši (A2) místo přepsání;
  - kontrola po zápisu, která ztrátu aspoň odhalí a nahlásí.
  Obojí se do modelu doplní, až bude známé chování API.
- Přesun do koše je bezpečný i bez etagu, protože server přesune skutečnou
  aktuální položku (předpoklad A2 v modelu).

## Předpoklady modelu

- A1: lokální „ověř a nahraď“ je atomické (dočasný soubor a přejmenování,
  předchozí verze jde do historie).
- A2: přesun do koše na serveru přesune aktuální položku, ne verzi, kterou
  démon viděl.
- A3: konfliktní kopie je nový soubor a nic nepřepisuje.

A2 a vynucování etagu se ověří pokusem v milníku 0. A1 a A3 jsou na
implementaci.

## Co model zatím nepokrývá

Přesuny a přejmenování, normalizaci názvů a velikost písmen, brzdu hromadných
změn a chybějící lokální složku. Doplní se před milníkem 2, kdy vzniká
`ikeloud-sync`.
