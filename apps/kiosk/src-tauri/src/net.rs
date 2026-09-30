//! Klient kiosku: połączenie z aplikacją DJ-a, wyszukiwanie i wysyłanie prośby
//! (PLAN.md, sekcje 5 i 6).
//!
//! Kiosk tylko pyta — nie ma żadnego komunikatu, którym mógłby sterować aplikacją DJ-a.
//! Połączenie ponawia się samo, bo przy imprezie sieć bywa kapryśna, a gość nie może zostać
//! z ekranem, który nic nie robi.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{
    DjMessage, ErrorCode, KioskMessage, Limits, PROTOCOL_VERSION, RequestStatus, TrackInfo,
};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::error::Error as WsError;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tracing::{debug, info, warn};

/// Ile czekamy na odpowiedź na `hello`.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Pierwsza przerwa przed ponowieniem połączenia.
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Najdłuższa przerwa przed ponowieniem — dłuższe czekanie denerwuje gościa.
const MAX_BACKOFF: Duration = Duration::from_secs(15);

/// Zakres portu, jaki można wpisać na ekranie konfiguracji.
pub const PORT_RANGE: (u16, u16) = (1024, 65_535);

/// Typ strumienia zwracanego przez `connect_async` dla adresu `ws://`.
type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Dane wpisane przez DJ-a na ekranie konfiguracji kiosku.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ServerConfig {
    /// Adres komputera DJ-a, np. `192.168.0.10`.
    pub address: String,
    pub port: u16,
    pub pin: String,
    pub kiosk_name: String,
}

impl ServerConfig {
    /// Buduje adres WebSocket. Sprawdzamy go sami, bo literówka w adresie objawiłaby się
    /// dopiero jako „kiosk nie chce się połączyć”, bez śladu przyczyny.
    pub fn url(&self) -> Result<String, ClientError> {
        let host = self.address.trim();

        if host.is_empty() {
            return Err(ClientError::MissingAddress);
        }

        let looks_like_host = host
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'));

        if !looks_like_host {
            return Err(ClientError::InvalidAddress);
        }

        if self.port < PORT_RANGE.0 {
            return Err(ClientError::InvalidPort);
        }

        Ok(format!("ws://{host}:{}", self.port))
    }
}

/// Stan połączenia kiosku z aplikacją DJ-a.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectionState {
    /// Brak połączenia: albo kiosk nie jest jeszcze skonfigurowany, albo sieć padła.
    #[default]
    Disconnected,
    /// Trwa łączenie albo ponawianie.
    Connecting,
    /// Sparowany z aplikacją DJ-a.
    Connected {
        server_name: String,
        kiosk_id: i64,
        /// Limity ustawione przez DJ-a — kiosk pokazuje gościowi ten sam limit, co walidacja
        /// po stronie DJ-a.
        limits: Limits,
        /// Po ilu sekundach ekran potwierdzenia sam wraca do wyszukiwania (ustawienie DJ-a).
        confirmation_seconds: u32,
    },
}

/// Zdarzenie dla interfejsu kiosku.
#[derive(Debug, Clone)]
pub enum UiEvent {
    Connection(ConnectionState),
    SearchResults {
        request_id: u64,
        tracks: Vec<TrackInfo>,
    },
    RequestReceived {
        request_id: u64,
        status: RequestStatus,
    },
    Error {
        request_id: Option<u64>,
        code: ErrorCode,
    },
}

/// Błąd klienta kiosku.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("podaj adres komputera DJ-a")]
    MissingAddress,

    #[error("adres ma być jak 192.168.0.10 — bez „ws://” i bez portu")]
    InvalidAddress,

    #[error("port musi być liczbą z zakresu {}-{}", PORT_RANGE.0, PORT_RANGE.1)]
    InvalidPort,

    #[error("nie ma połączenia z aplikacją DJ-a")]
    NotConnected,

    #[error("stan połączenia jest chwilowo niedostępny")]
    Poisoned,
}

/// Klient kiosku. Jeden na aplikację.
pub struct Client {
    inner: Mutex<Inner>,
    ui_events: mpsc::UnboundedSender<UiEvent>,
}

/// Stan klienta pilnowany pod mutexem. Sekcje krytyczne są krótkie i bez `await`.
struct Inner {
    /// Kanał do aktualnie działającego połączenia (`None` = nie ma połączenia).
    outbox: Option<mpsc::UnboundedSender<KioskMessage>>,
    state: ConnectionState,
    /// Numer bieżącej sesji. Rośnie przy każdym łączeniu i rozłączeniu; starsze zadania
    /// widzą, że nie są już aktualne, i kończą się same.
    session: u64,
    /// Licznik identyfikatorów próśb wysyłanych do DJ-a.
    next_request_id: u64,
}

