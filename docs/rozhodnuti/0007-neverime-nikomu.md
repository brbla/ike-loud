# 0007: V cargo vet nevěříme nikomu

Datum: 2026-11-15 · rozhodnutí vlastníka projektu

## Rozhodnutí

Knihovna je prověřená jen tehdy, když ji prověřil tento projekt. Žádné
importy cizích auditů (Mozilla, Google, Bytecode Alliance, ISRG…) a žádní
důvěryhodní autoři (`cargo vet trust`).

- Importy z `supply-chain/config.toml` jsou odstraněné; dřívější import
  pokrýval 13 knihoven, teď je všech 48 ve výjimkách.
- CI selže, když se v `supply-chain/` objeví `[imports…]` nebo `[trusted…]`.
- Výjimka znamená „zatím neprověřeno“, ne „v pořádku“. Vlastní audit
  (`cargo vet certify`) výjimku nahradí.

Nahrazuje část rozhodnutí 0003 a 0005 o importu auditů.

## Co z toho plyne

Prověřit všech 48 knihoven je podle `cargo vet suggest` přes 200 000 řádků
kódu, většinu tvoří `libc`, `syn` a `num-bigint`. Pořadí auditů:

1. knihovny na cestě k heslu a k datům: `sha2`, `pbkdf2`, `hmac`,
   `num-bigint`, `getrandom`, `unicode-normalization`;
2. knihovny, které běží při sestavení (procedurální makra: `syn`,
   `quote`, `proc-macro2`, `displaydoc`, `thiserror-impl`, `serde_derive`);
3. ostatní.

Každá nová závislost tím zdraží: kromě zdůvodnění v
`dependency-policy.toml` přibude audit. To je záměr a zároveň důvod
vybírat knihovny s co nejmenším stromem závislostí.
