# 0024: Relace v klíčence

Datum: 2026-11-17 · Milník 1

## Rozhodnutí

Přihlášená relace (cookies a důvěryhodný token; heslo nikdy) je v
klíčence systému přes D-Bus rozhraní Secret Service: na Plasmě
`ksecretd` (KWallet), na GNOME GNOME Keyring. Jedna položka s atributy
`application=io.github.brbla.IkeLoud`, `type=session`.

- Crate `ikeloud-keyring` nad GIO D-Bus: stejné GLib/GIO, které už
  aplikace má kvůli GTK; žádná nová knihovna ve `vendor/`. Démon je tím
  linkován proti GLib, což na desktopu s GTK nevadí.
- Tajemství jde v „plain“ relaci protokolu: po sběrnici relace mezi dvěma
  procesy téhož uživatele. Šifrovaná relace (DH) by vyžadovala další
  kryptografii; přínos proti útočníkovi, který už čte sběrnici uživatele,
  je malý.
- Bez klíčenky, nebo když je zamčená a chtěla by heslo (démon dialog
  ukázat nemůže), zůstává relace v souboru `session.json` s právy 0600
  jako dosud.
- Soubor má přednost: je novější (z verze před klíčenkou, nebo zapsaný,
  když klíčenka nebyla). Při načtení se přesune do klíčenky a smaže.
- Do klíčenky se zapisuje jen při změně (démon ukládá relaci po každém
  volání).
- Odhlášení položku smaže v klíčence i soubor.

## Testy

`cargo test -p ikeloud-keyring -- --ignored` proti klíčence relace (CI ji
nemá): uložení, načtení, nahrazení, smazání; přesun souboru do klíčenky.
Testy používají vlastní druh položky, skutečné relace se nedotknou.
Ověřeno s `ksecretd` (Plasma 6).
