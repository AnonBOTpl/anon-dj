//! Aplikacja DJ-a ANON DJ: konfiguracja uruchomienia, stan współdzielony i komendy Tauri.

mod audio;
mod clips;
mod db;
mod library;
mod logging;
mod net;
// Silnik lektora: głosy Piper uruchamiane przez `sherpa-onnx`.
mod piper;
mod settings;
// Publiczny, bo kontrakt lektora (`TtsProvider`) jest zamierzony jako punkt wejścia dla silników
// TTS — dopóki nic go nie woła z wewnątrz, prywatny moduł zgłaszałby martwy kod.
pub mod tts;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use protocol::RequestStatus;
use tauri::{Emitter, Manager};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::db::{Db, DbError, LibraryFolder, QueuedRequest};
use crate::library::{LibraryError, ScanSummary};
use crate::settings::{AppSettings, validate_tts};
use crate::tts::{TtsProvider, VoiceTuning};

/// Ile prośb pokazujemy w kolejce przeglądu.
const QUEUE_LIMIT: u32 = 100;

/// Zdarzenie o zmianie stanu połączeń z kioskami.
const EVENT_KIOSK_STATUS: &str = "kiosk://status";
/// Zdarzenie o zmianie kolejki prośb.
const EVENT_REQUESTS_CHANGED: &str = "requests://changed";

/// Komunikat zwracany, gdy wcześniejszy błąd zostawił ustawienia w stanie zatrutym.
const SETTINGS_UNAVAILABLE: &str = "ustawienia są chwilowo niedostępne";

/// Komunikat zwracany, gdy nie ma czym przeczytać dedykacji.
const TTS_UNAVAILABLE: &str = "lektor jest niedostępny";

/// Komunikat zwracany, gdy DJ próbuje odsłuchać prośbę, której klip jeszcze się liczy.
const CLIP_NOT_READY: &str = "voice-over jeszcze się liczy";

/// Komunikat zwracany, gdy DJ prosi o podgląd głosu, którego nie ma na dysku.
const VOICE_UNKNOWN: &str = "nie ma takiego głosu lektora";

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
    /// Lektor dedykacji — albo gotowy, albo wyłączony z podanym powodem. Za blokadą, bo zapis
    /// ustawień przełącza go na inny głos w locie.
    tts: Mutex<piper::TtsRuntime>,
    /// Zlecenia generowania klipów lektora w tle wraz z postępem dla interfejsu.
    clips: Arc<clips::ClipJobs>,
    /// Odsłuch voice-overu na wybranym urządzeniu wyjściowym.
    audio: Arc<audio::VoiceOverPlayer>,
}

/// Stan lektora dla interfejsu DJ-a.
#[derive(Debug, Clone, serde::Serialize)]
struct TtsStatus {
    /// Czy da się przeczytać dedykację.
    available: bool,
    /// Głos, którym mówimy (a gdy lektora nie ma — `null`).
    selected: Option<String>,
    /// Dlaczego lektora nie ma — DJ ma wiedzieć, co poprawić.
    reason: Option<String>,
    /// Głosy znalezione na dysku.
    voices: Vec<piper::Voice>,
}

/// Domyślne ustawienia lektora — dla przycisku „Przywróć domyślne” na ekranie lektora.
///
/// Wartości bierzemy z warstwy Rust, a nie z interfejsu, żeby domyślne istniały dokładnie
/// w jednym miejscu: tam, gdzie stoi walidacja i od czego startuje świeża instalacja.
#[tauri::command]
fn tts_defaults() -> crate::settings::TtsSettings {
    crate::settings::TtsSettings::default()
}

/// Stan lektora: jakie głosy są zainstalowane i czy da się czytać dedykacje.
#[tauri::command]
fn tts_status(state: tauri::State<'_, AppState>) -> Result<TtsStatus, String> {
    let tts = state.tts.lock().map_err(|_| TTS_UNAVAILABLE.to_string())?;

    Ok(TtsStatus {
        available: tts.is_available(),
        selected: tts.selected().map(str::to_string),
        reason: tts.unavailable_reason().map(str::to_string),
        voices: tts.voices().to_vec(),
    })
}

/// Stan klipów lektora. Interfejs pyta o niego po wczytaniu, żeby po przeładowaniu okna
/// dalej wiedzieć, co się właśnie liczy (zdarzenia sprzed wczytania przepadają).
#[tauri::command]
fn clip_states(state: tauri::State<'_, AppState>) -> Vec<clips::ClipProgress> {
    state.clips.states()
}

/// Generuje (albo odświeża) klip lektora dla prośby. Zwraca od razu — postęp leci zdarzeniami
/// `clips://progress`.
#[tauri::command(rename_all = "snake_case")]
async fn generate_clip(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request_id: i64,
) -> Result<(), String> {
    start_clip_generation(&app, state.inner(), request_id).await
}

