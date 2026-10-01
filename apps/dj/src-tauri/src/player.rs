//! Adapter odtwarzacza (PLAN.md, sekcja 7).
//!
//! Wszystko, co wie o foobar2000 i beefweb, siedzi **tutaj**. Reszta aplikacji widzi wyłącznie
//! cechę [`PlayerAdapter`] i typy z tego modułu — dzięki temu kolejny odtwarzacz (AIMP, VLC)
//! nie wymaga zmian w kolejce ani w interfejsie.
//!
//! Klient HTTP jest synchroniczny (`ureq`), bo wywołania i tak muszą iść przez `spawn_blocking`:
//! ani żądanie do beefweb, ani odpowiedź nie mogą blokować wątku UI (AGENTS.md).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::volume::{MAX_VOLUME_DB, clamp_db};

/// Prefiks adresu API odtwarzacza: `{base}/api/player` itd.
const API_PREFIX: &str = "/api";

/// Kolumny, o które pytamy stan odtwarzacza. beefweb zwraca je w tej samej kolejności.
const STATE_COLUMNS: &str = "%25title%25,%25artist%25,%25path%25";
/// Kolumna ze ścieżką pliku, używana przy dopasowywaniu utworu z biblioteki.
const PATH_COLUMN: &str = "%25path%25";

/// Nazwa playlisty, do której odkładamy utwory, których nie ma w żadnej playliście DJ-a.
const STAGING_PLAYLIST: &str = "ANON DJ";

/// Ile elementów playlisty przeglądamy szukając ścieżki. Powyżej tego limitu uznajemy, że utworu
/// w playliście nie ma i odkładamy go na scenę — lepiej wykonać jedno dodatkowe żądanie, niż
/// blokować interfejs przy playliście z dziesiątkami tysięcy pozycji.
const ITEM_SCAN_LIMIT: u32 = 10_000;

/// Jak długo czekamy na odpowiedź odtwarzacza. Powyżej tego uznajemy, że go nie ma — DJ nie może
/// czekać w środku imprezy, a stan i tak odświeżamy w tle.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Błąd pracy z odtwarzaczem.
#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    /// Odtwarzacz nie odpowiada (nie działa, zły adres, brak beefweb).
    #[error("odtwarzacz nie odpowiada: {0}")]
    Unavailable(String),

    /// Odtwarzacz odpowiedział, ale odrzucił żądanie.
    #[error("odtwarzacz odrzucił żądanie (HTTP {status}): {message}")]
    Rejected { status: u16, message: String },

    /// Odpowiedź nie miała kształtu, którego się spodziewamy.
    #[error("nieoczekiwana odpowiedź odtwarzacza: {0}")]
    Protocol(String),

    /// Nie da się zbudować klienta dla podanego adresu.
    #[error("nieprawidłowy adres odtwarzacza: {0}")]
    Config(String),
}

/// Stan odtwarzania raportowany przez odtwarzacz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
}

impl PlaybackState {
    /// Mapuje stan z API beefweb. Nieznana wartość to `Stopped` — bezpieczniej uznać, że nic nie
    /// gra, niż pokazać DJ-owi „gra”, kiedy nie gra.
    fn from_wire(value: &str) -> Self {
        match value {
            "playing" => Self::Playing,
            "paused" => Self::Paused,
            _ => Self::Stopped,
        }
    }
}

/// Stan odtwarzacza widziany przez aplikację DJ-a.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerStatus {
    /// Czy odtwarzacz odpowiada. `false` oznacza tryb pracy bez odtwarzacza (degraded mode).
    pub available: bool,
    pub state: PlaybackState,
    pub title: Option<String>,
    pub artist: Option<String>,
    /// Ścieżka pliku aktualnie odtwarzanego utworu — po niej poznajemy, czy to zamówienie.
    pub path: Option<String>,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    /// Bieżący wolumen odtwarzacza w decybelach (`0.0` = maksimum, `-100.0` = cisza).
    pub volume_db: f64,
}

impl PlayerStatus {
    /// Stan, gdy odtwarzacza nie ma. Aplikacja ma działać dalej (PLAN.md, sekcja 7).
    pub fn offline() -> Self {
        Self {
            available: false,
            state: PlaybackState::Stopped,
            title: None,
            artist: None,
            path: None,
            position_seconds: 0.0,
            duration_seconds: 0.0,
            // Poziom neutralny: nie znamy głośności odtwarzacza, a zero to maksimum skali.
            volume_db: MAX_VOLUME_DB,
        }
    }
}

