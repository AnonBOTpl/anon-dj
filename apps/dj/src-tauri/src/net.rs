//! Serwer LAN dla kiosków: parowanie PIN-em, wyszukiwanie w bibliotece i przyjmowanie prośb
//! gości (PLAN.md, sekcje 5, 6 i 12).
//!
//! Zasady, których pilnuje ten moduł:
//!
//! - kiosk musi się najpierw sparować PIN-em z ustawień DJ-a, zanim cokolwiek wyśle,
//! - każdy komunikat przechodzi walidację (`protocol`) i limit tempa,
//! - **nic nie trafia na antenę** — serwer tylko zapisuje prośbę w kolejce przeglądu; decyzję
//!   podejmuje DJ w interfejsie,
//! - baza chodzi na wątku roboczym, bo zapytanie SQLite nie może blokować wątku asynchronicznego.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use protocol::{
    DjMessage, ErrorCode, KioskMessage, Limits, MAX_KIOSK_NAME_CHARS, MAX_SEARCH_RESULTS,
    PROTOCOL_VERSION, RequestStatus, count_chars, normalize_text, validate_dedication_with,
    validate_guest_name_with,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::error::Error as WsError;
use tracing::{debug, info, warn};

use crate::db::{Db, DbError, now_ms};
use crate::settings::AppSettings;

/// Nazwa serwera pokazywana kioskowi przy parowaniu.
const SERVER_NAME: &str = "ANON DJ";

/// Ile czasu kiosk ma na przysłanie `hello` po otwarciu połączenia. Milczący klient zajmuje
/// gniazdo, więc go zamykamy.
const PAIRING_TIMEOUT: Duration = Duration::from_secs(10);

/// Limit tempa na jeden kiosk: tyle komunikatów w oknie [`RATE_WINDOW`].
const RATE_MAX_MESSAGES: usize = 30;
/// Długość okna limitera tempa.
const RATE_WINDOW: Duration = Duration::from_secs(10);

/// Przez ile czasu identyczna prośba z tego samego kiosku jest uznawana za duplikat
/// (ochrona przed podwójnym kliknięciem „Wyślij”).
const DUPLICATE_WINDOW: Duration = Duration::from_secs(30);

/// Przerwa przed ponowną próbą, gdy nasłuch nie mógł wystartować (np. port zajęty).
const RESTART_DELAY: Duration = Duration::from_secs(5);

/// Zdarzenie dla interfejsu DJ-a.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiEvent {
    /// Zmienił się zestaw podłączonych kiosków.
    KiosksChanged,
    /// Do kolejki przyszła prośba — interfejs odświeża listę.
    RequestsChanged,
}

/// Nasłuchujący serwer kiosków. Jeden na aplikację, żyje od startu do zamknięcia.
pub struct Server {
    db: Arc<Db>,
    settings: Arc<Mutex<AppSettings>>,
    ui_events: mpsc::UnboundedSender<UiEvent>,
    state: Mutex<State>,
    /// Port, na którym serwer ma nasłuchiwać. Zmiana wartości restartuje nasłuch.
    desired_port: watch::Sender<u16>,
}

/// Stan połączeń pilnowany pod mutexem. Sekcje krytyczne są krótkie i bez `await`.
#[derive(Default)]
struct State {
    /// Podłączone kioski: identyfikator → nazwa.
    kiosks: HashMap<i64, String>,
    limiter: RateLimiter,
    /// Czy nasłuch faktycznie działa. Zajęty port to najczęstsza przyczyna „kiosk się nie łączy”,
    /// więc DJ musi to widzieć w interfejsie, a nie tylko w logu.
    listening: bool,
}

impl Server {
    /// Tworzy serwer na podanym porcie (jeszcze nie nasłuchuje).
    pub fn new(
        db: Arc<Db>,
        settings: Arc<Mutex<AppSettings>>,
        ui_events: mpsc::UnboundedSender<UiEvent>,
        port: u16,
    ) -> Arc<Self> {
        let (desired_port, _) = watch::channel(port);

        Arc::new(Self {
            db,
            settings,
            ui_events,
            state: Mutex::new(State::default()),
            desired_port,
        })
    }

