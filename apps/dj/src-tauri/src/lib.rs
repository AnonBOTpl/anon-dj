//! Aplikacja DJ-a ANON DJ: konfiguracja uruchomienia, stan współdzielony i komendy Tauri.

mod db;
mod library;
mod logging;
mod net;
mod settings;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use protocol::RequestStatus;
use tauri::{Emitter, Manager};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::db::{Db, DbError, LibraryFolder, QueuedRequest};
use crate::library::{LibraryError, ScanSummary};
use crate::settings::AppSettings;

/// Ile prośb pokazujemy w kolejce przeglądu.
const QUEUE_LIMIT: u32 = 100;

/// Zdarzenie o zmianie stanu połączeń z kioskami.
const EVENT_KIOSK_STATUS: &str = "kiosk://status";
/// Zdarzenie o zmianie kolejki prośb.
const EVENT_REQUESTS_CHANGED: &str = "requests://changed";

/// Komunikat zwracany, gdy wcześniejszy błąd zostawił ustawienia w stanie zatrutym.
const SETTINGS_UNAVAILABLE: &str = "ustawienia są chwilowo niedostępne";

/// Stan aplikacji współdzielony między komendami Tauri.
struct AppState {
    db: Arc<Db>,
    db_path: PathBuf,
    log_dir: PathBuf,
    /// Efektywne ustawienia trzymane w pamięci, żeby walidacja nie sięgała do bazy przy każdym
    /// komunikacie. Współdzielone z serwerem kiosków — ten czyta PIN i limity na bieżąco,
    /// więc zmiana w ustawieniach działa bez restartu aplikacji.
    settings: Arc<Mutex<AppSettings>>,
    /// Blokada przed równoległym skanowaniem: dwa kliknięcia „Skanuj” nie mogą puścić
    /// dwóch przebiegów po tym samym dysku.
    scanning: Arc<AtomicBool>,
    /// Serwer LAN dla kiosków.
    server: Arc<net::Server>,
}

impl AppState {
    fn new(
        db: Arc<Db>,
        db_path: PathBuf,
        log_dir: PathBuf,
        settings: Arc<Mutex<AppSettings>>,
        server: Arc<net::Server>,
    ) -> Self {
        Self {
            db,
            db_path,
            log_dir,
            settings,
            scanning: Arc::new(AtomicBool::new(false)),
            server,
        }
    }
}

/// Stan połączeń z kioskami pokazywany w pasku statusu DJ-a.
#[derive(Debug, Clone, serde::Serialize)]
struct ServerStatus {
    port: u16,
    /// Adres tego komputera w sieci lokalnej, jeśli dało się go ustalić.
    address: Option<String>,
    /// Czy nasłuch działa (zajęty port to najczęstsza przyczyna „kiosk się nie łączy”).
    listening: bool,
    connected: u32,
    /// Nazwy podłączonych kiosków.
    kiosks: Vec<String>,
}

/// Stan połączeń na teraz — dla paska statusu i ekranu ustawień.
#[tauri::command]
fn server_status(state: tauri::State<'_, AppState>) -> Result<ServerStatus, String> {
    let port = state
        .settings
        .lock()
        .map_err(|_| SETTINGS_UNAVAILABLE.to_string())?
        .port;

    Ok(server_status_snapshot(&state.server, port))
}

/// Prośby czekające na decyzję DJ-a, najstarsze pierwsze.
#[tauri::command]
async fn pending_requests(state: tauri::State<'_, AppState>) -> Result<Vec<QueuedRequest>, String> {
    let db = Arc::clone(&state.db);

    run_db(move || db.requests_with_status(RequestStatus::Submitted, QUEUE_LIMIT)).await
}

/// Składa stan połączeń z aktualnej listy kiosków.
fn server_status_snapshot(server: &net::Server, port: u16) -> ServerStatus {
    let kiosks = server.connected_kiosks();

    ServerStatus {
        port,
        address: net::local_ip().map(|address| address.to_string()),
        listening: server.is_listening(),
        connected: u32::try_from(kiosks.len()).unwrap_or(u32::MAX),
        kiosks,
    }
}