/// Cecha odtwarzacza. Implementacje są synchroniczne — wywołujący odpowiada za odsunięcie ich
/// od wątku UI.
pub trait PlayerAdapter: Send + Sync {
    /// Bieżący stan odtwarzacza.
    fn status(&self) -> Result<PlayerStatus, PlayerError>;

    /// Dodaje plik do kolejki odtwarzania, żeby zagrał **po** bieżącym utworze.
    fn play_next(&self, path: &str) -> Result<(), PlayerError>;

    /// Dodaje plik do kolejki i od razu go uruchamia.
    fn play_now(&self, path: &str) -> Result<(), PlayerError>;

    /// Ustawia głośność odtwarzacza (decybele, `0.0` = maksimum).
    ///
    /// Rampy wysyłają tę wartość krok po kroku — patrz [`crate::volume`].
    fn set_volume_db(&self, volume_db: f64) -> Result<(), PlayerError>;

    /// Włącza albo zdejmuje „stop po bieżącym utworze”.
    ///
    /// Włączamy to na czas sekwencji, żeby utwór, który skończy się pod lektorem, **zatrzymał**
    /// odtwarzanie, a nie wciągnął pod głos następnej pozycji playlisty (PLAN.md, sekcja 7).
    fn set_stop_after_current(&self, stop: bool) -> Result<(), PlayerError>;

    /// Zatrzymuje odtwarzanie. Bieżący utwór zostaje wybrany, ale milknie.
    fn stop(&self) -> Result<(), PlayerError>;
}

/// Adapter foobar2000 + beefweb.
pub struct BeefwebAdapter {
    base_url: String,
    agent: ureq::Agent,
}

impl BeefwebAdapter {
    /// Buduje adapter dla bazowego adresu API, np. `http://localhost:8880`.
    pub fn new(base_url: &str) -> Result<Self, PlayerError> {
        let base_url = base_url.trim().trim_end_matches('/').to_string();

        if !base_url.starts_with("http://") || base_url.len() <= "http://".len() {
            return Err(PlayerError::Config(base_url));
        }

        // Bez TLS (beefweb stoi na localhost), bez proxy z otoczenia (żądanie do localhost nie ma
        // prawa trafić przez firmowe proxy) i bez podążania za przekierowaniami.
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent("ANON-DJ")
            .build();

        Ok(Self {
            base_url,
            agent: config.into(),
        })
    }

    /// Dokłada prefiks API do ścieżki i składa pełny adres.
    fn url(&self, path: &str) -> String {
        format!("{}{}{}", self.base_url, API_PREFIX, path)
    }

    /// `GET` po JSON.
    fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, PlayerError> {
        let response = self
            .agent
            .get(self.url(path))
            .call()
            .map_err(PlayerError::from_transport)?;

        let response = check_status(response)?;

        response
            .into_body()
            .read_json::<T>()
            .map_err(|error| PlayerError::Protocol(error.to_string()))
    }

    /// `POST` z ciałem JSON. Sukces to dowolny kod 2xx.
    fn post_json(&self, path: &str, body: &serde_json::Value) -> Result<(), PlayerError> {
        let response = self
            .agent
            .post(self.url(path))
            .send_json(body)
            .map_err(PlayerError::from_transport)?;

        check_status(response).map(|_| ())
    }

    /// `POST` bez ciała.
    fn post_empty(&self, path: &str) -> Result<(), PlayerError> {
        let response = self
            .agent
            .post(self.url(path))
            .send_empty()
            .map_err(PlayerError::from_transport)?;

        check_status(response).map(|_| ())
    }

    /// Lista playlist.
    fn playlists(&self) -> Result<Vec<RawPlaylist>, PlayerError> {
        let envelope: PlaylistsEnvelope = self.get_json("/playlists")?;

        Ok(envelope.playlists)
    }

    /// Ścieżki elementów playlisty (w kolejności).
    fn playlist_paths(&self, playlist_id: &str) -> Result<Vec<String>, PlayerError> {
        let path =
            format!("/playlists/{playlist_id}/items/0:{ITEM_SCAN_LIMIT}?columns={PATH_COLUMN}");
        let envelope: PlaylistItemsEnvelope = self.get_json(&path)?;

        Ok(envelope
            .playlist_items
            .items
            .into_iter()
            .map(|item| item.columns.into_iter().next().unwrap_or_default())
            .collect())
    }

