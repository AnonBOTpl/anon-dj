//! Aplikacja DJ-a ANON DJ: konfiguracja uruchomienia, stan współdzielony i komendy Tauri.

mod db;
mod logging;
mod settings;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::Manager;
use tracing::{error, info, warn};

use crate::db::Db;
use crate::settings::AppSettings;

/// Komunikat zwracany, gdy wcześniejszy błąd zostawił ustawienia w stanie zatrutym.
const SETTINGS_UNAVAILABLE: &str = "ustawienia są chwilowo niedostępne";

/// Stan aplikacji współdzielony między komendami Tauri.
struct AppState {
    db: Arc<Db>,
    db_path: PathBuf,
    log_dir: PathBuf,
    /// Efektywne ustawienia trzymane w pamięci, żeby walidacja nie sięgała do bazy przy każdym komunikacie.
    settings: Mutex<AppSettings>,
}

impl AppState {
    fn new(db: Db, db_path: PathBuf, log_dir: PathBuf, settings: AppSettings) -> Self {
        Self {
            db: Arc::new(db),
            db_path,
            log_dir,
            settings: Mutex::new(settings),
        }
    }
}

/// Informacje o aplikacji pokazywane w pasku statusu DJ-a.
#[derive(Debug, serde::Serialize)]
struct AppStatus {
    version: &'static str,
    protocol_version: u16,
    db_path: String,
    log_dir: String,
}

#[tauri::command]
async fn app_status(state: tauri::State<'_, AppState>) -> Result<AppStatus, String> {
    Ok(AppStatus {
        version: env!("CARGO_PKG_VERSION"),
        protocol_version: protocol::PROTOCOL_VERSION,
        db_path: state.db_path.display().to_string(),
        log_dir: state.log_dir.display().to_string(),
    })
}

#[tauri::command]
async fn get_setting(
    state: tauri::State<'_, AppState>,
    key: String,
) -> Result<Option<String>, String> {
    let db = Arc::clone(&state.db);

    run_db(move || db.setting(&key)).await
}

#[tauri::command]
async fn set_setting(
    state: tauri::State<'_, AppState>,
    key: String,
    value: String,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);

    run_db(move || db.set_setting(&key, &value)).await
}

/// Zwraca bieżące ustawienia (PIN parowania i limity tekstów) dla ekranu ustawień.
#[tauri::command]
fn settings_snapshot(state: tauri::State<'_, AppState>) -> Result<AppSettings, String> {
    let settings = state
        .settings
        .lock()
        .map_err(|_| SETTINGS_UNAVAILABLE.to_string())?;

    Ok(settings.clone())
}

/// Zapisuje ustawienia: sprawdza poprawność, utrwala w bazie i podmienia stan w pamięci.
#[tauri::command]
async fn save_settings(
    state: tauri::State<'_, AppState>,
    settings: AppSettings,
) -> Result<(), String> {
    settings.validate().map_err(|error| error.to_string())?;

    let db = Arc::clone(&state.db);
    let to_persist = settings.clone();
    run_db(move || to_persist.save(&db)).await?;

    let mut current = state
        .settings
        .lock()
        .map_err(|_| SETTINGS_UNAVAILABLE.to_string())?;
    *current = settings.clone();

    info!(
        dedication_max_chars = settings.limits.dedication_max_chars,
        guest_name_max_chars = settings.limits.guest_name_max_chars,
        search_query_max_chars = settings.limits.search_query_max_chars,
        "ustawienia zapisane"
    );

    Ok(())
}

/// Uruchamia operację na bazie na wątku roboczym — dostęp do dysku nigdy nie idzie przez
/// wątek interfejsu (AGENTS.md, zasady architektury).
async fn run_db<T, E, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    E: std::error::Error + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(operation).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            error!(error = %error, "operacja na bazie danych nie powiodła się");

            Err(error.to_string())
        }
        Err(join_error) => {
            error!(error = %join_error, "wątek bazy danych zakończył się błędem");

            Err("operacja bazy danych nie powiodła się".to_string())
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle();

            let log_dir = handle.path().app_log_dir()?;
            let db_path = handle.path().app_data_dir()?.join("anon-dj.sqlite");

            // Strażnik logowania plikowego ma żyć dokładnie tyle, ile proces, więc celowo
            // go „wyciekamy” — zwolnienie nastąpi razem z zakończeniem programu.
            let guard = logging::init(&log_dir)?;
            Box::leak(Box::new(guard));

            info!(db = %db_path.display(), "otwieram bazę danych");
            let db = Db::open(&db_path)?;

            // Zepsute ustawienia nie mogą zablokować startu — startujemy z domyślnymi
            // i mówimy o tym w logu.
            let settings = match AppSettings::load(&db) {
                Ok(settings) => settings,
                Err(error) => {
                    warn!(error = %error, "nie udało się wczytać ustawień, używam domyślnych");

                    AppSettings::default()
                }
            };

            info!(
                dedication_max_chars = settings.limits.dedication_max_chars,
                guest_name_max_chars = settings.limits.guest_name_max_chars,
                search_query_max_chars = settings.limits.search_query_max_chars,
                "ustawienia wczytane"
            );

            app.manage(AppState::new(db, db_path, log_dir, settings));
            info!("aplikacja DJ-a gotowa");

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_status,
            get_setting,
            set_setting,
            settings_snapshot,
            save_settings
        ])
        .run(tauri::generate_context!())
        .expect("nie udało się uruchomić aplikacji ANON DJ");
}
