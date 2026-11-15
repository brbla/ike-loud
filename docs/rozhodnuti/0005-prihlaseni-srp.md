# 0005: Přihlášení SRP-6a ve variantě Applu

Datum: 2026-11-15 · Milník 0

## Rozhodnutí

`ikeloud_api::srp` počítá klientskou stranu přihlášení: odvození hesla
(`s2k`, `s2k_fo`), veřejné číslo A a z odpovědi serveru důkazy M1, M2
a klíč K. Kód vychází z rclone (`backend/iclouddrive/api/srp.go`, MIT)
a výsledky jsou ověřené proti knihovně pysrp 1.0.22, kterou používá
pyicloud (MIT). Síťová část (`signin/init`, `signin/complete`, 2FA) přijde
nad tímto modulem.

Vlastní implementace místo crate `srp`: ten počítá `x` se jménem účtu
a Apple ho tam nemá.

## Co testy ověřují

- Odvození hesla a celá výměna dávají bajt po bajtu totéž co pysrp, a to
  v obou jeho backendech (`_ctsrp` nad OpenSSL i čistý Python). Server
  pysrp důkaz klienta přijal.
- Okrajové případy, kde se varianty liší, pro každou variantu zvlášť.
- Chybné vstupy: B mimo rozsah, nulové tajemství, nulové iterace, neznámý
  protokol.

Generátor vektorů je v `scripts/srp-vektory/`.

## Co zjistil rozbor (otázky pro milník 0)

Implementace, které s Applem fungují, se v podrobnostech liší:

| Kde | rclone | pysrp (čistý Python) | pysrp `_ctsrp` (výchozí) |
| --- | --- | --- | --- |
| A, B v M1 a M2 | doplněné na 256 B | bez úvodních nul | bez úvodních nul |
| S před výpočtem K | doplněné na 256 B | bez úvodních nul | bez úvodních nul |
| salt v x a M1 | bajty beze změny | bajty beze změny | úvodní nuly zahodí |

Rozdíl nastává, jen když číslo vyjde kratší: A nebo S zhruba v 1 %
přihlášení, salt s úvodní nulou v 1/256. Jedna z implementací tedy občas
selže a vypadá to jako náhodná chyba přihlášení.

Implementace umí obě varianty čísel (`Zarovnani`), výchozí je rclone.
Apple corecrypto podle všeho převádí čísla na pevnou délku skupiny, ale
ověřené to není. Salt se bere jako bajty beze změny.

**Pokus na testovacím účtu:** klient si tajné číslo `a` volí sám. Zvolí se
takové, aby A vyšlo kratší než 256 bajtů, a zkusí se přihlášení s každou
variantou. Přijatá varianta se nastaví jako jediná a druhá se odstraní.
Na salt s úvodní nulou se nedá cílit (volí ho server); ověří se
opakovaným přihlášením nebo se nechá podle rclone.

## Bezpečnost

- `num-bigint` nepočítá v konstantním čase. Útok měřením času by
  vyžadoval přístup k počítači uživatele během přihlášení; pro desktopového
  klienta je to přijatelné. Přechod na `crypto-bigint` je možný později.
- Tajné číslo se nevypisuje (`Debug`) a nikam se neukládá. Mazání z paměti
  (`zeroize`) zatím není.

## Dodavatelský řetězec

Nové závislosti: `num-bigint`, `sha2`, `pbkdf2`, `getrandom` (přesně
připnuté, jediný obal je `srp.rs`) a jejich závislosti. Veřejné audity
pokryly část; 17 knihoven má v `supply-chain/config.toml` výjimku se
zdůvodněním. `cargo vet suggest` navrhuje důvěřovat autorům, kterým věří
Mozilla (Josh Stone pro `num-*`, rust-lang pro `cfg-if` a `libc`), nebo
importovat audity ISRG a Zcash. To je rozhodnutí o důvěře a čeká na
vlastníka projektu.