    /// Ustawia port, na którym serwer ma nasłuchiwać. Nadzorca sam podniesie nasłuch od nowa.
    pub fn set_port(&self, port: u16) {
        self.desired_port.send_replace(port);
    }

    /// Czy serwer faktycznie nasłuchuje (port mógł być zajęty).
    pub fn is_listening(&self) -> bool {
        match self.state.lock() {
            Ok(state) => state.listening,
            Err(_) => false,
        }
    }

    /// Nazwy podłączonych kiosków — do paska statusu DJ-a.
    pub fn connected_kiosks(&self) -> Vec<String> {
        match self.state.lock() {
            Ok(state) => {
                let mut names: Vec<String> = state.kiosks.values().cloned().collect();
                names.sort();

                names
            }
            Err(_) => Vec::new(),
        }
    }

    /// Startuje nadzorcę nasłuchu. Wołane raz, przy starcie aplikacji.
    pub fn spawn(self: &Arc<Self>) {
        let server = Arc::clone(self);

        tauri::async_runtime::spawn(async move {
            server.supervise().await;
        });
    }

    /// Nasłuchuje i restartuje się po zmianie portu oraz po nieudanym starcie.
    async fn supervise(self: Arc<Self>) {
        let mut port_rx = self.desired_port.subscribe();

        loop {
            let port = *port_rx.borrow();

            if let Err(error) = Arc::clone(&self).serve(port, &mut port_rx).await {
                warn!(
                    port,
                    error = %error,
                    "nie udało się nasłuchiwać kiosków, ponawiam próbę"
                );

                // Czekamy albo na upływ czasu, albo na zmianę portu — bez tego pętla
                // kręciłaby się bez przerwy na zajętym porcie.
                tokio::select! {
                    () = tokio::time::sleep(RESTART_DELAY) => {}
                    result = port_rx.changed() => {
                        if result.is_err() {
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Nasłuchuje na porcie do momentu zmiany portu w ustawieniach.
    async fn serve(
        self: Arc<Self>,
        port: u16,
        port_rx: &mut watch::Receiver<u16>,
    ) -> std::io::Result<()> {
        // Nasłuchujemy na wszystkich interfejsach: kiosk stoi w tej samej sieci, a nie
        // na tym samym komputerze.
        let listener = TcpListener::bind(("0.0.0.0", port)).await?;

        self.set_listening(true);

        match local_ip() {
            Some(address) => info!(port, %address, "serwer kiosków nasłuchuje"),
            None => info!(
                port,
                "serwer kiosków nasłuchuje (nie udało się ustalić adresu LAN)"
            ),
        }

        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, peer)) => {
                            let server = Arc::clone(&self);

                            tauri::async_runtime::spawn(async move {
                                server.accept(stream, peer).await;
                            });
                        }
                        Err(error) => warn!(error = %error, "nie udało się przyjąć połączenia kiosku"),
                    }
                }
                result = port_rx.changed() => {
                    if result.is_ok() {
                        info!(port, "zmiana portu — restart serwera kiosków");
                    }

                    break;
                }
            }
        }

        self.set_listening(false);

        Ok(())
    }

    /// Przyjmuje połączenie i pilnuje go od parowania do rozłączenia.
    async fn accept(&self, stream: TcpStream, peer: SocketAddr) {
        // Kiosk w lokalnej sieci nie potrzebuje TLS-a: podsłuchujący w niej i tak jest
        // w niej fizycznie, a szyfrowanie komplikowałoby konfigurację bez realnego zysku.
        let ws = match tokio_tungstenite::accept_async(stream).await {
            Ok(ws) => ws,
            Err(error) => {
                warn!(peer = %peer, error = %error, "nie udało się zestawić połączenia WebSocket");

                return;
            }
        };

        self.handle_connection(ws, peer).await;
    }

