use std::path::Path;

/// Katalog, z którego instalator bierze gotowe głosy lektora (patrz `tauri.conf.json`,
/// `bundle.resources`). Modele mają po ~60 MB, więc **nie leżą w repozytorium** — przygotowuje się
/// je raz na maszynie budującej skryptem `tools/voices/convert.py` i wstrzykuje tutaj.
const BUNDLED_VOICES_DIR: &str = "voices";

fn main() {
    let voices = Path::new(BUNDLED_VOICES_DIR);

    // Tauri sprawdza istnienie zasobów przy każdym buildzie, także `cargo check`. Bez tego
    // katalogu praca nad kodem (i testy) wymagałaby 321 MB modeli, więc zakładamy pusty katalog:
    // aplikacja działa bez lektora, a ostrzeżenie mówi wprost, że instalator wyjdzie bez głosów.
    if !voices.is_dir() {
        if let Err(error) = std::fs::create_dir_all(voices) {
            println!(
                "cargo:warning=nie udało się założyć katalogu {}: {error}",
                voices.display()
            );
        }

        println!(
            "cargo:warning=brak głosów w {} — instalator nie dołączy lektora; \
             przygotuj je skryptem tools/voices/convert.py (patrz tools/voices/README.md)",
            voices.display()
        );
    }

    // Zmiana zawartości katalogu musi przestawić zasoby w kolejnym buildzie.
    println!("cargo:rerun-if-changed={BUNDLED_VOICES_DIR}");

    // `tauri_build` zaszywa ikonę systemową w pliku wykonywalnym (Windows: zasób `.ico`).
    // Bez tej linii podmiana plików w `icons/` nie przelicza build.rs i w exe zostaje stara ikona.
    println!("cargo:rerun-if-changed=icons");

    tauri_build::build()
}