/// Zamawia klip lektora dla prośby, o ile mamy głos i znamy tekst.
///
/// Tekst czytamy z bazy w momencie zlecenia — dzięki temu lektor czyta to, co DJ widzi na
/// karcie, nawet gdy komenda dostała parametr chwilę wcześniej. Brak głosu nie jest błędem
/// aplikacji: dedykację można wtedy przeczytać z ekranu (PLAN.md, sekcja 8).
async fn start_clip_generation(
    app: &tauri::AppHandle,
    state: &AppState,
    request_id: i64,
) -> Result<(), String> {
    let provider = state
        .tts
        .lock()
        .map_err(|_| TTS_UNAVAILABLE.to_string())?
        .provider()
        .ok_or_else(|| TTS_UNAVAILABLE.to_string())?;

    let tuning = {
        let settings = state
            .settings
            .lock()
            .map_err(|_| SETTINGS_UNAVAILABLE.to_string())?;

        settings.tts.tuning
    };

    let db = Arc::clone(&state.db);
    let text = run_db({
        let db = Arc::clone(&db);

        move || db.dedication(request_id)
    })
    .await?
    .ok_or_else(|| format!("prośba {request_id} nie istnieje"))?;

    state
        .clips
        .start(app.clone(), db, provider, tuning, request_id, text);

    Ok(())
}