    /// Pętla jednego kiosku: najpierw parowanie, potem komunikaty.
    async fn handle_connection(&self, mut ws: WebSocketStream<TcpStream>, peer: SocketAddr) {
        let Some((kiosk_id, name)) = self.pair(&mut ws, peer).await else {
            return;
        };

        self.mark_connected(kiosk_id, name.clone());

        while let Some(incoming) = ws.next().await {
            match incoming {
                Ok(Message::Text(text)) => {
                    if !self.handle_message(kiosk_id, text.as_str(), &mut ws).await {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                // Ping/pong obsługuje biblioteka, binarne komunikaty nie należą do protokołu.
                Ok(_) => {}
                Err(error) => {
                    debug!(peer = %peer, error = %error, "błąd odczytu od kiosku");

                    break;
                }
            }
        }

        self.mark_disconnected(kiosk_id);
    }

    /// Czeka na `hello` i sprawdza PIN. `None` oznacza, że połączenie zostało odrzucone.
    async fn pair(
        &self,
        ws: &mut WebSocketStream<TcpStream>,
        peer: SocketAddr,
    ) -> Option<(i64, String)> {
        let limits = self.limits();

        let text = match tokio::time::timeout(PAIRING_TIMEOUT, ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => text,
            Ok(Some(Ok(_))) => {
                warn!(peer = %peer, "pierwszy komunikat kiosku nie jest tekstem");

                self.reject(ws, ErrorCode::InvalidMessage).await;

                return None;
            }
            Ok(Some(Err(error))) => {
                debug!(peer = %peer, error = %error, "błąd odczytu przy parowaniu");

                return None;
            }
            Ok(None) => return None,
            Err(_) => {
                warn!(peer = %peer, "kiosk nie przysłał hello w wyznaczonym czasie");

                self.reject(ws, ErrorCode::InvalidMessage).await;

                return None;
            }
        };

        let message = match KioskMessage::from_json(text.as_str()) {
            Ok(message) => message,
            Err(error) => {
                warn!(peer = %peer, error = %error, "nieczytelny komunikat parowania");

                self.reject(ws, ErrorCode::InvalidMessage).await;

                return None;
            }
        };

        // Sprawdzenie wersji protokołu, formatu PIN-u i długości nazwy kiosku.
        if let Err(error) = message.validate_with(&limits) {
            warn!(peer = %peer, error = %error, "parowanie odrzucone przez walidację");

            self.reject(ws, ErrorCode::InvalidRequest).await;

            return None;
        }

        let KioskMessage::Hello {
            pin, kiosk_name, ..
        } = message
        else {
            warn!(peer = %peer, "pierwszy komunikat kiosku to nie hello");

            self.reject(ws, ErrorCode::InvalidMessage).await;

            return None;
        };

        if !self.pin_matches(&pin) {
            warn!(peer = %peer, "kiosk podał nieprawidłowy PIN");

            self.reject(ws, ErrorCode::Unauthorized).await;

            return None;
        }

        let name = match normalize_text(&kiosk_name) {
            Ok(name) if !name.is_empty() && count_chars(&name) <= MAX_KIOSK_NAME_CHARS => name,
            _ => {
                warn!(peer = %peer, "kiosk przysłał pustą albo dziwną nazwę");

                self.reject(ws, ErrorCode::InvalidRequest).await;

                return None;
            }
        };

        let db = Arc::clone(&self.db);
        let kiosk_id = match db_call({
            let name = name.clone();

            move || db.kiosk_by_name(&name)
        })
        .await
        {
            Ok(kiosk_id) => kiosk_id,
            Err(error) => {
                warn!(peer = %peer, error = %error, "nie udało się zapisać kiosku");

                self.reject(ws, ErrorCode::Internal).await;

                return None;
            }
        };

        let hello_ok = DjMessage::HelloOk {
            protocol_version: PROTOCOL_VERSION,
            kiosk_id,
            server_name: SERVER_NAME.to_string(),
            limits,
        };

        if !send(ws, &hello_ok).await {
            return None;
        }

        info!(peer = %peer, kiosk_id, name = %name, "kiosk sparowany");

        Some((kiosk_id, name))
    }

    /// Obsługuje jeden komunikat po parowaniu. Zwraca `false`, gdy trzeba zamknąć połączenie.
    async fn handle_message(
        &self,
        kiosk_id: i64,
        raw: &str,
        ws: &mut WebSocketStream<TcpStream>,
    ) -> bool {
        let message = match KioskMessage::from_json(raw) {
            Ok(message) => message,
            Err(error) => {
                // Nieznany komunikat zwykle oznacza niezgodne wersje aplikacji — nie ma sensu
                // dalej udawać, że rozmowa ma sens.
                warn!(kiosk_id, error = %error, "nieczytelny komunikat od kiosku");

                send(ws, &error_message(None, ErrorCode::InvalidMessage)).await;

                return false;
            }
        };

        let limits = self.limits();

        if let Err(error) = message.validate_with(&limits) {
            debug!(kiosk_id, error = %error, "komunikat odrzucony przez walidację");

            send(
                ws,
                &error_message(message.request_id(), ErrorCode::InvalidRequest),
            )
            .await;

            return true;
        }

        if !self.allow_message(kiosk_id) {
            warn!(kiosk_id, "kiosk przekroczył limit tempa");

            send(
                ws,
                &error_message(message.request_id(), ErrorCode::RateLimited),
            )
            .await;

            return true;
        }

        match message {
            // Drugie hello w tym samym połączeniu to błąd protokołu: kiosk jest już sparowany.
            KioskMessage::Hello { .. } => {
                send(ws, &error_message(None, ErrorCode::InvalidMessage)).await;

                false
            }
            KioskMessage::Search { request_id, query } => {
                self.handle_search(request_id, &query, ws).await;

                true
            }
            KioskMessage::SubmitRequest {
                request_id,
                track_id,
                dedication,
                guest_name,
            } => {
                self.handle_submit(
                    kiosk_id,
                    request_id,
                    track_id,
                    &dedication,
                    guest_name.as_deref(),
                    ws,
                )
                .await;

                true
            }
        }
    }

    /// Wyszukiwanie w bibliotece DJ-a na prośbę gościa.
    async fn handle_search(
        &self,
        request_id: u64,
        query: &str,
        ws: &mut WebSocketStream<TcpStream>,
    ) {
        // Zapytanie jest już znormalizowane przez walidację komunikatów.
        let query = match normalize_text(query) {
            Ok(query) if !query.is_empty() => query,
            _ => {
                send(
                    ws,
                    &error_message(Some(request_id), ErrorCode::InvalidRequest),
                )
                .await;

                return;
            }
        };

        let db = Arc::clone(&self.db);
        let search = db_call(move || db.search_tracks(&query, MAX_SEARCH_RESULTS as u32)).await;

        match search {
            Ok(tracks) => {
                debug!(
                    request_id,
                    results = tracks.len(),
                    "wyniki wyszukiwania dla kiosku"
                );

                let results = DjMessage::SearchResults { request_id, tracks };

                send(ws, &results).await;
            }
            Err(error) => {
                warn!(request_id, error = %error, "wyszukiwanie dla kiosku nie powiodło się");

                send(ws, &error_message(Some(request_id), ErrorCode::Internal)).await;
            }
        }
    }

    /// Przyjmuje prośbę gościa i wkłada ją do kolejki przeglądu.
    async fn handle_submit(
        &self,
        kiosk_id: i64,
        request_id: u64,
        track_id: i64,
        dedication: &str,
        guest_name: Option<&str>,
        ws: &mut WebSocketStream<TcpStream>,
    ) {
        let limits = self.limits();

        let dedication =
            match validate_dedication_with(dedication, limits.dedication_max_chars as usize) {
                Ok(dedication) => dedication,
                Err(error) => {
                    debug!(kiosk_id, error = %error, "dedykacja odrzucona");

                    send(
                        ws,
                        &error_message(Some(request_id), ErrorCode::InvalidRequest),
                    )
                    .await;

                    return;
                }
            };

        let guest_name = match validate_guest_name_with(
            guest_name.unwrap_or_default(),
            limits.guest_name_max_chars as usize,
        ) {
            Ok(guest_name) => guest_name,
            Err(error) => {
                debug!(kiosk_id, error = %error, "imię gościa odrzucone");

                send(
                    ws,
                    &error_message(Some(request_id), ErrorCode::InvalidRequest),
                )
                .await;

                return;
            }
        };

        let db = Arc::clone(&self.db);

        // Utwór musi istnieć w bibliotece — DJ ma go czym zagrać.
        match db_call({
            let db = Arc::clone(&db);

            move || db.track_exists(track_id)
        })
        .await
        {
            Ok(true) => {}
            Ok(false) => {
                warn!(
                    kiosk_id,
                    track_id, "prośba o utwór, którego nie ma w bibliotece"
                );

                send(
                    ws,
                    &error_message(Some(request_id), ErrorCode::InvalidRequest),
                )
                .await;

                return;
            }
            Err(error) => {
                warn!(kiosk_id, error = %error, "nie udało się sprawdzić utworu");

                send(ws, &error_message(Some(request_id), ErrorCode::Internal)).await;

                return;
            }
        }

        let since = now_ms() - DUPLICATE_WINDOW.as_millis() as i64;
        let duplicate = match db_call({
            let db = Arc::clone(&db);
            let dedication = dedication.clone();

            move || db.duplicate_request_exists(kiosk_id, track_id, &dedication, since)
        })
        .await
        {
            Ok(duplicate) => duplicate,
            Err(error) => {
                warn!(kiosk_id, error = %error, "nie udało się sprawdzić duplikatu");

                send(ws, &error_message(Some(request_id), ErrorCode::Internal)).await;

                return;
            }
        };

        if duplicate {
            debug!(kiosk_id, track_id, "duplikat prośby odrzucony");

            send(
                ws,
                &error_message(Some(request_id), ErrorCode::DuplicateRequest),
            )
            .await;

            return;
        }

        let inserted = db_call({
            let db = Arc::clone(&db);
            let dedication = dedication.clone();
            let guest_name = guest_name.clone();

            move || db.insert_request(track_id, &dedication, guest_name.as_deref(), Some(kiosk_id))
        })
        .await;

        match inserted {
            Ok((id, _created_at)) => {
                info!(
                    kiosk_id,
                    request_id = id,
                    track_id,
                    "nowa prośba od gościa w kolejce przeglądu"
                );

                send(
                    ws,
                    &DjMessage::RequestReceived {
                        request_id,
                        status: RequestStatus::Submitted,
                    },
                )
                .await;

                self.emit(UiEvent::RequestsChanged);
            }
            Err(error) => {
                warn!(kiosk_id, error = %error, "nie udało się zapisać prośby");

                send(ws, &error_message(Some(request_id), ErrorCode::Internal)).await;
            }
        }
    }

    /// Zamyka połączenie po wysłaniu kioskowi kodu błędu.
    async fn reject(&self, ws: &mut WebSocketStream<TcpStream>, code: ErrorCode) {
        send(ws, &error_message(None, code)).await;

        if let Err(error) = ws.close(None).await {
            debug!(error = %error, "nie udało się zamknąć odrzuconego połączenia");
        }
    }

    fn set_listening(&self, listening: bool) {
        match self.state.lock() {
            Ok(mut state) => state.listening = listening,
            Err(_) => return,
        };

        self.emit(UiEvent::KiosksChanged);
    }

    /// Czy kiosk nie przekroczył limitu tempa. Sprawdzenie „przy okazji” rejestruje zdarzenie.
    fn allow_message(&self, kiosk_id: i64) -> bool {
        match self.state.lock() {
            Ok(mut state) => state.limiter.allow(kiosk_id, Instant::now()),
            // Zatruty mutex jest po panice w innym wątku; odmowa jest bezpieczniejsza niż wpuszczenie.
            Err(_) => false,
        }
    }

    fn mark_connected(&self, kiosk_id: i64, name: String) {
        match self.state.lock() {
            Ok(mut state) => {
                state.kiosks.insert(kiosk_id, name);
            }
            Err(_) => return,
        };

        self.emit(UiEvent::KiosksChanged);
    }

    fn mark_disconnected(&self, kiosk_id: i64) {
        match self.state.lock() {
            Ok(mut state) => {
                state.kiosks.remove(&kiosk_id);
                // Historia tempa nie jest już potrzebna — kiosk przyjdzie od nowa.
                state.limiter.forget(kiosk_id);
            }
            Err(_) => return,
        };

        self.emit(UiEvent::KiosksChanged);
    }

    fn emit(&self, event: UiEvent) {
        if self.ui_events.send(event).is_err() {
            debug!("brak odbiorcy zdarzeń interfejsu");
        }
    }

    fn limits(&self) -> Limits {
        match self.settings.lock() {
            Ok(settings) => settings.limits,
            // Bezpieczny upadek: domyślne limity protokołu są ostrzejsze niż ustawienia DJ-a.
            Err(_) => Limits::default(),
        }
    }

    fn pin_matches(&self, pin: &str) -> bool {
        match self.settings.lock() {
            Ok(settings) => settings.pin == pin,
            Err(_) => false,
        }
    }
}

/// Limiter tempa: najwyżej `max_events` zdarzeń na klucz w oknie `window`.
///
/// Czas jest podawany z zewnątrz, dzięki czemu logikę można przetestować bez spania w testach
/// (AGENTS.md: logika ma być czysta i testowalna).
struct RateLimiter {
    max_events: usize,
    window: Duration,
    events: HashMap<i64, VecDeque<Instant>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RATE_MAX_MESSAGES, RATE_WINDOW)
    }
}