impl Client {
    /// Tworzy klienta (jeszcze się nie łączy).
    pub fn new(ui_events: mpsc::UnboundedSender<UiEvent>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                outbox: None,
                state: ConnectionState::default(),
                session: 0,
                next_request_id: 1,
            }),
            ui_events,
        })
    }

    /// Bieżący stan połączenia.
    pub fn state(&self) -> ConnectionState {
        match self.inner.lock() {
            Ok(inner) => inner.state.clone(),
            Err(_) => ConnectionState::Disconnected,
        }
    }

    /// Zaczyna łączyć (i ponawiać) połączenie z podanym serwerem.
    pub fn connect(self: &Arc<Self>, config: ServerConfig) -> Result<(), ClientError> {
        let url = config.url()?;

        // Nowa sesja unieważnia poprzednie zadania.
        let session = {
            let mut inner = self.lock()?;
            inner.session += 1;
            inner.outbox = None;
            inner.session
        };

        self.publish(session, ConnectionState::Connecting);

        let client = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            client.run_session(config, url, session).await;
        });

        Ok(())
    }

    /// Rozłącza i przywraca ekran konfiguracji.
    pub fn disconnect(&self) {
        let session = {
            let Ok(mut inner) = self.inner.lock() else {
                return;
            };

            inner.session += 1;
            inner.outbox = None;
            inner.session
        };

        self.publish(session, ConnectionState::Disconnected);
    }

    /// Wysyła zapytanie o utwory i zwraca identyfikator prośby, którego dotyczy odpowiedź.
    pub fn search(&self, query: String) -> Result<u64, ClientError> {
        let request_id = self.next_request_id()?;

        self.send(KioskMessage::Search { request_id, query })?;

        Ok(request_id)
    }

    /// Wysyła prośbę gościa i zwraca identyfikator, którego dotyczy odpowiedź.
    pub fn submit_request(
        &self,
        track_id: i64,
        dedication: String,
        guest_name: Option<String>,
    ) -> Result<u64, ClientError> {
        let request_id = self.next_request_id()?;

        self.send(KioskMessage::SubmitRequest {
            request_id,
            track_id,
            dedication,
            guest_name,
        })?;

        Ok(request_id)
    }

    /// Kolejny wolny identyfikator prośby.
    fn next_request_id(&self) -> Result<u64, ClientError> {
        let mut inner = self.lock()?;
        let request_id = inner.next_request_id;
        inner.next_request_id += 1;

        Ok(request_id)
    }

    /// Wysyła komunikat do aplikacji DJ-a.
    fn send(&self, message: KioskMessage) -> Result<(), ClientError> {
        let inner = self.lock()?;
        let outbox = inner.outbox.as_ref().ok_or(ClientError::NotConnected)?;

        outbox.send(message).map_err(|_| ClientError::NotConnected)
    }

    /// Pętla jednej sesji: łączenie, praca, a po rozłączeniu ponawianie.
    async fn run_session(self: Arc<Self>, config: ServerConfig, url: String, session: u64) {
        let mut backoff = INITIAL_BACKOFF;

        loop {
            if !self.is_current(session) {
                return;
            }

            self.publish(session, ConnectionState::Connecting);

            match connect_async(&url).await {
                Ok((stream, _response)) => {
                    info!(url = %url, "połączono z aplikacją DJ-a");
                    backoff = INITIAL_BACKOFF;

                    if self.session_loop(stream, &config, session).await == SessionEnd::Stop {
                        break;
                    }
                }
                Err(error) => {
                    warn!(url = %url, error = %error, "nie udało się połączyć z aplikacją DJ-a");
                }
            }

            if !self.is_current(session) {
                return;
            }

            self.publish(session, ConnectionState::Disconnected);

            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }

        self.publish(session, ConnectionState::Disconnected);
        warn!("kiosk nie ponowi połączenia — sprawdź adres i PIN");
    }

    /// Jedno połączenie: parowanie, a potem obsługa komunikatów.
    async fn session_loop(
        &self,
        stream: WsStream,
        config: &ServerConfig,
        session: u64,
    ) -> SessionEnd {
        let (mut sink, mut incoming) = stream.split();

        // Parowanie PIN-em — bez tego DJ nie przyjmie od nas żadnej prośby.
        let hello = KioskMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            pin: config.pin.clone(),
            kiosk_name: config.kiosk_name.clone(),
        };

        if !send_ws(&mut sink, &hello).await {
            return SessionEnd::Retry;
        }

        let reply = match tokio::time::timeout(HELLO_TIMEOUT, incoming.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => KioskMessageReply::parse(text.as_str()),
            Ok(Some(Ok(_))) | Ok(None) => return SessionEnd::Retry,
            Ok(Some(Err(error))) => {
                debug!(error = %error, "błąd odczytu przy parowaniu");

                return SessionEnd::Retry;
            }
            Err(_) => {
                warn!("aplikacja DJ-a nie odpowiedziała na hello");

                return SessionEnd::Retry;
            }
        };

        let (server_name, kiosk_id, limits, confirmation_seconds) = match reply {
            KioskMessageReply::HelloOk {
                server_name,
                kiosk_id,
                limits,
                confirmation_seconds,
            } => (server_name, kiosk_id, limits, confirmation_seconds),
            KioskMessageReply::Error { code } => {
                warn!(code = ?code, "aplikacja DJ-a odrzuciła parowanie");

                self.emit(UiEvent::Error {
                    request_id: None,
                    code,
                });

                // Zły PIN nie naprawi się sam — ponawianie tylko by go dobijało.
                return if code == ErrorCode::Unauthorized {
                    SessionEnd::Stop
                } else {
                    SessionEnd::Retry
                };
            }
            KioskMessageReply::Other => return SessionEnd::Retry,
        };

        let (outbox, mut outgoing) = mpsc::unbounded_channel::<KioskMessage>();

        if !self.publish_connected(
            session,
            outbox,
            server_name,
            kiosk_id,
            limits,
            confirmation_seconds,
        ) {
            return SessionEnd::Stop;
        }

        loop {
            tokio::select! {
                incoming_message = incoming.next() => {
                    match incoming_message {
                        Some(Ok(Message::Text(text))) => {
                            if self.handle_dj_message(text.as_str()) == SessionEnd::Stop {
                                return SessionEnd::Stop;
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => return SessionEnd::Retry,
                        Some(Ok(_)) => {}
                        Some(Err(error)) => {
                            debug!(error = %error, "błąd odczytu od aplikacji DJ-a");

                            return SessionEnd::Retry;
                        }
                    }
                }
                outgoing_message = outgoing.recv() => {
                    match outgoing_message {
                        Some(message) => {
                            if !send_ws(&mut sink, &message).await {
                                return SessionEnd::Retry;
                            }
                        }
                        // Kanał zamknięty = nowa sesja albo rozłączenie z interfejsu.
                        None => return SessionEnd::Stop,
                    }
                }
            }
        }
    }

    /// Obsługuje komunikat od aplikacji DJ-a. `Stop` oznacza koniec pracy klienta.
    fn handle_dj_message(&self, raw: &str) -> SessionEnd {
        let message = match DjMessage::from_json(raw) {
            Ok(message) => message,
            Err(error) => {
                warn!(error = %error, "nieczytelny komunikat od aplikacji DJ-a");

                return SessionEnd::Retry;
            }
        };

        match message {
            DjMessage::SearchResults { request_id, tracks } => {
                self.emit(UiEvent::SearchResults { request_id, tracks });
            }
            DjMessage::RequestReceived { request_id, status } => {
                self.emit(UiEvent::RequestReceived { request_id, status });
            }
            DjMessage::RequestStatus { request_id, status } => {
                self.emit(UiEvent::RequestReceived { request_id, status });
            }
            DjMessage::Error { request_id, code } => {
                self.emit(UiEvent::Error { request_id, code });
            }
            // Powtórzone hello_ok w trakcie rozmowy nie niesie nic nowego.
            DjMessage::HelloOk { .. } => {}
        }

        SessionEnd::Retry
    }

    fn lock(&self) -> Result<MutexGuard<'_, Inner>, ClientError> {
        self.inner.lock().map_err(|_| ClientError::Poisoned)
    }

    /// Czy zadanie o tym numerze nadal jest aktualne.
    fn is_current(&self, session: u64) -> bool {
        match self.inner.lock() {
            Ok(inner) => inner.session == session,
            Err(_) => false,
        }
    }

    /// Zapisuje stan i wysyła go do interfejsu. Cisza, gdy sesja jest już nieaktualna.
    fn publish(&self, session: u64, state: ConnectionState) {
        let changed = match self.inner.lock() {
            Ok(mut inner) if inner.session == session => {
                inner.state = state.clone();

                true
            }
            Ok(_) => false,
            Err(_) => false,
        };

        if changed {
            self.emit(UiEvent::Connection(state));
        }
    }

    /// Rejestruje kanał wysyłania i przechodzi w stan „połączony”.
    fn publish_connected(
        &self,
        session: u64,
        outbox: mpsc::UnboundedSender<KioskMessage>,
        server_name: String,
        kiosk_id: i64,
        limits: Limits,
        confirmation_seconds: u32,
    ) -> bool {
        let state = ConnectionState::Connected {
            server_name,
            kiosk_id,
            limits,
            confirmation_seconds,
        };

        let registered = match self.inner.lock() {
            Ok(mut inner) if inner.session == session => {
                inner.outbox = Some(outbox);
                inner.state = state.clone();

                true
            }
            Ok(_) => false,
            Err(_) => false,
        };

        if registered {
            self.emit(UiEvent::Connection(state));
        }

        registered
    }

    fn emit(&self, event: UiEvent) {
        if self.ui_events.send(event).is_err() {
            debug!("brak odbiorcy zdarzeń interfejsu");
        }
    }
}

