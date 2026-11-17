# 0021: Klient iCloudu v Rustu a co ukázal skutečný účet

Datum: 2026-11-16 · Milník 0 a 1

## Rozhodnutí

`ikeloud-api` umí přihlášení (SRP, kód z SMS, důvěryhodný token), ověření
uložené relace, výpis složky a stažení souboru. Vše jen pro čtení.
`ikeloud-cli login | ls | get` je ruční zkouška proti skutečnému účtu.

- **HTTP: `ureq` 3 nad `rustls` (`ring`) s přibalenými kořenovými
  certifikáty (`webpki-roots`).** Blokující klient bez asynchronního
  běhového prostředí; démon synchronizuje po jednom požadavku. Varianta
  se systémovým OpenSSL (`native-tls`) by přinesla víc knihoven,
  procedurální makra a závislost na knihovně systému. Na Linuxu přibylo
  asi 20 knihoven.
- **`getrandom` 0.2 místo 0.3.** `ring` potřebuje 0.2 a `deny.toml`
  nepovoluje dvě verze. Volání je stejné; z `vendor/` zmizely `wasip2`,
  `wit-bindgen` a `r-efi`.
- **JSON přes `serde_json`, base64 a cookies vlastní** (`b64.rs`,
  `cookies.rs`): pár desítek řádků místo dalších knihoven.
- **Pevný seznam domén** (`http::host_allowed`): `idmsa.apple.com`,
  `*.icloud.com`, `*.icloud-content.com`. Jen https, bez přesměrování,
  cookies posílá jen náš kód a jen do jejich domény.
- **Logika je oddělená od sítě** (`Transport`). Testy (`tests/client.rs`)
  běží proti falešnému serveru, který odpovídá ve tvarech zaznamenaných ze
  skutečné služby.

## Co ukázal testovací účet (pokusy 16. 11. 2026)

| Otázka | Odpověď |
| --- | --- |
| Projde naše SRP? | Ano, `s2k`, 20 920 iterací, zarovnání čísel podle rclone. Vzácný případ s kratším A (~1 %) ověřený zatím není. |
| Dvoufázové ověření | Webový účet bez zařízení Apple ho má vždy (`hsa2`); kód přijde SMS na ověřené číslo. E-mailem Apple kódy neposílá. Hlasový hovor jako náhradní cesta zatím nevyzkoušen. |
| Opakování SMS | Jde; odpověď hlásí `tooManyCodesSent`, `securityCodeCooldown`, `securityCodeLocked`. |
| Druhé přihlášení bez SMS | Ano: s důvěryhodným tokenem z `2sv/trust` stačí heslo (ověřeno klientem v Rustu). Doba platnosti tokenu neznámá. |
| Bez hesla | Uložené cookies stačí na `validate` a na práci s Drive, dokud platí. Doba platnosti neznámá; změří 14denní běh v milníku 1. |
| Nový účet | Drive vrací `402 CONTAINER_NEEDS_MIGRATION`, dokud se jednou neotevře na icloud.com. Aplikace to musí srozumitelně říct (`ApiError::DriveNotSetUp`). |
| ADP | Na testovacím účtu vypnuté; zapnutí je samostatný pokus. |
| Výpis | Položka má neměnné `docwsid`, `parentId`, `etag`; přípona je zvlášť; `dateModified` je čas souboru, ne nahrání. |
| Etagy | Různé tvary (`a`, `6`, `9::8`), porovnávají se jen na shodu. Po nahrání do podsložky se změnil etag podsložky i kořene: **hypotéza**, že stačí sledovat etag kořene. Ověří se víc pokusy, než se na ni engine spolehne. |
| Stažení | `docws …/download/by_id` vrátí podepsanou adresu na `cvws.icloud-content.com`; obsah sedí s velikostí ve výpisu. |

## Bezpečnost

- Odpověď na ověření kódu obsahuje **celé** telefonní číslo. Odpovědi
  přihlašovací služby se proto nikdy nelogují ani nevkládají do chyb;
  chyby nesou jen stav a popis, `Debug` relace ukazuje jen počty a příznaky.
- Heslo se neukládá. CLI drží relaci (cookies, token) v
  `$XDG_STATE_HOME/ikeloud/cli-session.json` s právy 0600; démon ji bude
  držet v klíčence (Secret Service).
- Kořenové certifikáty jsou zkompilované v programu: aktualizují se s
  novou verzí `webpki-roots`, ne se systémem.

## Dodavatelský řetězec

- Nové knihovny jsou ve výjimkách `cargo vet` (0007: nevěříme nikomu).
  Audit přednostně `rustls`, `rustls-webpki`, `ring`, `webpki-roots`,
  `ureq`, `ureq-proto`, `httparse`, `serde_json`.
- `ring` sestavuje C a assembler; jeho proměnné prostředí (`RING_*`,
  `PERL_EXECUTABLE`) jsou v klíči cache v CI (0008).
- `cargo vendor` stáhne i knihovny pro Windows (`windows-sys` a
  `windows_*`, ~60 MB), protože je `ring` uvádí pro cíl Windows. Na
  Linuxu se nesestavují. Zmenšení (`cargo-vendor-filterer`) je otevřené.