impl RateLimiter {
    fn new(max_events: usize, window: Duration) -> Self {
        Self {
            max_events,
            window,
            events: HashMap::new(),
        }
    }

    /// Czy klucz może teraz wykonać zdarzenie. Wywołanie „przy okazji” zapisuje zdarzenie.
    fn allow(&mut self, key: i64, now: Instant) -> bool {
        let events = self.events.entry(key).or_default();

        while let Some(oldest) = events.front() {
            if now.saturating_duration_since(*oldest) >= self.window {
                events.pop_front();
            } else {
                break;
            }
        }

        if events.len() >= self.max_events {
            return false;
        }

        events.push_back(now);

        true
    }

    fn forget(&mut self, key: i64) {
        self.events.remove(&key);
    }
}

/// Komunikat błędu do kiosku.
fn error_message(request_id: Option<u64>, code: ErrorCode) -> DjMessage {
    DjMessage::Error { request_id, code }
}

/// Wysyła komunikat do kiosku. `false` oznacza, że połączenie już nie działa.
async fn send<S>(sink: &mut S, message: &DjMessage) -> bool
where
    S: futures_util::Sink<Message, Error = WsError> + Unpin,
{
    let json = match message.to_json() {
        Ok(json) => json,
        Err(error) => {
            warn!(error = %error, "nie udało się zakodować komunikatu do kiosku");

            return false;
        }
    };

    match sink.send(Message::text(json)).await {
        Ok(()) => true,
        Err(error) => {
            debug!(error = %error, "nie udało się wysłać komunikatu do kiosku");

            false
        }
    }
}

