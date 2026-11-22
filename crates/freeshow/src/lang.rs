// SPDX-License-Identifier: GPL-3.0-only

/// Drawer tab id, full label, and compact label for `labelsDisabled`.
pub fn drawer_tabs(language: Option<&str>) -> [(&'static str, &'static str, &'static str); 8] {
    match language
        .unwrap_or("en")
        .replace('_', "-")
        .to_ascii_lowercase()
        .as_str()
    {
        "es" => [
            ("shows", "Canciones", "C"),
            ("media", "Medios", "M"),
            ("audio", "Audio", "A"),
            ("overlays", "Superposiciones", "S"),
            ("templates", "Plantillas", "P"),
            ("scripture", "Escritura", "E"),
            ("calendar", "Calendario", "L"),
            ("functions", "Funciones", "F"),
        ],
        "de" => [
            ("shows", "Lieder", "L"),
            ("media", "Medien", "M"),
            ("audio", "Audio", "A"),
            ("overlays", "Overlays", "O"),
            ("templates", "Vorlagen", "V"),
            ("scripture", "Bibel", "B"),
            ("calendar", "Kalender", "K"),
            ("functions", "Funktionen", "F"),
        ],
        "fr" | "fr-fr" => [
            ("shows", "Chants", "C"),
            ("media", "Médias", "M"),
            ("audio", "Audio", "A"),
            ("overlays", "Superpositions", "S"),
            ("templates", "Modèles", "T"),
            ("scripture", "Écritures", "É"),
            ("calendar", "Calendrier", "L"),
            ("functions", "Fonctions", "F"),
        ],
        "nb" | "nn" | "no" => [
            ("shows", "Sanger", "S"),
            ("media", "Media", "M"),
            ("audio", "Lyd", "L"),
            ("overlays", "Overlegg", "O"),
            ("templates", "Maler", "T"),
            ("scripture", "Skrift", "K"),
            ("calendar", "Kalender", "A"),
            ("functions", "Funksjoner", "F"),
        ],
        _ => [
            ("shows", "Shows", "S"),
            ("media", "Media", "M"),
            ("audio", "Audio", "A"),
            ("overlays", "Overlays", "O"),
            ("templates", "Templates", "T"),
            ("scripture", "Scripture", "B"),
            ("calendar", "Calendar", "C"),
            ("functions", "Functions", "F"),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_packs_keep_the_same_tab_ids() {
        for lang in [
            None,
            Some("en"),
            Some("es"),
            Some("nb"),
            Some("de"),
            Some("fr"),
        ] {
            let tabs = drawer_tabs(lang);
            assert_eq!(tabs[0].0, "shows");
            assert_eq!(tabs[7].0, "functions");
            assert!(!tabs[0].1.is_empty());
        }
        assert_eq!(drawer_tabs(Some("nb"))[0].1, "Sanger");
        assert_eq!(drawer_tabs(Some("en"))[0].1, "Shows");
    }
}