/// Urządzenia wyjściowe, na których DJ może odsłuchać voice-over.
///
/// Wypisywanie urządzeń sięga do systemu, więc idzie poza wątek interfejsu — okno nie ma prawa
/// zamarznąć w trakcie imprezy, nawet na chwilę.
#[tauri::command]
async fn audio_devices() -> Result<Vec<audio::OutputDevice>, String> {
    tauri::async_runtime::spawn_blocking(audio::output_devices)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

/// Odsłuchuje gotowy voice-over prośby na urządzeniu wyjściowym z ustawień.
///
/// To przycisk „Odsłuchaj” z PLAN.md (sekcja 8): DJ sprawdza dedykację i brzmienie głosu, zanim
/// puści ją na antenę. Klipu nie liczymy na nowo — bierzemy gotowy plik i tylko go odtwarzamy.
#[tauri::command(rename_all = "snake_case")]
async fn play_dedication(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request_id: i64,
) -> Result<(), String> {
    let path = {
        let db = Arc::clone(&state.db);

        run_db(move || db.tts_clip_path(request_id)).await?
    }
    .ok_or_else(|| CLIP_NOT_READY.to_string())?;

    let (device, volume) = {
        let settings = state
            .settings
            .lock()
            .map_err(|_| SETTINGS_UNAVAILABLE.to_string())?;

        (
            settings.audio.output_device_id.clone(),
            audio::playback_volume(settings.tts.tuning.volume_percent),
        )
    };

    // Otwarcie strumienia audio to operacja systemowa — też poza wątkiem interfejsu.
    let player = Arc::clone(&state.audio);
    let handle = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        player.play(
            &handle,
            audio::PlaybackTarget::Request(request_id),
            Path::new(&path),
            &device,
            volume,
        )
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

/// Wynik podglądu głosu lektora.
#[derive(Debug, Clone, Copy, serde::Serialize)]
struct VoicePreview {
    /// Ile trwała synteza próbki — po tym DJ ocenia, czy klip zdąży przed emisją.
    synthesis_ms: u64,
    /// Długość nagrania w milisekundach.
    duration_ms: u32,
}

/// Odsłuchuje próbkę głosu z podanym strojeniem i od razu ją puszcza.
///
/// To ekran porównywania głosów (PLAN.md, sekcja 8): DJ zmienia głos i suwaki, słucha, a wybór
/// zapisuje dopiero wtedy, gdy mu się spodoba. Podgląd bierze więc głos i strojenie z formularza,
/// a nie z zapisanych ustawień, i niczego nie utrwala.
#[tauri::command(rename_all = "snake_case")]
async fn preview_voice(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    voice: String,
    tuning: VoiceTuning,
    text: String,
) -> Result<VoicePreview, String> {
    let (device, max_chars) = {
        let settings = state
            .settings
            .lock()
            .map_err(|_| SETTINGS_UNAVAILABLE.to_string())?;

        (
            settings.audio.output_device_id.clone(),
            settings.limits.dedication_max_chars as usize,
        )
    };

    // Próbkę czytamy tym samym lektorem co dedykacje, więc obowiązuje ją ten sam limit i ta sama
    // walidacja głosu oraz strojenia.
    let text =
        protocol::validate_dedication_with(&text, max_chars).map_err(|error| error.to_string())?;
    validate_tts(&voice, &tuning).map_err(|error| error.to_string())?;

    let provider = state
        .tts
        .lock()
        .map_err(|_| TTS_UNAVAILABLE.to_string())?
        .provider_for(&voice)
        .ok_or_else(|| format!("{VOICE_UNKNOWN}: {voice}"))?;

    let volume = audio::playback_volume(tuning.volume_percent);
    let player = Arc::clone(&state.audio);
    let handle = app.clone();

    // Synteza i otwarcie strumienia to praca dla wątku roboczego — nawet podgląd nie może
    // zamrozić okna.
    tauri::async_runtime::spawn_blocking(move || {
        let started = std::time::Instant::now();

        let clip = provider
            .synthesize(&text, &tuning)
            .map_err(|error| error.to_string())?;
        let synthesis_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

        player
            .play(
                &handle,
                audio::PlaybackTarget::Preview,
                &clip.path,
                &device,
                volume,
            )
            .map_err(|error| error.to_string())?;

        Ok(VoicePreview {
            synthesis_ms,
            duration_ms: clip.duration_ms,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Zatrzymuje odsłuch voice-overu.
#[tauri::command]
async fn stop_dedication(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let player = Arc::clone(&state.audio);
    let handle = app.clone();

    tauri::async_runtime::spawn_blocking(move || player.stop(&handle))
        .await
        .map_err(|error| error.to_string())
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

/// Zatwierdzone prośby w kolejności, w jakiej pójdą na antenę.
#[tauri::command]
async fn ready_requests(state: tauri::State<'_, AppState>) -> Result<Vec<QueuedRequest>, String> {
    let db = Arc::clone(&state.db);

    run_db(move || db.ready_requests(QUEUE_LIMIT)).await
}

/// Historia: wykonane i odrzucone dedykacje, ostatnio zmienione pierwsze.
#[tauri::command]
async fn request_history(state: tauri::State<'_, AppState>) -> Result<Vec<QueuedRequest>, String> {
    let db = Arc::clone(&state.db);

    run_db(move || db.request_history(QUEUE_LIMIT)).await
}

/// Zatwierdza dedykację: prośba przechodzi z kolejki przeglądu do kolejki gotowych.
///
/// Zatwierdzenie od razu zamawia klip lektora — „Wykonaj” ma potem tylko odtworzyć gotowy plik
/// (PLAN.md, sekcja 8). Brak głosu nie przewraca zatwierdzenia.
#[tauri::command(rename_all = "snake_case")]
async fn approve_request(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request_id: i64,
) -> Result<(), String> {
    run_db({
        let db = Arc::clone(&state.db);

        move || db.approve_request(request_id)
    })
    .await?;

    notify_kiosk(&state, request_id, RequestStatus::Approved).await;
    state.server.notify_queue_changed();

    info!(request_id, "prośba zatwierdzona przez DJ-a");

    if let Err(error) = start_clip_generation(&app, state.inner(), request_id).await {
        warn!(request_id, error = %error, "nie udało się zamówić klipu lektora");
    }

    Ok(())
}

/// Odrzuca dedykację — z kolejki przeglądu albo z gotowych.
#[tauri::command(rename_all = "snake_case")]
async fn reject_request(state: tauri::State<'_, AppState>, request_id: i64) -> Result<(), String> {
    run_db({
        let db = Arc::clone(&state.db);

        move || db.reject_request(request_id)
    })
    .await?;

    notify_kiosk(&state, request_id, RequestStatus::Rejected).await;
    state.server.notify_queue_changed();

    // Odrzucona prośba nie ma już voice-overu — nie trzymamy ani jej stanu, ani liczonego klipu.
    state.clips.forget(request_id);

    info!(request_id, "prośba odrzucona przez DJ-a");

    Ok(())
}

/// Zapisuje poprawioną dedykację.
///
/// Limit długości bierzemy z ustawień DJ-a — ten sam, który obowiązuje gościa przy wpisywaniu.
#[tauri::command(rename_all = "snake_case")]
async fn update_dedication(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request_id: i64,
    dedication: String,
) -> Result<(), String> {
    let max_chars = {
        let settings = state
            .settings
            .lock()
            .map_err(|_| SETTINGS_UNAVAILABLE.to_string())?;

        settings.limits.dedication_max_chars as usize
    };

    let dedication = protocol::validate_dedication_with(&dedication, max_chars)
        .map_err(|error| error.to_string())?;

    run_db({
        let db = Arc::clone(&state.db);

        move || -> Result<(), DbError> {
            db.set_dedication(request_id, &dedication)?;

            // Stary klip czytał stary tekst. Nie może zostać ani chwili dłużej — inaczej DJ
            // mógłby wykonać dedykację przed policzeniem nowego i puścić nieaktualny voice-over.
            db.set_tts_clip_path(request_id, None)
        }
    })
    .await?;

    state.server.notify_queue_changed();

    info!(request_id, "dedykacja poprawiona przez DJ-a");

    // Poprawiony tekst to nowy voice-over. Zlecenie starsze od tej poprawki straci aktualność
    // i nie zapisze klipu z tekstem, którego DJ już nie widzi.
    if let Err(error) = start_clip_generation(&app, state.inner(), request_id).await {
        warn!(request_id, error = %error, "nie udało się zamówić klipu po poprawce");
    }

    Ok(())
}

/// Przesuwa prośbę w kolejce gotowych o jedno miejsce. `direction` to `up` albo `down`.
#[tauri::command(rename_all = "snake_case")]
async fn move_request(
    state: tauri::State<'_, AppState>,
    request_id: i64,
    direction: String,
) -> Result<(), String> {
    let up = match direction.as_str() {
        "up" => true,
        "down" => false,
        other => return Err(format!("nieznany kierunek przesunięcia kolejki: {other}")),
    };

    let moved = run_db({
        let db = Arc::clone(&state.db);

        move || db.move_request(request_id, up)
    })
    .await?;

    if moved {
        state.server.notify_queue_changed();
    }

    Ok(())
}

/// Mówi kioskowi, który przysłał prośbę, co DJ z nią zrobił.
///
/// Brak połączenia z kioskiem nie jest błędem — gość zdążył już odejść, a informacja zostaje
/// w aplikacji DJ-a.
async fn notify_kiosk(state: &tauri::State<'_, AppState>, request_id: i64, status: RequestStatus) {
    let db = Arc::clone(&state.db);

    match run_db(move || db.request_target(request_id)).await {
        Ok(Some(target)) => state.server.notify_request_status(target, status),
        Ok(None) => warn!(
            request_id,
            "nie ma prośby, której status mielibyśmy przekazać"
        ),
        Err(error) => warn!(request_id, error = %error, "nie udało się ustalić adresu prośby"),
    }
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

    // Głos lektora działa od razu. DJ wybiera go przed imprezą i nie może się dowiedzieć, że nowy
    // wybór wejdzie w życie dopiero po restarcie aplikacji.
    let switched = state
        .tts
        .lock()
        .map_err(|_| TTS_UNAVAILABLE.to_string())?
        .select(&settings.tts.voice);

    if switched {
        info!(voice = %settings.tts.voice, "lektor przełączony na wybrany głos");
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
            let app_data_dir = handle.path().app_data_dir()?;
            let db_path = app_data_dir.join("anon-dj.sqlite");

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

            // Lektor przygotowujemy z gotowych plików; brak głosów nie może zatrzymać startu —
            // wtedy aplikacja działa bez voice-overu, a DJ czyta dedykację z ekranu.
            let voices_dir = piper::default_voices_dir(&app_data_dir);
            let cache = crate::tts::ClipCache::new(app_data_dir.join("tts-cache"));
            let tts = match piper::discover_voices(&voices_dir) {
                Ok(voices) => {
                    let preferred = settings
                        .lock()
                        .map(|settings| settings.tts.voice.clone())
                        .unwrap_or_default();

                    piper::TtsRuntime::prepare(voices, &preferred, cache)
                }
                Err(error) => {
                    warn!(error = %error, "nie udało się wypisać głosów lektora, wyłączam lektora");

                    piper::TtsRuntime::unavailable(error.to_string(), cache)
                }
            };

            match (tts.selected(), tts.unavailable_reason()) {
                (Some(voice), _) => {
                    info!(voice, voices_dir = %voices_dir.display(), "lektor gotowy")
                }
                (None, reason) => warn!(
                    voices_dir = %voices_dir.display(),
                    reason = reason.unwrap_or("nieznany powód"),
                    "lektor niedostępny — dedykacje trzeba przeczytać z ekranu"
                ),
            };

            let clips = Arc::new(clips::ClipJobs::new());
            let audio = Arc::new(audio::VoiceOverPlayer::new());

            app.manage(AppState {
                db,
                db_path,
                log_dir,
                settings,
                scanning: Arc::new(AtomicBool::new(false)),
                server,
                tts: Mutex::new(tts),
                clips,
                audio,
            });
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
            pending_requests,
            ready_requests,
            request_history,
            approve_request,
            reject_request,
            update_dedication,
            move_request,
            tts_status,
            tts_defaults,
            clip_states,
            generate_clip,
            audio_devices,
            play_dedication,
            stop_dedication,
            preview_voice
        ])
        .run(tauri::generate_context!())
        .expect("nie udało się uruchomić aplikacji ANON DJ");
}