/// Przerzuca zdarzenia serwera kiosków do interfejsu DJ-a.
fn spawn_ui_forwarder(
    app: tauri::AppHandle,
    server: Arc<net::Server>,
    mut events: mpsc::UnboundedReceiver<net::UiEvent>,
) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                net::UiEvent::KiosksChanged => {
                    let port = match app.state::<AppState>().settings.lock() {
                        Ok(settings) => settings.port,
                        Err(_) => continue,
                    };

                    let status = server_status_snapshot(&server, port);

                    if let Err(error) = app.emit(EVENT_KIOSK_STATUS, status) {
                        warn!(error = %error, "nie udało się wysłać stanu kiosków");
                    }
                }
                net::UiEvent::RequestsChanged => {
                    if let Err(error) = app.emit(EVENT_REQUESTS_CHANGED, ()) {
                        warn!(error = %error, "nie udało się powiadomić o nowej prośbie");
                    }
                }
            }
        }
    });
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

    // Blokadę zdejmujemy przed wołaniem serwera — nasłuch nie może czekać na ustawienia.
    let port_changed = {
        let mut current = state
            .settings
            .lock()
            .map_err(|_| SETTINGS_UNAVAILABLE.to_string())?;
        let port_changed = current.port != settings.port;

        *current = settings.clone();

        port_changed
    };

    if port_changed {
        info!(port = settings.port, "zmiana portu serwera kiosków");

        state.server.set_port(settings.port);
    }

    info!(
        dedication_max_chars = settings.limits.dedication_max_chars,
        guest_name_max_chars = settings.limits.guest_name_max_chars,
        search_query_max_chars = settings.limits.search_query_max_chars,
        "ustawienia zapisane"
    );

    Ok(())
}

/// Zdarzenie z postępem skanowania biblioteki; nasłuchuje widok biblioteki.
const EVENT_SCAN_PROGRESS: &str = "library://scan-progress";

/// Postęp skanowania wysyłany do interfejsu.
#[derive(Debug, Clone, serde::Serialize)]
struct ScanProgress {
    /// Folder, którego dotyczy postęp.
    folder: String,
    /// Liczba folderów już przeprocesowanych.
    folders_done: u32,
    folders_total: u32,
    files_seen: u64,
    indexed: u64,
    skipped: u64,
    /// `true` po zakończeniu całego skanowania.
    done: bool,
}

/// Liczby widoczne w widoku biblioteki.
#[derive(Debug, Clone, serde::Serialize)]
struct LibraryStats {
    folders: u32,
    tracks: u64,
    scanning: bool,
}

/// Lista folderów wskazanych do skanowania.
#[tauri::command]
async fn library_folders(state: tauri::State<'_, AppState>) -> Result<Vec<LibraryFolder>, String> {
    let db = Arc::clone(&state.db);

    run_db(move || db.library_folders()).await
}

/// Dodaje folder do biblioteki i zwraca aktualną listę.
///
/// Ścieżka przychodzi z interfejsu, więc traktujemy ją jak dane z zewnątrz: sprawdzamy, czy
/// faktycznie jest folderem, i ujednolicamy jej postać.
#[tauri::command]
async fn add_library_folder(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<Vec<LibraryFolder>, String> {
    let folder = library::normalize_folder_path(&path);

    if !Path::new(&folder).is_dir() {
        return Err(LibraryError::NotAFolder {
            path: PathBuf::from(&folder),
        }
        .to_string());
    }

    let db = Arc::clone(&state.db);
    let added = run_db({
        let db = Arc::clone(&db);
        let folder = folder.clone();

        move || db.add_library_folder(&folder)
    })
    .await?;

    if added {
        info!(path = %folder, "folder dodany do biblioteki");
    }

    run_db(move || db.library_folders()).await
}

/// Usuwa folder z biblioteki razem z jego utworami.
#[tauri::command]
async fn remove_library_folder(
    state: tauri::State<'_, AppState>,
    id: i64,
) -> Result<Vec<LibraryFolder>, String> {
    let db = Arc::clone(&state.db);

    let removed = run_db({
        let db = Arc::clone(&db);

        move || -> Result<Option<(String, u64)>, DbError> {
            let Some(path) = db.remove_library_folder(id)? else {
                return Ok(None);
            };

            let deleted = db.delete_tracks_under(&path)?;

            Ok(Some((path, deleted)))
        }
    })
    .await?;

    if let Some((path, deleted)) = removed {
        info!(path = %path, tracks = deleted, "folder usunięty z biblioteki");
    }

    run_db(move || db.library_folders()).await
}

/// Liczby widoczne w widoku biblioteki.
#[tauri::command]
async fn library_stats(state: tauri::State<'_, AppState>) -> Result<LibraryStats, String> {
    let scanning = state.scanning.load(Ordering::SeqCst);
    let db = Arc::clone(&state.db);

    let folders = run_db({
        let db = Arc::clone(&db);

        move || db.library_folders()
    })
    .await?
    .len();

    let tracks = run_db(move || db.count_tracks()).await?;

    Ok(LibraryStats {
        folders: u32::try_from(folders).unwrap_or(u32::MAX),
        tracks,
        scanning,
    })
}

/// Uruchamia skanowanie wszystkich folderów biblioteki.
///
/// Skanowanie idzie w wątku roboczym, a postęp leci zdarzeniami — komenda wraca od razu,
/// żeby interfejs nie czekał zablokowany na kilka minut.
#[tauri::command]
async fn start_library_scan(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    let scanning = Arc::clone(&state.scanning);

    if scanning
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(LibraryError::AlreadyScanning.to_string());
    }

    let folders = match run_db({
        let db = Arc::clone(&db);

        move || db.library_folders()
    })
    .await
    {
        Ok(folders) => folders,
        Err(error) => {
            scanning.store(false, Ordering::SeqCst);

            return Err(error);
        }
    };

    if folders.is_empty() {
        scanning.store(false, Ordering::SeqCst);

        return Err(LibraryError::NoFolders.to_string());
    }

    let folders_total = u32::try_from(folders.len()).unwrap_or(u32::MAX);
    info!(folders = folders_total, "start skanowania biblioteki");

    tauri::async_runtime::spawn_blocking(move || {
        let mut summary = ScanSummary::default();

        for (index, folder) in folders.iter().enumerate() {
            let folders_done = u32::try_from(index).unwrap_or(0);
            let path = PathBuf::from(&folder.path);

            let folder_summary = if path.is_dir() {
                match library::scan_folder(&db, &path, |progress| {
                    emit_scan_progress(
                        &app,
                        &ScanProgress {
                            folder: folder.path.clone(),
                            folders_done,
                            folders_total,
                            files_seen: progress.files_seen,
                            indexed: progress.indexed,
                            skipped: progress.skipped,
                            done: false,
                        },
                    );
                }) {
                    Ok(value) => value,
                    Err(error) => {
                        warn!(path = %folder.path, error = %error, "skanowanie folderu nie powiodło się");

                        ScanSummary::default()
                    }
                }
            } else {
                warn!(path = %folder.path, "folder biblioteki nie istnieje, pomijam");

                ScanSummary::default()
            };

            summary.merge(&folder_summary);

            let finished = folders_done + 1;
            emit_scan_progress(
                &app,
                &ScanProgress {
                    folder: folder.path.clone(),
                    folders_done: finished,
                    folders_total,
                    files_seen: folder_summary.files_seen,
                    indexed: folder_summary.indexed,
                    skipped: folder_summary.skipped,
                    done: finished >= folders_total,
                },
            );
        }

        scanning.store(false, Ordering::SeqCst);
        info!(
            files = summary.files_seen,
            indexed = summary.indexed,
            skipped = summary.skipped,
            "skanowanie biblioteki zakończone"
        );
    });

    Ok(())
}

