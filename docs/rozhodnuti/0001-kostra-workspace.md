# 0001: Kostra workspace a CI

Datum: 2026-11-14 · Milník 0

## Rozhodnutí

Workspace má šest crates podle zadání. Povolený směr závislostí je pevný a hlídá ho
`scripts/check-deps.py` podle `dependency-policy.toml`. Závislost se do
manifestu přidá, až ji kód použije; politika říká, co je dovoleno:

```
ikeloud-daemon ──> ikeloud-sync ──> ikeloud-safety
      │                                  ^
      └──────────> ikeloud-api ──────────┘
ikeloud-cli ─────> ikeloud-api
ikeloud-gui (jen D-Bus, na ostatních crates nezávisí)
```

- `ikeloud-api` závisí na `ikeloud-safety`, protože zápisové funkce budou
  vyžadovat schvalovací typ, který umí vyrobit jen safety.
- `ikeloud-sync` na `ikeloud-api` nezávisí. Plán akcí se na server dostane
  jen přes démona a jen se schválením.
- `ikeloud-safety` nemá žádné závislosti, ani vnitřní.
- Každý externí crate smí být povolený jen u jednoho člena workspace, svého
  obalu.

Lints workspace zakazují `unwrap`, `expect`, `panic!`, `todo!`, indexování
bez kontroly a `unsafe`. V testech je `clippy.toml` povoluje.

## Co CI dělá teď

`cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --locked`,
kontrola závislostí, `cargo deny` a `cargo audit`.

## Co přijde, až bude co hlídat

| Nástroj | Kdy se zapne |
| --- | --- |
| `cargo vendor` a `cargo vet` | s první externí závislostí |
| `cargo llvm-cov` (90 % u safety a sync) | s prvním kódem v safety a sync |
| `cargo-mutants` u safety | s prvním pravidlem v safety |
| Kani | s funkcí, která posuzuje plán akcí |
| Deterministická simulace | milník 2 |

Zapnout tyto nástroje nad prázdnými crates by nic neověřilo a jen by
zpomalilo CI.
