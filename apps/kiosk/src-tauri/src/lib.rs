//! Aplikacja kiosku ANON DJ: gość wyszukuje utwór, wybiera go i wpisuje dedykację.
//!
//! Kiosk **nie zawiera** skanera biblioteki, TTS ani kodu odtwarzacza (PLAN.md, sekcja 4.2).
//! Nie prowadzi też własnej bazy — trwałe dane trzyma wyłącznie aplikacja DJ-a.

use tracing::info;
use tracing_subscriber::EnvFilter;

/// Informacje o kiosku. Na ekranie gościa nie pokazujemy nic z tego.
#[derive(Debug, serde::Serialize)]
struct AppStatus {
    version: &'static str,
    protocol_version: u16,
}

#[tauri::command]
fn app_status() -> AppStatus {
    AppStatus {
        version: env!("CARGO_PKG_VERSION"),
        protocol_version: protocol::PROTOCOL_VERSION,
    }
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // Kiosk loguje tylko na konsolę — trwałe logi prowadzi aplikacja DJ-a.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|_app| {
            init_logging();
            info!("kiosk ANON DJ gotowy");

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![app_status])
        .run(tauri::generate_context!())
        .expect("nie udało się uruchomić kiosku ANON DJ");
}