/// Wyszukiwanie w bibliotece po tytule i wykonawcy. Puste zapytanie pokazuje początek biblioteki.
#[tauri::command]
async fn search_tracks(
    state: tauri::State<'_, AppState>,
    query: String,
) -> Result<Vec<protocol::TrackInfo>, String> {
    // Zapytanie przychodzi z interfejsu, więc przycinamy jego długość zanim trafi do bazy.
    let query = query
        .chars()
        .take(protocol::MAX_SEARCH_QUERY_CHARS)
        .collect::<String>();
    let db = Arc::clone(&state.db);

    run_db(move || db.search_tracks(&query, library::MAX_SEARCH_RESULTS)).await
}

/// Wysyła postęp skanowania do interfejsu.
fn emit_scan_progress(app: &tauri::AppHandle, progress: &ScanProgress) {
    if let Err(error) = app.emit(EVENT_SCAN_PROGRESS, progress) {
        warn!(error = %error, "nie udało się wysłać postępu skanowania");
    }
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
        // Wybór folderu z muzyką robimy natywnym oknem systemowym — DJ nie powinien
        // przepisywać ścieżek ręcznie.
        .plugin(tauri_plugin_dialog::init())
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
                port = settings.port,
                "ustawienia wczytane"
            );

            // Serwer kiosków dostaje te same ustawienia co interfejs, żeby zmiana PIN-u
            // albo limitów działała od razu, bez restartu aplikacji.
            let port = settings.port;
            let settings = Arc::new(Mutex::new(settings));
            let (ui_events, ui_rx) = mpsc::unbounded_channel();
            let db = Arc::new(db);
            let server = net::Server::new(Arc::clone(&db), Arc::clone(&settings), ui_events, port);

            server.spawn();
            spawn_ui_forwarder(app.handle().clone(), Arc::clone(&server), ui_rx);

            app.manage(AppState::new(db, db_path, log_dir, settings, server));
            info!("aplikacja DJ-a gotowa");

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_status,
            get_setting,
            set_setting,
            settings_snapshot,
            save_settings,
            library_folders,
            add_library_folder,
            remove_library_folder,
            library_stats,
            start_library_scan,
            search_tracks,
            server_status,
            pending_requests
        ])
        .run(tauri::generate_context!())
        .expect("nie udało się uruchomić aplikacji ANON DJ");
}
