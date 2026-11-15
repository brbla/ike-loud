# 0013: Kvalita testů v CI a co z revize zbývá

Datum: 2026-11-15 · Navazuje na 0001, 0008 a 0012

Nezávislá revize upozornila, že část postupů ze zadání byla jen
deklarovaná: zápis 0001 sliboval pokrytí, mutation testing a Kani „s prvním
kódem“, a ten už dávno existoval. README dokonce tvrdilo, že bezpečnostní
vrstvu Kani hlídá. Tento záznam říká, co je teď skutečně zapnuté a co ne.

## Zapnuto

**Pokrytí** (`cargo llvm-cov`, job `quality`): `ikeloud-safety`
a `ikeloud-sync` musí mít aspoň 95 % řádků. Zadání chce 90 %, ale kód má
98,8 %, a laťka u současného stavu nedovolí, aby pokrytí potichu klesalo.

**Mutation testing** (`cargo-mutants`, `scripts/mutants.sh`) bezpečnostní
vrstvy, jen s jejími vlastními testy. Bezpečnostní vrstva musí být správná
sama o sobě, ne jen díky testům jiných crate. První běh odhalil 11 přeživších
mutantů: `LocalId::same_file`, `RelPath::parent` a další se testovaly jen
v `ikeloud-sync`. Doplnily se testy (`tests/types.rs`) a teď nepřežije žádný
mutant (101 chycených, 11 nepřeložitelných).

Mutace trvají asi 7 minut, a proto se úspěšný běh pamatuje v keši. Klíčem je
otisk všech vstupů běhu (`scripts/ci-cache-key.sh --mutants`): zdrojové
kódy a testy safety, manifesty, lock, konfigurace cargo, toolchain,
proměnné prostředí, verze nástroje a samotný skript. Test klíče ověřuje, že
každý z těchto vstupů klíč mění a kód jiných crate ne. Je to jediný
výsledek, který se kešuje (pravidlo 10 ve skillu `ci-cache`). Nevyhovující
běh se nepamatuje nikdy.

**Přesné verze závislostí** hlídá `scripts/check-deps.py`. Dosud to bylo
jen dohodnuté pravidlo.

**Převody přes `as`** zakazuje lint `clippy::as_conversions`.
V produkčním kódu byl jediný.

**Připnuté verze nástrojů** jsou teď u jobu, který nástroj používá, ne pro
celý workflow. Klíč keše závislostí čte proměnné `CARGO_*`, takže
aktualizace nástroje by zbytečně zahodila zkompilované závislosti. Klíč keše
nástrojů najde připnuté verze podle vzoru `CARGO_*_VERSION` a nese jméno
sady nástrojů, protože dva joby se stejným klíčem a jiným obsahem by si
navzájem přepisovaly keš.

**Simulace v CI** běží s optimalizací pro `ikeloud-sync`, `ikeloud-safety`
a `unicode-normalization` i v debug buildu. Místo 30 s trvá 3 s.

**Kani** (job `kani`) dokazuje pro všechny vstupy dvě vlastnosti brzdy
(`crates/ikeloud-safety/src/proofs.rs`):

- výchozí limity: do 10 smazaných souborů nikdy nebrzdí, nad 50 vždy, pro
  jakýkoli počet synchronizovaných souborů (0,1 s);
- víc změn brzdu nikdy nevypne, pro všechny hodnoty `u64` včetně limitů
  (108 s).

Třetí vlastnost („víc synchronizovaných souborů brzdu nezapne“) solver
nedokázal ani za 15 minut, ani s mezí 4 miliardy souborů, a proto byla
vyřazena. Kani se instaluje z předpřipraveného balíku z GitHubu (140 MB).
Podle zásady 0007 je verze i kontrolní součet balíku připnutý ve
`scripts/kani-setup.sh`, stejně jako u `tla2tools.jar`. Součet pochází
z našeho vlastního stažení. Stejně jako mutace se úspěšný běh pamatuje pod
otiskem všech vstupů (`ci-cache-key.sh --kani`), takže se Kani instaluje
a spouští jen při změně bezpečnostní vrstvy. Mutace soubor s důkazy
vynechávají: v běžném buildu se nepřekládá, a jeho mutanty by proto nešlo
chytit.

**`RelPath` je vždy platná cesta.** Jediný způsob, jak ji vytvořit, je
`RelPath::parse`. Ta odmítne prázdnou cestu, prázdnou část (`a//b`, `/a`),
`.` a `..` (cesta tedy nikdy nevede ven ze synchronizované složky) a znak
NUL. Cesty přečtené z databáze se kontrolují znovu a vadná je poškozený
záznam. Překlad, ze kterého by vznikl neplatný název konfliktní kopie, se
neuplatní. `names::validate_path` už kontroluje jen pravidla serveru
(délku a řídicí znaky); struktura cesty je na jednom místě.

## Vědomě ne (schválil uživatel 2026-11-15)

**proptest.** Zadání chce property-based testy s náhodnými sekvencemi změn
na obou stranách a tvrzením, že žádný obsah nezmizí. Přesně to dělá
deterministická simulace (0011, 0012), a to vlastním generátorem bez
závislostí. Proptest by přidal přes deset crate, které podle zásady
„nevěříme nikomu“ (0007) nikdo z nás nečetl. Jediná jeho výhoda navíc je
zmenšování chybového případu, a simulace místo toho vypisuje seed a celý
průběh. **Rozhodnutí:** proptest nepřidávat. Kdyby se zmenšování ukázalo jako
potřebné, napíšeme ho do simulace sami.

**zeroize** pro tajemství SRP. `BigUint` drží tajemství ve `Vec<u64>`, které
bez unsafe kódu vymazat nejde. Vymazat jen některé kopie by předstíralo
ochranu, která neexistuje. Klient žije jedno přihlášení. Skutečné opatření
patří do démona: proces bez core dumpů (`PR_SET_DUMPABLE`), milník 1.
**Rozhodnutí:** zeroize nepřidávat, místo něj zákaz core dumpů v démonu.

## Zbývá

- **`cargo vet` má 60 výjimek a žádný audit.** Projde, ale nic tím
  neověřuje. Audity musí udělat člověk; seznam výjimek je dluh, který se
  bude splácet od nejmenších crate.
- **TLA+ modely** nepokrývají NFC, uložení novým souborem, identitu
  s časem vzniku ani brzdu. Tyto věci zatím pokrývá jen simulace.
