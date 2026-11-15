//! Přeložený název konfliktní kopie souboru.

use ikeloud_safety::Cesta;

use crate::{Hodnota, Preklady, Zprava};

impl Preklady {
    /// Název konfliktní kopie souboru `cesta` ve stejné složce.
    ///
    /// `datum` ve tvaru `2026-11-15`, `poradi` od 1 (vyšší číslo, když je
    /// název obsazený). Lomítko z překladu se nahradí pomlčkou, aby kopie
    /// zůstala ve stejné složce.
    pub fn nazev_konfliktni_kopie(&self, cesta: &Cesta, datum: &str, poradi: u32) -> Cesta {
        let (zaklad, pripona) = rozdel_priponu(cesta.nazev());
        let zprava = Zprava::new("konfliktni-kopie")
            .s("nazev", Hodnota::Text(zaklad.to_string()))
            .s("pripona", Hodnota::Text(pripona.to_string()))
            .s("datum", Hodnota::Text(datum.to_string()))
            .s("poradi", Hodnota::Cislo(u64::from(poradi)));
        let nazev = self.text(&zprava).replace('/', "-");
        match cesta.rodic() {
            "" => Cesta::new(nazev),
            rodic => Cesta::new(format!("{rodic}/{nazev}")),
        }
    }
}

/// `zprava.odt` → (`zprava`, `.odt`); skrytý soubor `.bashrc` příponu nemá.
fn rozdel_priponu(nazev: &str) -> (&str, &str) {
    match nazev.rfind('.') {
        Some(i) if i > 0 => (
            nazev.get(..i).unwrap_or(nazev),
            nazev.get(i..).unwrap_or(""),
        ),
        _ => (nazev, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::rozdel_priponu;

    #[test]
    fn pripony() {
        let pripady: &[(&str, (&str, &str))] = &[
            ("zprava.odt", ("zprava", ".odt")),
            ("archiv.tar.gz", ("archiv.tar", ".gz")),
            ("README", ("README", "")),
            (".bashrc", (".bashrc", "")),
            (".config.toml", (".config", ".toml")),
            ("konec.", ("konec", ".")),
        ];
        for (nazev, ocekavano) in pripady {
            assert_eq!(rozdel_priponu(nazev), *ocekavano, "{nazev}");
        }
    }
}