    /// Szuka utworu w istniejących playlistach. Zwraca `(id playlisty, indeks)`.
    fn find_in_playlists(&self, path: &str) -> Result<Option<(String, i64)>, PlayerError> {
        for playlist in self.playlists()? {
            let paths = self.playlist_paths(&playlist.id)?;

            if let Some(index) = paths
                .iter()
                .position(|candidate| same_path(candidate, path))
            {
                return Ok(Some((playlist.id, index as i64)));
            }
        }

        Ok(None)
    }

    /// Zwraca `(id playlisty, indeks)` utworu, odkładając go na scenę, jeśli nigdzie go nie ma.
    fn locate_or_stage(&self, path: &str) -> Result<(String, i64), PlayerError> {
        if let Some(found) = self.find_in_playlists(path)? {
            return Ok(found);
        }

        let (playlist_id, item_count) = self.staging_playlist()?;

        self.post_json(
            &format!("/playlists/{playlist_id}/items/add"),
            &serde_json::json!({ "items": [path] }),
        )?;

        // Element dopisany na końcu playlisty — jego indeks to liczba elementów sprzed dodania.
        Ok((playlist_id, item_count))
    }

    /// Znajduje albo zakłada playlistę sceny. Zwraca jej id i liczbę elementów.
    fn staging_playlist(&self) -> Result<(String, i64), PlayerError> {
        let playlists = self.playlists()?;

        if let Some(found) = playlists.iter().find(|p| p.title == STAGING_PLAYLIST) {
            return Ok((found.id.clone(), found.item_count));
        }

        let created: RawPlaylist = self.post_json_created(
            "/playlists/add",
            &serde_json::json!({ "title": STAGING_PLAYLIST, "setCurrent": false }),
        )?;

        Ok((created.id, created.item_count))
    }

    /// `POST`, po którym chcemy dostać utworzony obiekt (beefweb zwraca go w ciele).
    fn post_json_created<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, PlayerError> {
        let response = self
            .agent
            .post(self.url(path))
            .send_json(body)
            .map_err(PlayerError::from_transport)?;

        let response = check_status(response)?;

        response
            .into_body()
            .read_json::<T>()
            .map_err(|error| PlayerError::Protocol(error.to_string()))
    }
}

impl PlayerAdapter for BeefwebAdapter {
    fn status(&self) -> Result<PlayerStatus, PlayerError> {
        let path = format!("/player?columns={STATE_COLUMNS}");
        let envelope: PlayerStateEnvelope = self.get_json(&path)?;
        let raw = envelope.player;

        // `index < 0` znaczy „nic nie jest aktywne” — beefweb zwraca wtedy puste kolumny.
        let active = raw.active_item.index >= 0;
        let column = |index: usize| {
            if active {
                raw.active_item
                    .columns
                    .get(index)
                    .map(|value| value.trim())
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            } else {
                None
            }
        };

        Ok(PlayerStatus {
            available: true,
            state: PlaybackState::from_wire(&raw.playback_state),
            title: column(0),
            artist: column(1),
            path: column(2),
            position_seconds: if active {
                raw.active_item.position
            } else {
                0.0
            },
            duration_seconds: if active {
                raw.active_item.duration
            } else {
                0.0
            },
            volume_db: clamp_db(raw.volume.value),
        })
    }

    fn play_next(&self, path: &str) -> Result<(), PlayerError> {
        let (playlist_id, index) = self.locate_or_stage(path)?;

        self.post_json(
            "/playqueue/add",
            &serde_json::json!({ "plref": playlist_id, "itemIndex": index }),
        )
    }

    fn play_now(&self, path: &str) -> Result<(), PlayerError> {
        let (playlist_id, index) = self.locate_or_stage(path)?;

        // Kolejka + „następny” działa też, gdy nic nie gra (sprawdzone na żywym foobarze):
        // odtwarzacz zjada element z kolejki i zostaje na playliście DJ-a, więc po zamówionym
        // utworze set gra dalej.
        self.post_json(
            "/playqueue/add",
            &serde_json::json!({ "plref": playlist_id, "itemIndex": index }),
        )?;

        self.post_empty("/player/next")
    }

    fn set_volume_db(&self, volume_db: f64) -> Result<(), PlayerError> {
        // Przycinamy u siebie: wysłanie wartości spoza skali beefweba skończyłoby się błędem
        // w środku sekwencji, a rampę da się policzyć i tak w tym zakresie.
        let volume_db = clamp_db(volume_db);

        self.post_json("/player", &serde_json::json!({ "volume": volume_db }))
    }

    fn set_stop_after_current(&self, stop: bool) -> Result<(), PlayerError> {
        self.post_json(
            "/player",
            &serde_json::json!({
                "options": [{ "id": "stopAfterCurrentTrack", "value": stop }]
            }),
        )
    }

