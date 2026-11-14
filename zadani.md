# ike loud – zadání

Nov 13, 2026 · @Brbla

## Cíl a pro koho

ike loud zpřístupní soubory z iCloud Drive na Linuxu jako běžnou složku v domácím adresáři, bez příkazové řádky. Apple oficiální klienta pro Linux nevydává, takže lidé s placeným iCloudem dnes na Linuxu zůstávají u webu icloud.com.

- **Cílový uživatel:** laik, který má iPhone nebo Mac a na počítači Linux (Ubuntu, Fedora, Mint, CachyOS). Nezná rclone ani systemd.
- **Hlavní použití:** otevřít, upravit a uložit soubor z iCloud Drive stejně jako lokální soubor; změny se objeví na iPhonu a Macu.
- **Měřítko úspěchu:** žádný ztracený ani poškozený soubor na iCloudu; nastavení do 5 minut; aplikace běží týdny bez zásahu.

## Základní principy

Když si aplikace není jistá, nic nemění a zeptá se. Pomalá nebo nedokončená synchronizace je přijatelná, zničená data na iCloudu ne.

1. **iCloud je zdroj pravdy.** Lokální složka je kopie; při pochybnosti vítězí verze na serveru a lokální se odloží stranou.
2. **Nic se nepřepisuje naslepo.** Každý zápis na server ověří, že soubor je stále ve verzi, kterou aplikace zná (etag). Jinak vznikne konfliktní kopie.
3. **Mazání je vratné.** Smazání na Linuxu jde na iCloudu do „Nedávno smazané“, nikdy trvale.
4. **Hromadné změny mají brzdu.** Když by jeden běh smazal nebo přepsal víc než nastavený limit souborů, synchronizace se zastaví a čeká na potvrzení.
5. **Neúplná data se nikdy nenahrají.** Soubor se nahrává až po dokončení zápisu; stažení jde do dočasného souboru a ten se přejmenuje až po kontrole velikosti a kontrolního součtu.
6. **Každá akce je v deníku.** Uživatel vidí, co se kdy nahrálo, stáhlo a smazalo, a může ji vrátit obnovením z koše nebo z lokální historie.

## Uživatelský zážitek

Uživatel aplikaci nainstaluje, jednou se přihlásí a pak už jen používá složku **iCloud** v domovském adresáři.

**První spuštění (průvodce):**

1. Přihlášení Apple účtem: e-mail a heslo, s nabídkou „Zapamatovat heslo v klíčence“.
2. Dvoufázové ověření: kód z iPhonu nebo SMS, se srozumitelnou nápovědou.
3. Pokud má účet zapnutou pokročilou ochranu dat (ADP), průvodce vysvětlí, že na iPhonu musí povolit „Přístup k datům iCloudu na webu“ a potvrdit výzvu.
4. Výběr složek k synchronizaci. Výchozí je vše, pokud se to vejde na disk. Nevybrané složky na Linuxu nejsou; stahování při otevření přijde až později.
5. První běh jen stahuje. Zápis na iCloud se zapne až po dokončení a kontrole prvního stažení, bez dotazu na „režim“. Režim jen pro čtení jde kdykoli zapnout v nastavení.

**Každodenní používání:**

- Stav je vidět v okně aplikace a v oznámeních: synchronizováno, probíhá, čeká na potvrzení, chyba. Ikona v liště je doplněk tam, kde ji prostředí umí; GNOME ji ve výchozím stavu nemá.
- Oznámení jen tehdy, když je potřeba jednat: konflikt, vypršelé přihlášení, zastavená hromadná změna.
- Okno aplikace: stav, deník změn, konflikty k vyřešení, nastavení, tlačítko „Otevřít složku“.
- Opakované přihlášení (Apple dává důvěru zařízení obvykle na 30 dní): aplikace upozorní tři dny předem a synchronizace mezitím bezpečně stojí. S uloženým heslem stačí zadat kód, bez něj heslo a kód.
- Rozhraní česky a anglicky; texty bez technických pojmů.

## Funkce MVP a co je mimo rozsah

První verze umí jen iCloud Drive, ale pořádně.

