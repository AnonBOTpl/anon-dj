//! Aplikacja kiosku ANON DJ: gość wyszukuje utwór, wybiera go i wpisuje dedykację.
//!
//! Kiosk **nie zawiera** skanera biblioteki, TTS ani kodu odtwarzacza (PLAN.md, sekcja 4.2).
//! Nie prowadzi też własnej bazy — trwałe dane trzyma wyłącznie aplikacja DJ-a. Kiosk może
//! wyłącznie pytać o utwory i składać prośby; nie ma komunikatu, którym sterowałby DJ-em.

mod net;

use std::sync::Arc;

use protocol::{ErrorCode, RequestStatus, TrackInfo};
use tauri::{Emitter, Manager};
use tokio::sync::mpsc;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::net::{Client, ConnectionState, ServerConfig, UiEvent};

/// Zdarzenie o zmianie stanu połączenia z aplikacją DJ-a.
const EVENT_CONNECTION: &str = "kiosk://connection";
/// Zdarzenie z wynikami wyszukiwania.
const EVENT_SEARCH_RESULTS: &str = "kiosk://search-results";
/// Zdarzenie o statusie wysłanej prośby.
const EVENT_REQUEST_STATUS: &str = "kiosk://request-status";
/// Zdarzenie z błędem zwróconym przez aplikację DJ-a.
const EVENT_ERROR: &str = "kiosk://error";

/// Informacje o kiosku. Na ekranie gościa nie pokazujemy nic z tego.
#[derive(Debug, serde::Serialize)]
struct AppStatus {
    version: &'static str,
    protocol_version: u16,
}

/// Wyniki wyszukiwania przekazywane do interfejsu.
#[derive(Debug, Clone, serde::Serialize)]
struct SearchResultsPayload {
    request_id: u64,
    tracks: Vec<TrackInfo>,
}

/// Status prośby przekazywany do interfejsu.
#[derive(Debug, Clone, serde::Serialize)]
struct RequestStatusPayload {
    request_id: u64,
    status: RequestStatus,
}

/// Błąd z aplikacji DJ-a przekazywany do interfejsu.
#[derive(Debug, Clone, serde::Serialize)]
struct ErrorPayload {
    request_id: Option<u64>,
    code: ErrorCode,
}

#[tauri::command]
fn app_status() -> AppStatus {
    AppStatus {
        version: env!("CARGO_PKG_VERSION"),
        protocol_version: protocol::PROTOCOL_VERSION,
    }
}

/// Bieżący stan połączenia — interfejs pyta o niego po starcie.
#[tauri::command]
fn connection_state(client: tauri::State<'_, Arc<Client>>) -> ConnectionState {
    client.state()
}

/// Łączy kiosk z aplikacją DJ-a (i ponawia próby, dopóki się nie uda).
#[tauri::command(rename_all = "snake_case")]
fn connect(client: tauri::State<'_, Arc<Client>>, config: ServerConfig) -> Result<(), String> {
    client.connect(config).map_err(|error| error.to_string())
}

/// Rozłącza kiosk i pokazuje ekran konfiguracji.
#[tauri::command]
fn disconnect(client: tauri::State<'_, Arc<Client>>) {
    client.disconnect();
}

/// Wysyła zapytanie o utwory. Wyniki przyjdą zdarzeniem `kiosk://search-results`.
#[tauri::command(rename_all = "snake_case")]
fn search(client: tauri::State<'_, Arc<Client>>, query: String) -> Result<u64, String> {
    client.search(query).map_err(|error| error.to_string())
}

/// Wysyła prośbę gościa. Odpowiedź przyjdzie zdarzeniem `kiosk://request-status`.
#[tauri::command(rename_all = "snake_case")]
fn submit_request(
    client: tauri::State<'_, Arc<Client>>,
    track_id: i64,
    dedication: String,
    guest_name: Option<String>,
) -> Result<u64, String> {
    client
        .submit_request(track_id, dedication, guest_name)
        .map_err(|error| error.to_string())
}

/// Przerzuca zdarzenia sieciowe do interfejsu kiosku.
fn spawn_ui_forwarder(app: tauri::AppHandle, mut events: mpsc::UnboundedReceiver<UiEvent>) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                UiEvent::Connection(state) => emit(&app, EVENT_CONNECTION, state),
                UiEvent::SearchResults { request_id, tracks } => emit(
                    &app,
                    EVENT_SEARCH_RESULTS,
                    SearchResultsPayload { request_id, tracks },
                ),
                UiEvent::RequestReceived { request_id, status } => emit(
                    &app,
                    EVENT_REQUEST_STATUS,
                    RequestStatusPayload { request_id, status },
                ),
                UiEvent::Error { request_id, code } => {
                    emit(&app, EVENT_ERROR, ErrorPayload { request_id, code })
                }
            }
        }
    });
}

fn emit<T>(app: &tauri::AppHandle, event: &str, payload: T)
where
    T: serde::Serialize + Clone,
{
    if let Err(error) = app.emit(event, payload) {
        warn!(event, error = %error, "nie udało się wysłać zdarzenia do interfejsu");
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
        .setup(|app| {
            init_logging();

            let (events, receiver) = mpsc::unbounded_channel();
            let client = Client::new(events);

            spawn_ui_forwarder(app.handle().clone(), receiver);
            app.manage(client);

            info!("kiosk ANON DJ gotowy");

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_status,
            connection_state,
            connect,
            disconnect,
            search,
            submit_request
        ])
        .run(tauri::generate_context!())
        .expect("nie udało się uruchomić kiosku ANON DJ");
}