    fn stop(&self) -> Result<(), PlayerError> {
        self.post_empty("/player/stop")
    }
}

impl PlayerError {
    /// Zamienia błąd transportu `ureq` na błąd odtwarzacza. Wszystko, co nie jest odpowiedzią
    /// serwera, znaczy dla nas tyle samo: „odtwarzacza nie ma”.
    fn from_transport(error: ureq::Error) -> Self {
        match error {
            ureq::Error::StatusCode(status) => Self::Rejected {
                status,
                message: String::new(),
            },
            other => Self::Unavailable(other.to_string()),
        }
    }
}

/// Sprawdza kod odpowiedzi i wyciąga treść błędu, gdy odtwarzacz odmówił.
fn check_status(
    mut response: ureq::http::Response<ureq::Body>,
) -> Result<ureq::http::Response<ureq::Body>, PlayerError> {
    let status = response.status();

    if status.is_success() {
        return Ok(response);
    }

    // Czytanie ciała może się nie udać — kod HTTP i tak wystarczy, żeby powiedzieć DJ-owi, co się stało.
    let message = response
        .body_mut()
        .read_to_string()
        .map(|body| body.trim().to_string())
        .unwrap_or_default();

    Err(PlayerError::Rejected {
        status: status.as_u16(),
        message,
    })
}

/// Porównuje ścieżki tak, jak robi to Windows: bez rozróżniania wielkości liter i z jednolitym
/// separatorem.
fn same_path(left: &str, right: &str) -> bool {
    let normalize = |value: &str| value.trim().replace('/', "\\").to_ascii_lowercase();

    normalize(left) == normalize(right)
}

#[derive(Debug, Deserialize)]
struct PlayerStateEnvelope {
    player: RawPlayerState,
}

#[derive(Debug, Deserialize)]
struct RawPlayerState {
    #[serde(rename = "activeItem")]
    active_item: RawActiveItem,
    #[serde(rename = "playbackState")]
    playback_state: String,
    #[serde(default)]
    volume: RawVolume,
}

#[derive(Debug, Default, Deserialize)]
struct RawVolume {
    #[serde(default)]
    value: f64,
}

#[derive(Debug, Deserialize)]
struct RawActiveItem {
    #[serde(default)]
    index: i64,
    #[serde(default)]
    position: f64,
    #[serde(default)]
    duration: f64,
    #[serde(default)]
    columns: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PlaylistsEnvelope {
    playlists: Vec<RawPlaylist>,
}

#[derive(Debug, Deserialize)]
struct RawPlaylist {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default, rename = "itemCount")]
    item_count: i64,
}

#[derive(Debug, Deserialize)]
struct PlaylistItemsEnvelope {
    #[serde(rename = "playlistItems")]
    playlist_items: RawPlaylistItems,
}

#[derive(Debug, Deserialize)]
struct RawPlaylistItems {
    #[serde(default)]
    items: Vec<RawPlaylistItem>,
}

#[derive(Debug, Deserialize)]
struct RawPlaylistItem {
    #[serde(default)]
    columns: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_playback_state_is_treated_as_stopped() {
        assert_eq!(PlaybackState::from_wire("playing"), PlaybackState::Playing);
        assert_eq!(PlaybackState::from_wire("paused"), PlaybackState::Paused);
        assert_eq!(PlaybackState::from_wire("stopped"), PlaybackState::Stopped);
        assert_eq!(PlaybackState::from_wire("changing"), PlaybackState::Stopped);
        assert_eq!(PlaybackState::from_wire(""), PlaybackState::Stopped);
    }

    #[test]
    fn offline_status_is_not_available_and_not_playing() {
        let status = PlayerStatus::offline();

        assert!(!status.available);
        assert_eq!(status.state, PlaybackState::Stopped);
        assert!(status.title.is_none());
        assert!(status.path.is_none());
    }

    #[test]
    fn windows_paths_are_compared_without_case_or_separator_noise() {
        assert!(same_path(
            "F:\\MP3\\Bitles - Hey Jude.mp3",
            "f:/mp3/bitles - hey jude.mp3"
        ));
        assert!(!same_path("F:\\MP3\\a.mp3", "F:\\MP3\\b.mp3"));
    }

    #[test]
    fn the_adapter_rejects_addresses_it_cannot_use() {
        assert!(BeefwebAdapter::new("localhost:8880").is_err());
        assert!(BeefwebAdapter::new("https://localhost:8880").is_err());
        assert!(BeefwebAdapter::new("http://").is_err());
        assert!(BeefwebAdapter::new("http://localhost:8880/").is_ok());
    }