| Funkce | MVP | Později | Mimo rozsah |
| --- | --- | --- | --- |
| Přihlášení s 2FA, uložení relace v klíčence | ano |  |  |
| Podpora účtů s ADP | ano |  |  |
| Režim jen pro čtení | ano |  |  |
| Obousměrná synchronizace vybraných složek | ano |  |  |
| Soubory na vyžádání (stáhnout při otevření) |  | ano |  |
| Konflikty, koš, brzda hromadných změn, deník | ano |  |  |
| Tray ikona, oznámení, okno stavu | ano |  |  |
| Integrace do správce souborů (ikony stavu v Nautilu, Dolphinu) |  | ano |  |
| Více Apple účtů |  | ano |  |
| iCloud Fotky (jen čtení, export) |  | ano |  |
| Sdílené složky jiných uživatelů |  | ano |  |
| Kalendář a kontakty |  |  | ano: řeší CalDAV/CardDAV v GNOME a KDE |
| Poznámky, Klíčenka, Najdi |  |  | ano |
| Windows a macOS |  |  | ano |

## Technická architektura v Rustu

Celé je to v Rustu: synchronizační démon jako uživatelská služba a oddělená GUI aplikace, která s ním mluví přes D-Bus. Když GUI spadne nebo se zavře, synchronizace běží dál a nic se nerozbije.

**Komponenty (Cargo workspace):**

- `ikeloud-api`: klient iCloud webových služeb. Přihlášení (SRP-6a, 2FA, trust token, ADP/PCS cookies), výpis složek, stahování, nahrávání, přesun do koše. Žádná synchronizační logika.
- `ikeloud-sync`: synchronizační engine. Porovná tři stavy (lokální, vzdálený, poslední společný) a vytvoří plán akcí. Plán projde bezpečnostní vrstvou, teprve pak se provádí.
- `ikeloud-safety`: pravidla z kapitoly Ochrana dat. Jediné místo, které smí pustit zápis na server.
- `ikeloud-daemon`: běží na pozadí (ve Flatpaku přes Background portal, v nativních balíčcích jako systemd user service). Lokální změny hlásí inotify jako nápovědu; pravdu dává pravidelný úplný průchod složkou. Serveru se ptá šetrně.
- `ikeloud-gui`: průvodce, okno stavu, tray ikona, oznámení.
- `ikeloud-cli`: diagnostika pro pokročilé a pro hlášení chyb.

&#91;embedded content: architektura ike loud · 8 částí\]

Žádná cesta ze synchronizace na server nevede mimo ikeloud-safety; GUI s démonem mluví jen přes D-Bus.

**Knihovny (návrh):**

| Účel | Crate |
| --- | --- |
| Asynchronní běh, HTTP | `tokio`, `reqwest` s `rustls` |
| Přihlášení Apple | `srp`, `pbkdf2`, `sha2` |
| Stav synchronizace | `rusqlite` (SQLite v režimu WAL) |
| Hesla a tokeny | `keyring` (Secret Service: GNOME Keyring, KWallet; ve Flatpaku Secret portal) |
| Sledování souborů | `notify` |
| GUI | `gtk4` + `libadwaita` (vypadá nativně v GNOME, funguje i jinde) |
| Tray ikona | `ksni` (StatusNotifierItem) |
| IPC | `zbus` (D-Bus) |
| Soubory na vyžádání (později) | `fuser` (FUSE) |

