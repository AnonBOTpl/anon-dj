fn main() {
    // `tauri_build` zaszywa ikonę systemową w pliku wykonywalnym (Windows: zasób `.ico`).
    // Bez tej linii cargo nie przelicza build.rs po podmianie plików w `icons/`, więc w exe
    // zostaje **stara** ikona — dokładnie to się zdarzyło przy zmianie ikony kiosku.
    println!("cargo:rerun-if-changed=icons");

    tauri_build::build()
}
