# 0018: Opakované spouštění a sledování složky

Datum: 2026-11-15 · Milník 1 a 2 (bez Apple účtu)

## Rozhodnutí

Démon (`ikeloud_daemon::Daemon`) opakuje běhy synchronizace (0016). Kdy
spustit další běh, rozhoduje `Schedule`. Je to čistá logika nad časy, takže
se testuje bez čekání.

| Situace | Další běh | Výchozí hodnota |
|---|---|---|
| Start démona | hned | |
| Změna ve složce (nápověda z inotify) | až je složka v klidu po dobu ustálení | 5 s |
| Složka se mění pořád | nejpozději od první změny po | 30 s |
| Mezi dvěma běhy | nejméně | 10 s |
| Běh něco udělal | hned znovu (po rozestupu), aby se ověřilo ustálení; nejvýš 3× za sebou | |
| Soubory se ještě zapisují | po době ustálení | 5 s |
| Bez změn | pravidelná kontrola | 5 min |
| Běh selhal (offline, chybí složka, databáze…) | čekání 30 s, pak 1, 2, 4… min | nejvýš 30 min |
| Uživatel řekl „synchronizovat“ nebo potvrdil hromadnou změnu | hned (po rozestupu), i během čekání po chybě | |
| inotify ztratil události | hned (po rozestupu) | |
| Úplné přečtení všech souborů | jednou denně | 24 h |

Hodnoty jsou v `Timing::DEFAULT`. Pravidelnou kontrolu a čekání po chybách
doladíme v milníku 0 podle toho, kolik dotazů snese účet a jestli jde levně
zjistit změnu na serveru (zadání, otevřené otázky). Dřív je určit nejde.

### Proč tato čísla

- **5 min pravidelná kontrola.** Změny z iPhonu vidí démon jen z výpisu
  serveru. Každý běh vypisuje celý strom, takže častější dotazy riskují
  omezování účtu (zadání: šetrné dotazování). Zpoždění v minutách zadání
  připouští.
- **5 s klidu.** Rovná se době ustálení. Soubor změněný před 5 s je hotový
  k nahrání, takže běh ho vezme napoprvé, místo aby ho odložil a čekal na
  další běh.
- **30 s nejpozději.** Při dlouhém kopírování se aspoň hotové soubory
  nahrají průběžně.
- **10 s rozestup.** I při neustálých změnách nejvýš 6 výpisů serveru za
  minutu.
- **Nejvýš 3 opakování po změně.** Když se stav neustálí ani po třech
  bězích, něco se mění pořád. Démon pak přejde na pravidelnou kontrolu
  a nezahltí server.
- **Čekání po chybách do 30 min.** Výpadek neznamená stovky marných dotazů.
  Uživatel může čekání kdykoli zkrátit.
- **Během chyb lokální změny běh neurychlí.** Bez serveru nebo bez složky by
  běh stejně nic neudělal.

### Pořadí sledování a skenu

inotify je jen nápověda, pravdu dává úplný sken (zadání). Před každým skenem
se znovu přidají sledování všech složek (`Watches::watch_all`; přidání
podruhé nevadí). Co se změní potom, ohlásí inotify. Co se změnilo předtím,
vidí sken. Nová podsložka se tím začne sledovat nejpozději při dalším běhu,
a ten její vytvoření sám vyvolá. Změny, které přijdou během běhu, vyvolají
další běh. Když systém nedovolí sledovat všechny složky
(`fs.inotify.max_user_watches`), démon to hlásí (`Coverage::failed`)
a změny v nesledovaných složkách zachytí pravidelná kontrola.

## Paměť otisků (nalezeno při návrhu)

Sken dosud četl celý obsah každého souboru při každém běhu. S během po každé
změně by démon u složky se stovkami GB pořád četl disk. To by porušilo
lehkost ze zadání: v klidu pod 1 % procesoru.

`ikeloud_local::HashCache` si pamatuje otisky z předchozího skenu. Otisk se
použije znovu jen pro tentýž soubor (zařízení, inode, čas vzniku) se stejnou
velikostí, časem změny obsahu (mtime) a časem změny metadat (ctime), vše na
nanosekundy:

- ctime nejde nastavit žádným programem a každý zápis ho posune. Proto
  nestačí vrátit mtime nebo zachovat velikost (test
  `unchanged_files_are_not_read_again`).
- Zbývá zápis ve stejném tiku hodin jako čtení. Souborové systémy mají hrubé
  časy, FAT dokonce dvousekundové. Otisk se proto použije znovu, jen když
  soubor byl při čtení nezměněný aspoň 3 s. Do té doby se čte znovu. Stejně
  to dělá git s „racily clean“ soubory.
- Zbývá jen ručně vrácené hodiny na tutéž nanosekundu. Proto se jednou
  denně přečte všechno.

Po neúspěšném skenu zůstane paměť, jaká byla.

## Nová závislost

`inotify` 0.11.5 (a `inotify-sys` 0.1.8) bez výchozích funkcí, tedy bez
async. Volá je jen `ikeloud-local` (`src/watch.rs`). Standardní knihovna
inotify neumí a náš kód nesmí obsahovat `unsafe`. `libc` a `bitflags` už
ve stromu byly. Ve `supply-chain/` jsou nové knihovny jako výjimky čekající
na audit (0007). Sestavovací skript `inotify-sys` čte jen cílový systém.

## Testy

- `schedule.rs`: 9 testů rozvrhu se smyšlenými časy (klid, nejpozdější běh,
  rozestup, změna během běhu, opakování po změně, čekání po chybách
  a jeho zkrácení, přetečení).
- `ikeloud-local/tests/watch.rs`: nápovědy ze skutečné složky (hluboký
  soubor, přesun, nová podsložka před a po `watch_all`, smazaný strom).
- `ikeloud-local/tests/scan.rs`: paměť otisků (hrubé časy, vrácený mtime,
  smazaný soubor, `clear`, neúspěšný sken).
- `ikeloud-daemon/tests/daemon.rs`: skutečný démon v reálném čase se
  zkrácenými časy. Lokální změna se nahraje bez čekání na pravidelnou
  kontrolu; bez sledování běh spustí jen uživatel; nedostupný server se
  zkouší s rostoucím čekáním a po návratu se vše dorovná.
- Simulace démona (0017) drží paměť otisků mezi běhy jako démon. S okrajem
  3 s se v rychlé simulaci otisky znovu nepoužijí; správnost paměti
  ověřují testy skeneru.

## Co zbývá

- `main` démona stále končí kódem 2: chybí skutečný klient iCloudu
  (milník 0). Smyčka, sledování i rozvrh jsou připravené.
- Paměť otisků je jen v paměti procesu. Po restartu démona se celá složka
  jednou přečte. Uložení do databáze přijde, pokud to bude u velkých složek
  vadit.
- Čekání po chybách nemá náhodný rozptyl. Klienti po výpadku serveru
  selhávají v různých chvílích, takže se zatím nesejdou. Znovu to posoudíme
  s omezováním požadavků v milníku 0.