**Odkud vzít znalost API:** oficiální dokumentace neexistuje. Vzorem jsou dvě udržované open-source implementace: backend `iclouddrive` v [rclone](https://rclone.org/iclouddrive/) (Go, od verze 1.69, podporuje i ADP) a [pyicloud](https://pypi.org/project/pyicloud/) (Python, poslední vydání říjen 2026). Obě jsou pod licencí MIT, takže port do Rustu je možný s uvedením autorů.

**Záložní plán:** pokud by vlastní klient v Rustu ve fázi 0 narazil, démon může jako transportní vrstvu použít přibalené rclone. Bezpečnostní vrstva, stav i GUI zůstanou v Rustu.

## Ochrana dat

Každá akce na serveru prochází bezpečnostní vrstvou, která ji může pustit, odložit, nebo zastavit a zeptat se uživatele.

| Situace | Co aplikace udělá |
| --- | --- |
| Soubor změněn na Linuxu i jinde od poslední synchronizace | Nepřepíše. Obě verze zůstanou, lokální jako „soubor (konflikt Linux 2026-11-13).ext“. Konflikt se ukáže v okně. |
| Smazání na Linuxu | Přesun do koše iCloudu (Nedávno smazané, 30 dní). Nikdy trvalé smazání. |
| Smazání na iCloudu | Lokální kopie jde do koše systému (freedesktop Trash), ne `rm`. |
| Jeden běh by smazal nebo přepsal víc než 50 souborů, nebo víc než 10 souborů a zároveň přes 10 % synchronizovaného obsahu | Pauza a dotaz s přehledem změn. Limit jde nastavit. |
| Lokální složka najednou chybí nebo je prázdná (odpojený disk, smazaný adresář) | Bere se jako chyba, ne jako „smazat vše“. Synchronizace stojí. |
| Server vrátí prázdný nebo podivný výpis | Bere se jako chyba API, ne jako smazaný obsah. Stav se nemění. |
| Soubor se právě zapisuje (editor, kopírování) | Nahrání až po ustálení (velikost a čas se nemění 5 s). |
| Přerušené nahrávání nebo stahování | Dočasný soubor se zahodí, akce se zopakuje. Nikdy se nepoužije neúplný soubor. |
| Vypršelé přihlášení, chyba 401/421, výpadek sítě | Vše stojí, lokální změny čekají ve frontě, uživatel dostane jedno oznámení. |
| Neznámá odpověď API (Apple něco změnil) | Bezpečný režim: jen čtení, dokud nepřijde opravená verze. |
| Názvy souborů nepovolené na iCloudu nebo lišící se jen velikostí písmen | Soubor se nenahraje, uživatel dostane vysvětlení. iCloud nerozlišuje velká a malá písmena, Linux ano. |

**Další pojistky:**

- Stav (SQLite) se zapisuje transakčně; po pádu démon pokračuje tam, kde skončil.
- Lokální historie je povinná: před každým přepsáním, lokálně i na serveru, se předchozí verze uloží stranou na 30 dní. Je to záchranná síť pro případ, že server zápis podmíněný verzí nevynucuje.
- Žádný zápis na server bez výsledku `ikeloud-safety`; dodržuje to typový systém (zápis vyžaduje schválený token akce).
- Heslo se ukládá jen tehdy, když to uživatel při přihlášení sám zaškrtne, a pouze do klíčenky systému. Jinak je v klíčence jen trust token a cookies relace.

**Pravidla pro názvy, přesuny a zvláštní soubory:**

- Přejmenování a přesun se provádí jako přejmenování a přesun (lokálně podle inode, na serveru podle ID dokumentu), nikdy jako smazání a nové nahrání.
- Názvy se porovnávají po Unicode normalizaci (NFC) a bez ohledu na velikost písmen. „Příloha“ uložená z Macu a z Linuxu je tentýž soubor.
- O konfliktu rozhoduje verze na serveru (etag) a obsah, nikdy čas změny; hodiny počítače mohou jít špatně.
- Konfliktní kopie je běžný nový soubor: nahraje se na iCloud, takže obě verze jsou vidět i na iPhonu.
- Dočasné a zamykací soubory (`.~lock.*#`, `*.swp`, `*.part`, `.DS_Store`), symbolické odkazy a speciální soubory se nesynchronizují.
- Existující neprázdná složka se při prvním nastavení nikdy tiše neslučuje; aplikace chce novou nebo prázdnou složku.
- Plné úložiště na iCloudu nebo plný disk: synchronizace stojí s jasnou zprávou, nic se nemaže.
- Odhlášení nebo odinstalace: lokální soubory zůstávají, mažou se jen tokeny.
- Síťový klient smí mluvit jen s doménami Applu z pevného seznamu; heslo nikdy neodchází jinam a hlídá to test.

**Vzdálená pojistka:** klient se jednou denně zeptá na podepsaný stavový soubor projektu. Když denní kanárek zjistí změnu API, soubor přepne všechny klienty do režimu jen pro čtení dřív, než zkusí zapisovat. Neodcházejí při tom žádná data uživatele a dotaz jde v nastavení vypnout.

## Rizika a omezení

Největší riziko je, že aplikace stojí na neoficiálním API, které může Apple kdykoli změnit.

| Riziko | Dopad | Opatření |
| --- | --- | --- |
| Apple změní webové API nebo přihlášení | Synchronizace přestane fungovat | Bezpečný režim při neznámé odpovědi; automatické kontroly proti testovacímu účtu každý den; rychlé vydávání oprav |
| Podmínky použití iCloudu | Apple neoficiální klienty nepodporuje | Jasně uvést, že nejde o produkt Apple; žádné logo Apple ani název „iCloud“ v názvu aplikace |
| Účty s ADP | Vyžadují zapnutý přístup z webu a potvrzení na zařízení | Průvodce krok za krokem; rclone ukazuje, že to jde |
| Trust token platí jen 30 dní | Uživatel se musí jednou za měsíc znovu přihlásit | Připomínka předem, pauza místo chyb |
| Omezování požadavků (rate limiting) | Zpomalení, dočasné zablokování | Exponenciální čekání, šetrné dotazování |
| Velké knihovny (desítky tisíc souborů) | Pomalé první načtení | Postupný výpis, ukazatel postupu, výběr složek |
| Balíčky dokumentů z Macu (Pages, Keynote) | Na serveru jsou to složky, ne soubory | MVP: synchronizovat jako složky a neměnit je; ověřit ve fázi 0 |

**Poznámka k názvu a ikoně:** název „ike loud“ zůstává (rozhodnuto 14. 11. 2026). Zní jako ochranná známka Applu a Flathub žádá, aby název ani ikona známky neporušovaly, takže riziko námitky trvá; záložní název je „loud ike“. Slovo „iCloud“ se proto objeví jen v popisu („neoficiální klient pro iCloud Drive“). Ikona bude vlastní návrh (například zpívající mráček), ne fotografie skutečné osoby ani prvky značky Apple.

## Testování, distribuce a fáze vývoje

Na skutečných datech se testuje až po týdnech provozu na testovacím účtu.

**Pravidlo: každá funkce má testy vstupu a výstupu.** Kód bez testů se do hlavní větve nedostane.

- Každá funkce má testy, které říkají, co jde dovnitř a co má vyjít ven, včetně okrajových případů: prázdný vstup, velké soubory, diakritika a nepovolené znaky v názvech.
- Každá funkce, která může selhat, vrací `Result` s konkrétním typem chyby. Test ověří, že pro chybný vstup vrátí přesně tu chybu, kterou má, ne jen „nějakou chybu“.
- Pro každé pravidlo z kapitoly Ochrana dat existuje test, že nebezpečná akce byla zastavena.
- Testy jsou tabulkové (vstup → očekávaný výstup), aby šly snadno doplňovat.
- V produkčním kódu žádné `unwrap()`, `expect()` ani `panic!`; hlídá to `clippy`.
- CI při každé změně: `cargo test`, `cargo clippy -D warnings`, pokrytí (`cargo llvm-cov`, minimum 90 % u `ikeloud-safety` a `ikeloud-sync`) a mutation testing (`cargo-mutants`) u bezpečnostní vrstvy. Ten ověří, že testy opravdu chytí změnu chování.

Příklad pro funkci bezpečnostní vrstvy, která posuzuje plán akcí:

| Vstup | Očekávaný výstup |
| --- | --- |
| Plán: nahrát 1 soubor, etag na serveru sedí | `Ok(Schváleno)` |
| Plán: nahrát 1 soubor, etag na serveru se změnil | `Err(Konflikt)` |
| Plán: smazat 120 z 200 souborů | `Err(HromadnaZmenaCekaNaPotvrzeni)` |
| Plán: smazat vše, lokální složka chybí | `Err(LokalniSlozkaNedostupna)` |
| Prázdný plán | `Ok(NicKProvedeni)` |

**Formální ověření:** synchronizační algoritmus se ověří dřív, než se napíše, a bezpečnostní vrstva se ověřuje přímo v Rust kódu.

| Nástroj | Co ověřuje | Kdy |
| --- | --- | --- |
| TLA+ (model checker TLC) | Model protokolu: tři stavy, souběžné změny na obou stranách, výpadky, pády. Invarianty: žádný obsah nezmizí bez koše nebo konfliktní kopie; po ustálení se obě strany shodují. | Fáze 0, před psaním enginu; model se aktualizuje s každou změnou algoritmu |
| Kani (model checker pro Rust) | Kritické funkce `ikeloud-safety` a sestavení plánu: pro všechny vstupy do dané velikosti platí například „plán s mazáním nad limitem nikdy nedostane schválení“. | Každá změna v CI |
| Deterministická simulace | Celý engine proti falešnému serveru s řízeným časem a náhodnými poruchami; selhání jde přehrát ze seedu. Inspirace: Dropbox Nucleus. | Každá změna v CI (tisíce scénářů), noční běh (statisíce) |

Plný důkaz (Lean, Verus, Creusot nebo Aeneas) je volitelný a jen pro funkci, která ze tří stavů sestavuje plán akcí. Rozhodnutí padne po fázi 2 podle toho, co odhalí TLA+ a simulace.

**Cizí kód:** pro knihovny, převzatý kód i přibalené programy platí stejná pravidla jako pro vlastní kód. Žádná aktualizace nesmí změnit chování aplikace, aniž by selhal test.

- **Obal pro každou knihovnu.** Aplikace volá cizí kód jen přes vlastní tenký modul. Zbytek kódu knihovnu přímo nevidí a hlídá to pravidlo v CI.
- **Testy smlouvy.** Každá cizí funkce, kterou obal používá, má tabulku vstupů a očekávaných výstupů včetně okrajových a chybových případů. Změna výstupu (nula místo prázdné hodnoty, jiný znak, jiná chyba) shodí CI.
- **Přesně připnuté verze.** `Cargo.lock` je v repozitáři, verze jsou zapsané přesně (`=1.2.3`) a sestavuje se s `--locked`. Zdrojové kódy závislostí jsou uložené u projektu (`cargo vendor`), takže se vždy sestavuje ze stejného kódu.
- **Aktualizace jen záměrně.** Vždy jedna knihovna na jednu změnu, s přečteným rozdílem kódu, záznamem v `cargo vet` a všemi testy včetně simulace. Automatické aktualizace se nepouštějí.
- **Kontroly v CI.** `cargo deny` (licence, zdroje, zakázané a duplicitní knihovny) a `cargo audit` (známé zranitelnosti).
- **Co nejméně závislostí.** Výchozí funkce knihoven jsou vypnuté a každá nová knihovna potřebuje zdůvodnění. `ikeloud-safety` nepoužívá nic kromě standardní knihovny.
- **Převzatý kód** z rclone nebo pyicloud se po portu do Rustu stává vlastním kódem se všemi testy. Záložní rclone by bylo připnuté na verzi s kontrolním součtem a mělo by vlastní testy smlouvy.
- **Systémové knihovny.** SQLite je přibalená v pevné verzi. GTK přichází z prostředí Flatpaku v připnuté verzi a neleží na cestě k datům.
- **Server Applu** je také cizí kód: denní kanárek je test smlouvy pro jeho odpovědi.

Změnu typu zachytí Rust už při překladu. Změnu hodnoty při stejném typu zachytí testy smlouvy.

**Testování:**

- Samostatný testovací Apple účet s iCloud Drive pro všechny integrační testy; nikdy ne účet vývojáře.
- Simulátor serveru (fake `ikeloud-api`) pro testy engine: konflikty, výpadky, prázdné výpisy, přerušené přenosy.
- Property-based testy (`proptest`): náhodné sekvence změn na obou stranách; tvrzení, že žádný obsah nezmizí bez záznamu v koši nebo konfliktu.
- Testy pádu: zabít démona v náhodném okamžiku a ověřit konzistenci po restartu.
- Kontrolní běh na pozadí: porovnání kontrolních součtů lokálně a na serveru, hlášení rozdílů.

**Distribuce:**

- Flatpak na Flathubu jako hlavní kanál (jedna instalace pro všechny distribuce).
- AppImage a balíčky .deb a .rpm z GitHub Releases.
- AUR pro Arch a CachyOS.
- Open source (MIT nebo Apache-2.0), aby kódu důvěřovali i lidé, kteří mu svěřují Apple účet.

**Fáze:**

1. **Fáze 0, ověření:** přihlášení vlastním Rust klientem (SRP, 2FA, ADP), výpis, stažení, nahrání a přesun do koše na testovacím účtu. Rozhodne, zda vlastní klient, nebo záložní rclone. Současně vznikne TLA+ model synchronizace, který musí projít před začátkem fáze 2.
2. **Fáze 1, jen čtení:** démon a GUI, které iCloud pouze stahují do lokální složky. Na server se nezapisuje nic.
3. **Fáze 2, obousměrná synchronizace:** bezpečnostní vrstva, konflikty, brzda, deník. Uzavřená beta s několika uživateli.
4. **Fáze 3, veřejné vydání 1.0:** Flathub, česká a anglická lokalizace, dokumentace pro laiky.
5. **Fáze 4, rozšíření:** soubory na vyžádání (FUSE), integrace do správců souborů, Fotky jen pro čtení.

Podrobné milníky s výstupními kritérii a odhady: Plán realizace

## Zdroje

- [rclone: iCloud Drive backend](https://rclone.org/iclouddrive/)
- [pyicloud na PyPI](https://pypi.org/project/pyicloud/2026.10.1/)
- [icloud-linux (FUSE mount v Pythonu)](https://github.com/Bougakov/icloud-linux)