    #[test]
    fn status_is_read_from_a_real_http_answer() {
        let body = r#"{"player":{"activeItem":{"columns":["TROUBLE","2 VIBEZ","F:\\MP3\\2 VIBEZ - TROUBLE.MP3"],"duration":198.6,"index":5,"playlistId":"p1","playlistIndex":0,"position":1.04},"playbackState":"playing","volume":{"isMuted":false,"max":0.0,"min":-100.0,"type":"db","value":-6.0}}}"#;
        let (base_url, handle) = serve_once(body, "200 OK");

        let adapter = BeefwebAdapter::new(&base_url).expect("adapter");
        let status = adapter.status().expect("stan odtwarzacza");

        assert!(status.available);
        assert_eq!(status.state, PlaybackState::Playing);
        assert_eq!(status.title.as_deref(), Some("TROUBLE"));
        assert_eq!(status.artist.as_deref(), Some("2 VIBEZ"));
        assert_eq!(
            status.path.as_deref(),
            Some("F:\\MP3\\2 VIBEZ - TROUBLE.MP3")
        );
        assert!((status.duration_seconds - 198.6).abs() < 0.001);
        assert!(
            (status.volume_db - (-6.0)).abs() < 0.001,
            "wolumen odtwarzacza jest w decybelach"
        );

        let request = handle.join().expect("wątek serwera");
        assert!(request.starts_with("GET /api/player?columns="), "{request}");
    }

    #[test]
    fn an_idle_player_has_no_track_and_is_stopped() {
        let body = r#"{"player":{"activeItem":{"columns":[],"duration":0.0,"index":-1,"playlistId":"","playlistIndex":-1,"position":0.0},"playbackState":"stopped"}}"#;
        let (base_url, handle) = serve_once(body, "200 OK");

        let adapter = BeefwebAdapter::new(&base_url).expect("adapter");
        let status = adapter.status().expect("stan odtwarzacza");

        assert!(status.available);
        assert_eq!(status.state, PlaybackState::Stopped);
        assert!(
            status.path.is_none(),
            "przy indeksie -1 nie ma aktywnego utworu"
        );
        assert_eq!(status.position_seconds, 0.0);

        handle.join().expect("wątek serwera");
    }

    #[test]
    fn a_refused_request_carries_the_player_message() {
        let body = r#"{"error":{"message":"item is not under allowed path"}}"#;
        let (base_url, handle) = serve_once(body, "403 Forbidden");

        let adapter = BeefwebAdapter::new(&base_url).expect("adapter");
        let error = adapter.status().expect_err("odmowa musi być błędem");

        match error {
            PlayerError::Rejected { status, message } => {
                assert_eq!(status, 403);
                assert!(message.contains("allowed path"), "{message}");
            }
            other => panic!("nieoczekiwany błąd: {other:?}"),
        }

        handle.join().expect("wątek serwera");
    }

    /// Test na **żywym** odtwarzaczu. Wymaga foobar2000 z beefwebem, więc nie chodzi w zwykłym
    /// przebiegu. Uruchamiaj świadomie:
    ///
    /// ```text
    /// ANON_DJ_PLAYER_URL=http://localhost:8880 cargo test -- --ignored status_from_a_live_player
    /// ```
    #[test]
    #[ignore = "wymaga uruchomionego foobar2000 z beefwebem"]
    fn status_from_a_live_player() {
        let base_url =
            std::env::var("ANON_DJ_PLAYER_URL").expect("ustaw ANON_DJ_PLAYER_URL na adres beefweb");

        let adapter = BeefwebAdapter::new(&base_url).expect("adapter");
        let status = adapter.status().expect("stan odtwarzacza");

        assert!(status.available, "beefweb powinien odpowiedzieć");
        println!("stan odtwarzacza: {status:?}");
    }

    /// Serwer HTTP odpowiadający na jedno żądanie. Zwraca bazowy adres i uchwyt wątku,
    /// który oddaje pierwszą linię żądania (do sprawdzenia, czy wołamy właściwy endpoint).
    fn serve_once(
        body: &'static str,
        status: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("port testowy");
        let address = listener.local_addr().expect("adres testowy");

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("połączenie testowe");

            let mut buffer = [0_u8; 2048];
            let read = stream.read(&mut buffer).expect("odczyt żądania");
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            let first_line = request.lines().next().unwrap_or_default().to_string();

            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("odpowiedź");

            first_line
        });

        (format!("http://{address}"), handle)
    }
}