/// Czy sesja ma być ponowiona, czy zakończona na dobre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionEnd {
    Retry,
    Stop,
}

/// Odpowiedź na `hello` — tylko tyle nas interesuje z pierwszego komunikatu.
enum KioskMessageReply {
    HelloOk {
        server_name: String,
        kiosk_id: i64,
        limits: Limits,
        confirmation_seconds: u32,
    },
    Error {
        code: ErrorCode,
    },
    Other,
}

impl KioskMessageReply {
    fn parse(raw: &str) -> Self {
        match DjMessage::from_json(raw) {
            Ok(DjMessage::HelloOk {
                server_name,
                kiosk_id,
                limits,
                confirmation_seconds,
                ..
            }) => Self::HelloOk {
                server_name,
                kiosk_id,
                limits,
                confirmation_seconds,
            },
            Ok(DjMessage::Error { code, .. }) => Self::Error { code },
            Ok(_) | Err(_) => Self::Other,
        }
    }
}

/// Wysyła komunikat do aplikacji DJ-a. `false` oznacza, że połączenie padło.
async fn send_ws<S>(sink: &mut S, message: &KioskMessage) -> bool
where
    S: futures_util::Sink<Message, Error = WsError> + Unpin,
{
    let json = match message.to_json() {
        Ok(json) => json,
        Err(error) => {
            warn!(error = %error, "nie udało się zakodować komunikatu");

            return false;
        }
    };

    match sink.send(Message::text(json)).await {
        Ok(()) => true,
        Err(error) => {
            debug!(error = %error, "nie udało się wysłać komunikatu do aplikacji DJ-a");

            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use tokio::net::TcpListener;

    use super::*;

    /// Odbiera tekst z gniazda atrapy serwera.
    async fn next_text<S>(ws: &mut WebSocketStream<S>) -> Option<String>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => Some(text.to_string()),
            _ => None,
        }
    }

    /// Czeka na zdarzenie z kanału, aż `check` je zaakceptuje.
    async fn wait_for_event(
        events: &mut mpsc::UnboundedReceiver<UiEvent>,
        mut check: impl FnMut(&UiEvent) -> bool,
    ) -> UiEvent {
        let deadline = Instant::now() + Duration::from_secs(5);

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());

            let event = tokio::time::timeout(remaining, events.recv())
                .await
                .expect("zdarzenie nie przyszło w czasie")
                .expect("kanał zdarzeń zamknięty");

            if check(&event) {
                return event;
            }
        }
    }

    #[tokio::test]
    async fn the_client_pairs_and_searches_through_a_stub_server() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("nasłuch testowy");
        let port = listener.local_addr().expect("adres").port();

        // Atrapa aplikacji DJ-a: sprawdza, co kiosk wysyła, i odpowiada jak prawdziwy serwer.
        tokio::spawn(async move {
            let (stream, _peer) = listener.accept().await.expect("połączenie");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("handshake");

            let hello = next_text(&mut ws).await.expect("kiosk musi się przywitać");

            assert!(
                matches!(
                    KioskMessage::from_json(&hello),
                    Ok(KioskMessage::Hello { .. })
                ),
                "pierwszy komunikat kiosku to hello"
            );

            ws.send(Message::text(
                DjMessage::HelloOk {
                    protocol_version: PROTOCOL_VERSION,
                    kiosk_id: 7,
                    server_name: "ANON DJ".to_string(),
                    limits: Limits::default(),
                    confirmation_seconds: 20,
                }
                .to_json()
                .expect("kodowanie hello_ok"),
            ))
            .await
            .expect("wysyłka hello_ok");

            let search = next_text(&mut ws)
                .await
                .expect("kiosk musi zapytać o utwory");

            let Ok(KioskMessage::Search { request_id, query }) = KioskMessage::from_json(&search)
            else {
                panic!("oczekiwano zapytania, a przyszło: {search}");
            };

            assert_eq!(query, "kombi");

            ws.send(Message::text(
                DjMessage::SearchResults {
                    request_id,
                    tracks: vec![TrackInfo {
                        id: 3,
                        title: "Słodkiego, miłego życia".to_string(),
                        artist: "Kombi".to_string(),
                        album: None,
                        duration_ms: Some(255_000),
                    }],
                }
                .to_json()
                .expect("kodowanie wyników"),
            ))
            .await
            .expect("wysyłka wyników");
        });

        let (events, mut received) = mpsc::unbounded_channel();
        let client = Client::new(events);
        client
            .connect(config("127.0.0.1", port))
            .expect("start połączenia");

        // Parowanie: kiosk musi dostać HelloOk i przejść w stan „połączony”.
        match wait_for_event(&mut received, |event| {
            matches!(
                event,
                UiEvent::Connection(ConnectionState::Connected { .. })
            )
        })
        .await
        {
            UiEvent::Connection(ConnectionState::Connected { kiosk_id, .. }) => {
                assert_eq!(kiosk_id, 7)
            }
            other => panic!("oczekiwano połączenia, a przyszło {other:?}"),
        }

        // Po sparowaniu kiosk może pytać o utwory.
        let request_id = client.search("kombi".to_string()).expect("zapytanie");

        match wait_for_event(&mut received, |event| {
            matches!(event, UiEvent::SearchResults { request_id: id, .. } if *id == request_id)
        })
        .await
        {
            UiEvent::SearchResults { tracks, .. } => {
                assert_eq!(tracks.len(), 1);
                assert_eq!(tracks[0].artist, "Kombi");
            }
            other => panic!("oczekiwano wyników, a przyszło {other:?}"),
        }
    }

    fn config(address: &str, port: u16) -> ServerConfig {
        ServerConfig {
            address: address.to_string(),
            port,
            pin: "123456".to_string(),
            kiosk_name: "Kiosk przy wejściu".to_string(),
        }
    }

    #[test]
    fn a_plain_address_becomes_a_websocket_url() {
        assert_eq!(
            config("192.168.0.10", 8790).url().expect("adres"),
            "ws://192.168.0.10:8790"
        );
        assert_eq!(
            config("  dj-komputer ", 9000).url().expect("adres"),
            "ws://dj-komputer:9000"
        );
    }

    #[test]
    fn a_wrong_address_is_rejected_with_a_reason() {
        assert!(matches!(
            config("", 8790).url(),
            Err(ClientError::MissingAddress)
        ));
        assert!(matches!(
            config("ws://192.168.0.10", 8790).url(),
            Err(ClientError::InvalidAddress)
        ));
        assert!(matches!(
            config("192.168.0.10:8790", 8790).url(),
            Err(ClientError::InvalidAddress)
        ));
        assert!(matches!(
            config("192.168.0.10", 80).url(),
            Err(ClientError::InvalidPort)
        ));
    }

    #[test]
    fn sending_without_a_connection_is_an_error() {
        let (events, _receiver) = mpsc::unbounded_channel();
        let client = Client::new(events);

        assert!(matches!(
            client.search("kombi".to_string()),
            Err(ClientError::NotConnected)
        ));
        assert!(matches!(
            client.submit_request(1, "Sto lat!".to_string(), None),
            Err(ClientError::NotConnected)
        ));
    }

    #[test]
    fn request_identifiers_are_unique() {
        let (events, _receiver) = mpsc::unbounded_channel();
        let client = Client::new(events);

        let first = client.next_request_id().expect("identyfikator");
        let second = client.next_request_id().expect("identyfikator");

        assert_ne!(first, second);
    }
}