/// Woła bazę na wątku roboczym — zapytanie SQLite nie może blokować wątku asynchronicznego.
async fn db_call<T, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, DbError> + Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(operation).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(format!("zadanie bazy nie powiodło się: {error}")),
    }
}

/// Adres IP tego komputera w sieci lokalnej.
///
/// `connect` na gnieździe UDP **niczego nie wysyła** — system tylko wybiera trasę i pozwala
/// odczytać adres lokalny. To najprostszy sposób bez dokładania crate'a do listy interfejsów.
/// Gdy nie ma trasy domyślnej (np. komputer odłączony od sieci), zwracamy `None`.
pub fn local_ip() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;

    socket.local_addr().ok().map(|address| address.ip())
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use tokio::net::TcpStream as TokioTcpStream;
    use tokio_tungstenite::MaybeTlsStream;

    use super::*;
    use crate::db::TrackRecord;
    use crate::settings::DEFAULT_PIN;

    /// Strumień po stronie klienta testowego.
    type ClientStream = WebSocketStream<MaybeTlsStream<TokioTcpStream>>;

    /// Wolny port na czas testu. Między zamknięciem a ponownym otwarciem jest małe okno,
    /// ale w testach lokalnych to wystarcza.
    fn free_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("wolny port");
        let port = listener.local_addr().expect("adres").port();

        drop(listener);

        port
    }

    fn hello(pin: &str) -> KioskMessage {
        KioskMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            pin: pin.to_string(),
            kiosk_name: "Kiosk testowy".to_string(),
        }
    }

    /// Łączy się z serwerem, czekając aż nasłuch wstanie.
    async fn connect(port: u16) -> ClientStream {
        let url = format!("ws://127.0.0.1:{port}");

        for _ in 0..50 {
            if let Ok((ws, _response)) = tokio_tungstenite::connect_async(&url).await {
                return ws;
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        panic!("serwer nie wstał na porcie {port}");
    }

    async fn send_message(ws: &mut ClientStream, message: &KioskMessage) {
        let json = message.to_json().expect("kodowanie komunikatu");

        ws.send(Message::text(json)).await.expect("wysyłka");
    }

    /// Odbiera komunikat z serwera. `None` oznacza zamknięcie połączenia.
    async fn next_message(ws: &mut ClientStream) -> Option<DjMessage> {
        let incoming = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("serwer milczy")
            .expect("połączenie zamknięte")
            .expect("błąd odczytu");

        match incoming {
            Message::Text(text) => DjMessage::from_json(text.as_str()).ok(),
            _ => None,
        }
    }

    /// Serwer z bazą w pamięci, gotowy do przyjmowania kiosków. Odbiorcę zdarzeń zwracamy,
    /// żeby kanał nie zamknął się od razu (test trzyma go w zmiennej).
    fn start_server(db: &Arc<Db>, port: u16) -> (Arc<Server>, mpsc::UnboundedReceiver<UiEvent>) {
        let settings = Arc::new(Mutex::new(AppSettings::default()));
        let (events, receiver) = mpsc::unbounded_channel();
        let server = Server::new(Arc::clone(db), settings, events, port);

        server.spawn();

        (server, receiver)
    }

    #[tokio::test]
    async fn a_kiosk_pairs_searches_and_submits_a_request() {
        let db = Arc::new(Db::open_in_memory().expect("baza w pamięci"));

        db.upsert_track(&TrackRecord::new(
            "C:\\muzyka\\kombi.mp3".to_string(),
            "Słodkiego, miłego życia".to_string(),
            "Kombi".to_string(),
            None,
            Some(255_000),
        ))
        .expect("utwór w bibliotece");

        let port = free_port();
        let (server, _events) = start_server(&db, port);

        // 1. Zły PIN nie wpuszcza kiosku, a serwer zamyka rozmowę.
        let mut ws = connect(port).await;
        send_message(&mut ws, &hello("000000")).await;

        assert!(matches!(
            next_message(&mut ws).await,
            Some(DjMessage::Error {
                code: ErrorCode::Unauthorized,
                ..
            })
        ));
        assert!(
            next_message(&mut ws).await.is_none(),
            "po złym PIN-ie serwer zamyka połączenie"
        );

        // 2. Właściwy PIN paruje kiosk i odsyła limity z ustawień DJ-a.
        let mut ws = connect(port).await;
        send_message(&mut ws, &hello(DEFAULT_PIN)).await;

        let hello_ok = next_message(&mut ws)
            .await
            .expect("brak odpowiedzi na hello");

        match hello_ok {
            DjMessage::HelloOk { limits, .. } => assert_eq!(limits, Limits::default()),
            other => panic!("oczekiwano hello_ok, a przyszło {other:?}"),
        }

        // 3. Wyszukiwanie w bibliotece DJ-a.
        send_message(
            &mut ws,
            &KioskMessage::Search {
                request_id: 1,
                query: "kombi".to_string(),
            },
        )
        .await;

        let tracks = match next_message(&mut ws).await.expect("brak wyników") {
            DjMessage::SearchResults { tracks, .. } => tracks,
            other => panic!("oczekiwano wyników, a przyszło {other:?}"),
        };

        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].artist, "Kombi");
        assert_eq!(tracks[0].title, "Słodkiego, miłego życia");

        // 4. Prośba gościa trafia do kolejki przeglądu.
        send_message(
            &mut ws,
            &KioskMessage::SubmitRequest {
                request_id: 2,
                track_id: tracks[0].id,
                dedication: "Dla Kasi i Marka — sto lat!".to_string(),
                guest_name: Some("Ania".to_string()),
            },
        )
        .await;

        assert!(matches!(
            next_message(&mut ws).await,
            Some(DjMessage::RequestReceived {
                status: RequestStatus::Submitted,
                ..
            })
        ));

        let queue = db
            .requests_with_status(RequestStatus::Submitted, 10)
            .expect("kolejka");

        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].dedication, "Dla Kasi i Marka — sto lat!");
        assert_eq!(queue[0].guest_name, Some("Ania".to_string()));
        assert_eq!(queue[0].title, "Słodkiego, miłego życia");

        assert_eq!(server.connected_kiosks(), vec!["Kiosk testowy".to_string()]);
    }

    #[tokio::test]
    async fn the_same_dedication_cannot_be_sent_twice_in_a_row() {
        let db = Arc::new(Db::open_in_memory().expect("baza w pamięci"));

        db.upsert_track(&TrackRecord::new(
            "C:\\muzyka\\a.mp3".to_string(),
            "Alfa".to_string(),
            "Zespół".to_string(),
            None,
            None,
        ))
        .expect("utwór w bibliotece");

        let port = free_port();
        let (_server, _events) = start_server(&db, port);

        let mut ws = connect(port).await;
        send_message(&mut ws, &hello(DEFAULT_PIN)).await;
        assert!(next_message(&mut ws).await.is_some(), "parowanie");

        let submit = KioskMessage::SubmitRequest {
            request_id: 1,
            track_id: 1,
            dedication: "Sto lat!".to_string(),
            guest_name: None,
        };

        send_message(&mut ws, &submit).await;
        assert!(matches!(
            next_message(&mut ws).await,
            Some(DjMessage::RequestReceived { .. })
        ));

        // To samo zgłoszenie drugi raz — gość kliknął „Wyślij” dwa razy.
        send_message(&mut ws, &submit).await;
        assert!(matches!(
            next_message(&mut ws).await,
            Some(DjMessage::Error {
                code: ErrorCode::DuplicateRequest,
                ..
            })
        ));

        assert_eq!(
            db.requests_with_status(RequestStatus::Submitted, 10)
                .expect("kolejka")
                .len(),
            1,
            "duplikat nie może wejść do kolejki"
        );
    }

    #[tokio::test]
    async fn a_request_for_an_unknown_track_is_rejected() {
        let db = Arc::new(Db::open_in_memory().expect("baza w pamięci"));
        let port = free_port();
        let (_server, _events) = start_server(&db, port);

        let mut ws = connect(port).await;
        send_message(&mut ws, &hello(DEFAULT_PIN)).await;
        assert!(next_message(&mut ws).await.is_some(), "parowanie");

        send_message(
            &mut ws,
            &KioskMessage::SubmitRequest {
                request_id: 1,
                track_id: 999,
                dedication: "Sto lat!".to_string(),
                guest_name: None,
            },
        )
        .await;

        assert!(matches!(
            next_message(&mut ws).await,
            Some(DjMessage::Error {
                code: ErrorCode::InvalidRequest,
                ..
            })
        ));
    }

    #[test]
    fn rate_limiter_allows_up_to_the_limit() {
        let mut limiter = RateLimiter::new(3, Duration::from_secs(10));
        let start = Instant::now();

        assert!(limiter.allow(1, start));
        assert!(limiter.allow(1, start));
        assert!(limiter.allow(1, start));
        assert!(!limiter.allow(1, start), "czwarty komunikat w oknie");
    }

    #[test]
    fn rate_limiter_frees_the_window_over_time() {
        let mut limiter = RateLimiter::new(2, Duration::from_secs(10));
        let start = Instant::now();

        assert!(limiter.allow(1, start));
        assert!(limiter.allow(1, start));
        assert!(!limiter.allow(1, start + Duration::from_secs(5)));

        // Po upływie okna najstarsze zdarzenia wypadają i znowu można mówić.
        assert!(limiter.allow(1, start + Duration::from_secs(11)));
    }

    #[test]
    fn rate_limiter_counts_each_kiosk_separately() {
        let mut limiter = RateLimiter::new(1, Duration::from_secs(10));
        let start = Instant::now();

        assert!(limiter.allow(1, start));
        assert!(!limiter.allow(1, start));
        assert!(limiter.allow(2, start), "drugi kiosk ma własny licznik");
    }

    #[test]
    fn rate_limiter_forgets_a_disconnected_kiosk() {
        let mut limiter = RateLimiter::new(1, Duration::from_secs(10));
        let start = Instant::now();

        assert!(limiter.allow(7, start));
        limiter.forget(7);

        assert!(
            limiter.allow(7, start),
            "po rozłączeniu licznik startuje od zera"
        );
    }
}
