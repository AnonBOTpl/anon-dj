//! Lokalna baza SQLite aplikacji DJ-a.
//!
//! Schemat trzymamy w migracjach uruchamianych przy starcie (numer wersji w `PRAGMA user_version`).
//! Baza leży w katalogu danych aplikacji i nie zawiera plików muzycznych — tylko metadane i ścieżki
//! (PLAN.md, sekcja 9).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use protocol::{RequestStatus, TrackInfo};
use rusqlite::{Connection, OptionalExtension, params};

/// Migracje w kolejności stosowania. Każda następna podnosi `user_version` o jeden.
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_library.sql"),
    include_str!("../migrations/0003_review_queue.sql"),
];

/// Błąd warstwy bazy danych.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("błąd bazy danych: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("nie udało się utworzyć katalogu bazy {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("baza danych jest niedostępna po wcześniejszym błędzie w innym wątku")]
    Poisoned,

    #[error("nieznany status prośby zapisany w bazie: {0}")]
    UnknownStatus(String),

    #[error("prośba {0} nie istnieje")]
    RequestNotFound(i64),

    #[error("nie można zmienić statusu prośby {id} z „{from}” na „{to}”")]
    InvalidTransition {
        id: i64,
        from: &'static str,
        to: &'static str,
    },
}

/// Dokąd wysłać informację o zmianie statusu prośby: który kiosk ją przysłał i jak ją nazwał.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestTarget {
    pub kiosk_id: Option<i64>,
    pub kiosk_request_id: Option<u64>,
}

/// Baza SQLite aplikacji DJ-a. Jedno połączenie pod mutexem — operacje są krótkie,
/// a wywołania z UI idą przez `spawn_blocking`, więc nie blokują wątku interfejsu.
pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// Otwiera bazę pod podaną ścieżką, tworząc katalog, jeśli nie istnieje, i stosując migracje.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| DbError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let conn = Connection::open(path)?;

        Self::from_connection(conn)
    }

    /// Otwiera bazę w pamięci — używane wyłącznie w testach.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, DbError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, DbError> {
        // Wymuszamy klucze obce — dzięki temu literówka w track_id nie przejdzie po cichu.
        conn.pragma_update(None, "foreign_keys", "ON")?;
        apply_migrations(&conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Czyta ustawienie. Brak klucza to `None`, nie błąd.
    pub fn setting(&self, key: &str) -> Result<Option<String>, DbError> {
        let conn = self.lock()?;

        let value = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        Ok(value)
    }

    /// Zapisuje ustawienie (nadpisuje istniejące).
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), DbError> {
        let conn = self.lock()?;

        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;

        Ok(())
    }

    /// Dopisuje lub odświeża wpis o utworze. Kluczem jest ścieżka, więc ponowne skanowanie
    /// tego samego folderu nie tworzy duplikatów, a poprawione tagi trafiają do bazy.
    /// Data pierwszego dodania zostaje bez zmian.
    pub fn upsert_track(&self, track: &TrackRecord) -> Result<(), DbError> {
        let conn = self.lock()?;

        conn.execute(
            "INSERT INTO tracks (path, title, artist, album, duration_ms, added_at, title_fold, artist_fold)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(path) DO UPDATE SET
                 title = excluded.title,
                 artist = excluded.artist,
                 album = excluded.album,
                 duration_ms = excluded.duration_ms,
                 title_fold = excluded.title_fold,
                 artist_fold = excluded.artist_fold",
            params![
                track.path,
                track.title,
                track.artist,
                track.album,
                track.duration_ms,
                now_ms(),
                track.title_fold,
                track.artist_fold,
            ],
        )?;

        Ok(())
    }

    /// Liczba utworów w bibliotece.
    pub fn count_tracks(&self) -> Result<u64, DbError> {
        let conn = self.lock()?;

        let count: i64 = conn.query_row("SELECT count(*) FROM tracks", [], |row| row.get(0))?;

        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// Szuka utworów po tytule i wykonawcy. Puste zapytanie zwraca początek biblioteki —
    /// DJ chce czasem po prostu zobaczyć, co ma.
    pub fn search_tracks(&self, query: &str, limit: u32) -> Result<Vec<TrackInfo>, DbError> {
        let conn = self.lock()?;
        let pattern = format!("%{}%", escape_like(&fold_for_search(query.trim())));

        let mut statement = conn.prepare(
            "SELECT id, title, artist, album, duration_ms FROM tracks
             WHERE title_fold LIKE ?1 ESCAPE '!' OR artist_fold LIKE ?1 ESCAPE '!'
             ORDER BY artist_fold, title_fold
             LIMIT ?2",
        )?;

        let rows = statement.query_map(params![pattern, limit], |row| {
            Ok(TrackInfo {
                id: row.get(0)?,
                title: row.get(1)?,
                artist: row.get(2)?,
                album: row.get(3)?,
                duration_ms: row
                    .get::<_, Option<i64>>(4)?
                    .and_then(|milliseconds| u32::try_from(milliseconds).ok()),
            })
        })?;

        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Foldery wybrane przez DJ-a do skanowania, w kolejności alfabetycznej.
    pub fn library_folders(&self) -> Result<Vec<LibraryFolder>, DbError> {
        let conn = self.lock()?;

        let mut statement = conn.prepare("SELECT id, path FROM library_folders ORDER BY path")?;
        let rows = statement.query_map([], |row| {
            Ok(LibraryFolder {
                id: row.get(0)?,
                path: row.get(1)?,
            })
        })?;

        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Dodaje folder do biblioteki. Zwraca `false`, jeśli był już na liście.
    pub fn add_library_folder(&self, path: &str) -> Result<bool, DbError> {
        let conn = self.lock()?;

        let inserted = conn.execute(
            "INSERT INTO library_folders (path, added_at) VALUES (?1, ?2)
             ON CONFLICT(path) DO NOTHING",
            params![path, now_ms()],
        )?;

        Ok(inserted > 0)
    }

    /// Usuwa folder z listy i zwraca jego ścieżkę — potrzebną, żeby zabrać z biblioteki
    /// także utwory z tego folderu.
    pub fn remove_library_folder(&self, id: i64) -> Result<Option<String>, DbError> {
        let conn = self.lock()?;

        let path = conn
            .query_row(
                "SELECT path FROM library_folders WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        if path.is_some() {
            conn.execute("DELETE FROM library_folders WHERE id = ?1", params![id])?;
        }

        Ok(path)
    }

    /// Usuwa utwory leżące w podanym folderze. Separator dokładamy sami, żeby usunięcie
    /// `C:\Muzyka` nie zabrało przypadkiem zawartości `C:\Muzyka2`.
    pub fn delete_tracks_under(&self, folder: &str) -> Result<u64, DbError> {
        let conn = self.lock()?;
        let pattern = format!("{}%", escape_like(&folder_with_separator(folder)));

        let deleted = conn.execute(
            "DELETE FROM tracks WHERE path LIKE ?1 ESCAPE '!'",
            params![pattern],
        )?;

        Ok(u64::try_from(deleted).unwrap_or(0))
    }

    // --- Kioski i prośby gości (PLAN.md, sekcje 5 i 6) ---

    /// Zwraca identyfikator kiosku o podanej nazwie, zakładając wpis przy pierwszym parowaniu.
    ///
    /// Dzięki temu ten sam kiosk po ponownym połączeniu (albo po restarcie aplikacji) dostaje
    /// ten sam identyfikator, a limity tempa i kolejka nie mnożą się przy każdym łączeniu.
    pub fn kiosk_by_name(&self, name: &str) -> Result<i64, DbError> {
        let conn = self.lock()?;
        let seen_at = now_ms();

        let existing = conn
            .query_row(
                "SELECT id FROM kiosks WHERE name = ?1",
                params![name],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;

        let id = match existing {
            Some(id) => id,
            None => conn.query_row(
                "INSERT INTO kiosks (name, token, last_seen_at) VALUES (?1, ?2, ?3) RETURNING id",
                params![name, generate_kiosk_token(), seen_at],
                |row| row.get::<_, i64>(0),
            )?,
        };

        conn.execute(
            "UPDATE kiosks SET last_seen_at = ?1 WHERE id = ?2",
            params![seen_at, id],
        )?;

        Ok(id)
    }

    /// Czy utwór o tym identyfikatorze jest w bibliotece. Chroni przed prośbą o nieistniejący plik.
    pub fn track_exists(&self, track_id: i64) -> Result<bool, DbError> {
        let conn = self.lock()?;

        let count: i64 = conn.query_row(
            "SELECT count(*) FROM tracks WHERE id = ?1",
            params![track_id],
            |row| row.get(0),
        )?;

        Ok(count > 0)
    }

    /// Zapisuje prośbę gościa w kolejce przeglądu (status `submitted`).
    /// Zwraca identyfikator prośby i czas jej powstania.
    ///
    /// `kiosk_request_id` to identyfikator nadany przez kiosk — trzymamy go tylko po to, żeby
    /// umieć powiedzieć temu kioskowi, co DJ zrobił z jego dedykacją.
    pub fn insert_request(
        &self,
        track_id: i64,
        dedication: &str,
        guest_name: Option<&str>,
        kiosk_id: Option<i64>,
        kiosk_request_id: Option<u64>,
    ) -> Result<(i64, i64), DbError> {
        let conn = self.lock()?;
        let created_at = now_ms();
        let kiosk_request_id = kiosk_request_id.and_then(|id| i64::try_from(id).ok());

        let id = conn.query_row(
            "INSERT INTO requests
                 (track_id, dedication_original, guest_name, kiosk_id, kiosk_request_id,
                  status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             RETURNING id",
            params![
                track_id,
                dedication,
                guest_name,
                kiosk_id,
                kiosk_request_id,
                RequestStatus::Submitted.as_str(),
                created_at
            ],
            |row| row.get::<_, i64>(0),
        )?;

        Ok((id, created_at))
    }

    /// Czy ten sam kiosk przysłał już identyczną prośbę od czasu `since_ms`.
    /// Klient nie może „podwójnym kliknięciem” wepchnąć dedykacji dwa razy na antenę.
    pub fn duplicate_request_exists(
        &self,
        kiosk_id: i64,
        track_id: i64,
        dedication: &str,
        since_ms: i64,
    ) -> Result<bool, DbError> {
        let conn = self.lock()?;

        let count: i64 = conn.query_row(
            "SELECT count(*) FROM requests
             WHERE kiosk_id = ?1 AND track_id = ?2 AND dedication_original = ?3 AND created_at >= ?4",
            params![kiosk_id, track_id, dedication, since_ms],
            |row| row.get(0),
        )?;

        Ok(count > 0)
    }

    /// Prośby o podanym statusie, najstarsze pierwsze — to jest kolejka widziana przez DJ-a.
    pub fn requests_with_status(
        &self,
        status: RequestStatus,
        limit: u32,
    ) -> Result<Vec<QueuedRequest>, DbError> {
        let conn = self.lock()?;

        query_requests(
            &conn,
            &format!(
                "SELECT {QUEUE_COLUMNS} FROM requests r JOIN tracks t ON t.id = r.track_id
                 WHERE r.status = ?1
                 ORDER BY r.created_at
                 LIMIT ?2"
            ),
            params![status.as_str(), limit],
        )
    }

    /// Kolejka „gotowe do wykonania” w kolejności ustalonej przez DJ-a.
    pub fn ready_requests(&self, limit: u32) -> Result<Vec<QueuedRequest>, DbError> {
        let conn = self.lock()?;

        query_requests(
            &conn,
            &format!(
                "SELECT {QUEUE_COLUMNS} FROM requests r JOIN tracks t ON t.id = r.track_id
                 WHERE r.status = ?1
                 ORDER BY r.position, r.created_at
                 LIMIT ?2"
            ),
            params![RequestStatus::Approved.as_str(), limit],
        )
    }

    /// Historia: wykonane i odrzucone, ostatnio zmienione pierwsze.
    pub fn request_history(&self, limit: u32) -> Result<Vec<QueuedRequest>, DbError> {
        let conn = self.lock()?;

        query_requests(
            &conn,
            &format!(
                "SELECT {QUEUE_COLUMNS} FROM requests r JOIN tracks t ON t.id = r.track_id
                 WHERE r.status IN (?1, ?2)
                 ORDER BY r.updated_at DESC
                 LIMIT ?3"
            ),
            params![
                RequestStatus::Done.as_str(),
                RequestStatus::Rejected.as_str(),
                limit
            ],
        )
    }

    /// Zatwierdza prośbę z kolejki przeglądu i stawia ją na końcu kolejki gotowych.
    ///
    /// Zatwierdzić można wyłącznie prośbę czekającą na przegląd — ponowne zatwierdzenie
    /// przesunęłoby ją na koniec kolejki, o co DJ nie prosił.
    pub fn approve_request(&self, id: i64) -> Result<(), DbError> {
        let conn = self.lock()?;

        match current_status(&conn, id)? {
            Some(RequestStatus::Submitted) => {}
            Some(other) => return Err(invalid_transition(id, other, RequestStatus::Approved)),
            None => return Err(DbError::RequestNotFound(id)),
        }

        // Nowa prośba ląduje na końcu kolejki gotowych — DJ może ją potem przesunąć.
        let position: i64 = conn.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM requests WHERE status = ?1",
            params![RequestStatus::Approved.as_str()],
            |row| row.get(0),
        )?;

        conn.execute(
            "UPDATE requests SET status = ?2, position = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, RequestStatus::Approved.as_str(), position, now_ms()],
        )?;

        Ok(())
    }

    /// Odrzuca prośbę — zarówno czekającą na przegląd, jak i zatwierdzoną.
    pub fn reject_request(&self, id: i64) -> Result<(), DbError> {
        let conn = self.lock()?;

        match current_status(&conn, id)? {
            Some(RequestStatus::Submitted | RequestStatus::Approved) => {}
            Some(other) => return Err(invalid_transition(id, other, RequestStatus::Rejected)),
            None => return Err(DbError::RequestNotFound(id)),
        }

        conn.execute(
            "UPDATE requests SET status = ?2, position = NULL, updated_at = ?3 WHERE id = ?1",
            params![id, RequestStatus::Rejected.as_str(), now_ms()],
        )?;

        Ok(())
    }

    /// Zapisuje dedykację po edycji DJ-a. Tekst oryginalny zostaje w bazie — historia ma sens
    /// tylko wtedy, gdy widać, co napisał gość (PLAN.md, sekcja 6).
    pub fn set_dedication(&self, id: i64, dedication: &str) -> Result<(), DbError> {
        let conn = self.lock()?;

        let updated = conn.execute(
            "UPDATE requests SET dedication_edited = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, dedication, now_ms()],
        )?;

        if updated == 0 {
            return Err(DbError::RequestNotFound(id));
        }

        Ok(())
    }

    /// Przesuwa prośbę w kolejce gotowych o jedno miejsce. `false` oznacza, że nie ma dokąd —
    /// prośba stoi już na początku albo na końcu.
    pub fn move_request(&self, id: i64, up: bool) -> Result<bool, DbError> {
        let mut conn = self.lock()?;

        let position: Option<i64> = conn
            .query_row(
                "SELECT position FROM requests WHERE id = ?1 AND status = ?2",
                params![id, RequestStatus::Approved.as_str()],
                |row| row.get(0),
            )
            .optional()?;

        let Some(position) = position else {
            return Ok(false);
        };

        // Sąsiad to najbliższa prośba po właściwej stronie — z nią wymieniamy miejsca.
        let neighbour = if up {
            conn.query_row(
                "SELECT id, position FROM requests
                 WHERE status = ?1 AND position < ?2
                 ORDER BY position DESC
                 LIMIT 1",
                params![RequestStatus::Approved.as_str(), position],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?
        } else {
            conn.query_row(
                "SELECT id, position FROM requests
                 WHERE status = ?1 AND position > ?2
                 ORDER BY position ASC
                 LIMIT 1",
                params![RequestStatus::Approved.as_str(), position],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?
        };

        let Some((neighbour_id, neighbour_position)) = neighbour else {
            return Ok(false);
        };

        // Zamiana miejsc musi być atomowa — inaczej przerwany zapis zostawiłby dwie prośby
        // z tą samą pozycją.
        let transaction = conn.transaction()?;
        transaction.execute(
            "UPDATE requests SET position = ?2 WHERE id = ?1",
            params![id, neighbour_position],
        )?;
        transaction.execute(
            "UPDATE requests SET position = ?2 WHERE id = ?1",
            params![neighbour_id, position],
        )?;
        transaction.commit()?;

        Ok(true)
    }

    /// Dokąd wysłać zmianę statusu prośby. `None` oznacza, że prośby nie ma w bazie.
    pub fn request_target(&self, id: i64) -> Result<Option<RequestTarget>, DbError> {
        let conn = self.lock()?;

        let target = conn
            .query_row(
                "SELECT kiosk_id, kiosk_request_id FROM requests WHERE id = ?1",
                params![id],
                |row| {
                    Ok(RequestTarget {
                        kiosk_id: row.get(0)?,
                        kiosk_request_id: row
                            .get::<_, Option<i64>>(1)?
                            .and_then(|value| u64::try_from(value).ok()),
                    })
                },
            )
            .optional()?;

        Ok(target)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, DbError> {
        // Zatruty mutex oznacza panikę w innym wątku — traktujemy to jako błąd bazy,
        // nigdy nie panikujemy w kodzie aplikacji.
        self.conn.lock().map_err(|_| DbError::Poisoned)
    }
}

/// Kolumny jednego wiersza kolejki — w jednym miejscu, żeby trzy kolejki nie rozjechały się
/// z tabelami po cichu.
const QUEUE_COLUMNS: &str = "r.id, r.track_id, t.title, t.artist,
     COALESCE(r.dedication_edited, r.dedication_original), r.guest_name, r.status,
     r.created_at, r.updated_at";

/// Wiersz kolejki odczytany z bazy, jeszcze przed tłumaczeniem statusu.
struct QueuedRequestRow {
    id: i64,
    track_id: i64,
    title: String,
    artist: String,
    dedication: String,
    guest_name: Option<String>,
    status: String,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<QueuedRequestRow> for QueuedRequest {
    type Error = DbError;

    fn try_from(row: QueuedRequestRow) -> Result<Self, Self::Error> {
        let status = RequestStatus::parse(&row.status)
            .ok_or_else(|| DbError::UnknownStatus(row.status.clone()))?;

        Ok(Self {
            id: row.id,
            track_id: row.track_id,
            title: row.title,
            artist: row.artist,
            dedication: row.dedication,
            guest_name: row.guest_name,
            status,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// Wykonuje zapytanie zwracające wiersze kolejki (zawsze z [`QUEUE_COLUMNS`]).
fn query_requests(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<QueuedRequest>, DbError> {
    let mut statement = conn.prepare(sql)?;

    let rows = statement.query_map(params, |row| {
        Ok(QueuedRequestRow {
            id: row.get(0)?,
            track_id: row.get(1)?,
            title: row.get(2)?,
            artist: row.get(3)?,
            dedication: row.get(4)?,
            guest_name: row.get(5)?,
            status: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    })?;

    let mut requests = Vec::new();

    for row in rows {
        requests.push(QueuedRequest::try_from(row?)?);
    }

    Ok(requests)
}

/// Bieżący status prośby; `None` oznacza, że prośby nie ma w bazie.
fn current_status(conn: &Connection, id: i64) -> Result<Option<RequestStatus>, DbError> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT status FROM requests WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;

    match stored {
        None => Ok(None),
        Some(status) => RequestStatus::parse(&status)
            .map(Some)
            .ok_or(DbError::UnknownStatus(status)),
    }
}

/// Błąd przejścia, którego kolejki nie dopuszczają.
fn invalid_transition(id: i64, from: RequestStatus, to: RequestStatus) -> DbError {
    DbError::InvalidTransition {
        id,
        from: from.as_str(),
        to: to.as_str(),
    }
}

/// Wpis o utworze przygotowany do zapisu w bazie.
///
/// Postacie tekstu używane przy wyszukiwaniu (`title_fold`, `artist_fold`) liczymy już przy
/// tworzeniu rekordu, żeby żaden zapis nie przeszedł z pominięciem normalizacji.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackRecord {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    title_fold: String,
    artist_fold: String,
}

impl TrackRecord {
    /// Buduje rekord utworu.
    pub fn new(
        path: String,
        title: String,
        artist: String,
        album: Option<String>,
        duration_ms: Option<i64>,
    ) -> Self {
        let title_fold = fold_for_search(&title);
        let artist_fold = fold_for_search(&artist);

        Self {
            path,
            title,
            artist,
            album,
            duration_ms,
            title_fold,
            artist_fold,
        }
    }
}

/// Prośba gościa widziana przez DJ-a: treść dedykacji razem z metadanymi utworu.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct QueuedRequest {
    pub id: i64,
    pub track_id: i64,
    pub title: String,
    pub artist: String,
    /// Tekst po ewentualnej edycji DJ-a; dopóki jej nie ma — tekst oryginalny.
    pub dedication: String,
    pub guest_name: Option<String>,
    pub status: RequestStatus,
    pub created_at: i64,
    /// Czas ostatniej zmiany — po nim sortujemy historię.
    pub updated_at: i64,
}

/// Folder wybrany przez DJ-a do skanowania.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LibraryFolder {
    pub id: i64,
    pub path: String,
}

/// Postać tekstu używana przy wyszukiwaniu.
///
/// SQLite porównuje `LIKE` bez rozróżniania wielkości liter **tylko dla ASCII** — dla niego
/// „Ł” i „ł” to dwa różne znaki. Normalizujemy więc tekst po stronie Rusta (Unicode-aware
/// `to_lowercase`) i zapisujemy gotową postać w osobnej kolumnie.
pub fn fold_for_search(value: &str) -> String {
    value.to_lowercase()
}

/// Escapuje znaki specjalne `LIKE`, żeby `%` albo `_` z tytułu nie zamieniły się w wieloznacznik.
fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());

    for character in value.chars() {
        if matches!(character, '!' | '%' | '_') {
            escaped.push('!');
        }

        escaped.push(character);
    }

    escaped
}

/// Ścieżka folderu zakończona separatorem — wzorzec dla `LIKE`.
fn folder_with_separator(folder: &str) -> String {
    let trimmed = folder.trim_end_matches(['\\', '/']);

    if trimmed.is_empty() {
        return folder.to_string();
    }

    format!("{trimmed}\\")
}

/// Token kiosku: stabilny identyfikator zapisywany przy pierwszym parowaniu.
///
/// To **nie jest** sekret — o dostępie decyduje PIN z ustawień. Losowość bierzemy z `RandomState`
/// (ziarno z systemu, inne dla każdej instancji), żeby nie dokładać zależności tylko po to,
/// by wypełnić kolumnę `UNIQUE NOT NULL`.
fn generate_kiosk_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let mut first = RandomState::new().build_hasher();
    first.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    first.write_u64(now_ms() as u64);

    let mut second = RandomState::new().build_hasher();
    second.write_u64(u64::from(std::process::id()));

    format!("{:016x}{:016x}", first.finish(), second.finish())
}

/// Znacznik czasu w milisekundach od epoki — format zapisu czasu w schemacie.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn apply_migrations(conn: &Connection) -> Result<(), DbError> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    for (index, migration) in MIGRATIONS
        .iter()
        .enumerate()
        .skip(usize::try_from(current).unwrap_or(usize::MAX))
    {
        conn.execute_batch(migration)?;
        conn.pragma_update(None, "user_version", (index + 1) as i64)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(db: &Db, table: &str) -> Vec<String> {
        let conn = db.lock().expect("blokada bazy");

        let mut statement = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("zapytanie o kolumny");
        let rows = statement
            .query_map([], |row| row.get::<_, String>(1))
            .expect("kolumny tabeli");

        rows.collect::<Result<Vec<_>, _>>().expect("odczyt kolumn")
    }

    fn table_exists(db: &Db, name: &str) -> bool {
        let conn = db.lock().expect("blokada bazy");

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![name],
                |row| row.get(0),
            )
            .expect("zapytanie o schemat");

        count == 1
    }

    #[test]
    fn migrations_create_all_tables() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        for table in [
            "tracks",
            "requests",
            "kiosks",
            "settings",
            "library_folders",
        ] {
            assert!(table_exists(&db, table), "brak tabeli {table}");
        }

        let track_columns = columns(&db, "tracks");

        for column in ["title_fold", "artist_fold"] {
            assert!(
                track_columns.iter().any(|name| name == column),
                "brak kolumny {column} potrzebnej do wyszukiwania"
            );
        }

        let request_columns = columns(&db, "requests");

        for column in ["position", "kiosk_request_id"] {
            assert!(
                request_columns.iter().any(|name| name == column),
                "brak kolumny {column} potrzebnej kolejkom"
            );
        }
    }

    #[test]
    fn migrations_are_idempotent() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        let conn = db.lock().expect("blokada bazy");
        apply_migrations(&conn).expect("powtórne migracje");
        apply_migrations(&conn).expect("kolejne migracje");

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("wersja schematu");
        drop(conn);

        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    #[test]
    fn settings_round_trip_and_overwrite() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        assert_eq!(db.setting("tts.voice").expect("odczyt"), None);

        db.set_setting("tts.voice", "justyna").expect("zapis");
        assert_eq!(
            db.setting("tts.voice").expect("odczyt"),
            Some("justyna".to_string())
        );

        db.set_setting("tts.voice", "jarvis").expect("nadpisanie");
        assert_eq!(
            db.setting("tts.voice").expect("odczyt"),
            Some("jarvis".to_string())
        );
    }

    #[test]
    fn requests_require_an_existing_track() {
        let db = Db::open_in_memory().expect("baza w pamięci");
        let conn = db.lock().expect("blokada bazy");

        let result = conn.execute(
            "INSERT INTO requests (track_id, dedication_original, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![999_i64, "Sto lat!", "submitted", now_ms(), now_ms()],
        );

        assert!(
            result.is_err(),
            "klucz obcy powinien odrzucić nieznany utwór"
        );
    }

    #[test]
    fn request_can_be_stored_for_an_existing_track() {
        let db = Db::open_in_memory().expect("baza w pamięci");
        let conn = db.lock().expect("blokada bazy");

        conn.execute(
            "INSERT INTO tracks (path, title, artist, added_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                "C:\\muzyka\\kombi.mp3",
                "Słodkiego miłego życia",
                "Kombi",
                now_ms()
            ],
        )
        .expect("dodanie utworu");

        conn.execute(
            "INSERT INTO requests (track_id, dedication_original, status, created_at, updated_at)
             VALUES (1, ?1, ?2, ?3, ?4)",
            params!["Dla Kasi i Marka", "submitted", now_ms(), now_ms()],
        )
        .expect("dodanie prośby");

        let status: String = conn
            .query_row("SELECT status FROM requests WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("odczyt prośby");

        assert_eq!(status, "submitted");
    }

    #[test]
    fn music_files_are_never_copied_only_referenced() {
        let db = Db::open_in_memory().expect("baza w pamięci");
        let conn = db.lock().expect("blokada bazy");

        conn.execute(
            "INSERT INTO tracks (path, title, artist, added_at) VALUES (?1, ?2, ?3, ?4)",
            params!["D:\\muzyka\\utwor.mp3", "Tytuł", "Wykonawca", now_ms()],
        )
        .expect("dodanie utworu");

        let path: String = conn
            .query_row("SELECT path FROM tracks WHERE id = 1", [], |row| row.get(0))
            .expect("odczyt ścieżki");

        assert_eq!(path, "D:\\muzyka\\utwor.mp3");
    }

    fn store_track(db: &Db, path: &str, title: &str, artist: &str) {
        let record = TrackRecord::new(
            path.to_string(),
            title.to_string(),
            artist.to_string(),
            None,
            None,
        );

        db.upsert_track(&record).expect("zapis utworu");
    }

    #[test]
    fn upsert_keeps_one_row_per_path_and_refreshes_metadata() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        store_track(&db, "C:\\muzyka\\a.mp3", "Stary tytuł", "Kombi");
        store_track(&db, "C:\\muzyka\\a.mp3", "Nowy tytuł", "Kombi");

        assert_eq!(db.count_tracks().expect("liczba utworów"), 1);

        let found = db.search_tracks("nowy", 50).expect("wyszukiwanie");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Nowy tytuł");
    }

    #[test]
    fn search_ignores_letter_case_including_polish_letters() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        store_track(
            &db,
            "C:\\muzyka\\kombi.mp3",
            "Słodkiego, miłego życia",
            "Kombi",
        );

        assert_eq!(
            db.search_tracks("słodkiego", 50).expect("szukanie").len(),
            1
        );
        assert_eq!(
            db.search_tracks("SŁODKIEGO", 50).expect("szukanie").len(),
            1
        );
        assert_eq!(db.search_tracks("kombi", 50).expect("szukanie").len(), 1);
        assert!(
            db.search_tracks("nirvana", 50)
                .expect("szukanie")
                .is_empty()
        );
    }

    #[test]
    fn search_treats_wildcards_as_ordinary_text() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        store_track(&db, "C:\\muzyka\\procent.mp3", "Dysk 100%", "Zespół");
        store_track(&db, "C:\\muzyka\\inny.mp3", "Coś innego", "Zespół");

        assert_eq!(db.search_tracks("100%", 50).expect("szukanie").len(), 1);
        assert_eq!(
            db.search_tracks("%", 50).expect("szukanie").len(),
            1,
            "`%` w zapytaniu ma być zwykłym znakiem, a nie „wszystko”"
        );
    }

    #[test]
    fn empty_query_lists_the_library_and_respects_the_limit() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        store_track(&db, "C:\\muzyka\\1.mp3", "Alfa", "Zespół");
        store_track(&db, "C:\\muzyka\\2.mp3", "Beta", "Zespół");
        store_track(&db, "C:\\muzyka\\3.mp3", "Gamma", "Zespół");

        assert_eq!(db.search_tracks("", 50).expect("szukanie").len(), 3);
        assert_eq!(db.search_tracks("", 2).expect("szukanie").len(), 2);
    }

    #[test]
    fn folders_are_unique_and_can_be_removed() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        assert!(db.add_library_folder("C:\\Muzyka").expect("dodanie"));
        assert!(
            !db.add_library_folder("C:\\Muzyka")
                .expect("powtórzone dodanie"),
            "ten sam folder nie może wejść dwa razy"
        );

        let folders = db.library_folders().expect("lista folderów");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].path, "C:\\Muzyka");

        let removed = db
            .remove_library_folder(folders[0].id)
            .expect("usuwanie folderu");

        assert_eq!(removed, Some("C:\\Muzyka".to_string()));
        assert!(db.library_folders().expect("lista folderów").is_empty());
        assert_eq!(
            db.remove_library_folder(999)
                .expect("usuwanie nieistniejącego"),
            None
        );
    }

    #[test]
    fn removing_a_folder_deletes_only_its_tracks() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        store_track(&db, "C:\\Muzyka\\a.mp3", "Alfa", "Zespół");
        store_track(&db, "C:\\Muzyka\\Podfolder\\b.mp3", "Beta", "Zespół");
        store_track(&db, "C:\\Muzyka2\\c.mp3", "Gamma", "Zespół");

        let deleted = db.delete_tracks_under("C:\\Muzyka").expect("usuwanie");

        assert_eq!(deleted, 2);
        assert_eq!(db.count_tracks().expect("liczba utworów"), 1);
        assert_eq!(db.search_tracks("gamma", 50).expect("szukanie").len(), 1);
    }

    #[test]
    fn a_kiosk_keeps_its_identifier_across_pairings() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        let first = db.kiosk_by_name("Kiosk przy wejściu").expect("parowanie");
        let again = db
            .kiosk_by_name("Kiosk przy wejściu")
            .expect("ponowne parowanie");
        let other = db.kiosk_by_name("Kiosk w sali").expect("drugi kiosk");

        assert_eq!(first, again, "ten sam kiosk nie może mnożyć wpisów");
        assert_ne!(first, other);

        let conn = db.lock().expect("blokada bazy");
        let mut statement = conn.prepare("SELECT token FROM kiosks").expect("zapytanie");
        let tokens: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .expect("tokeny")
            .collect::<Result<_, _>>()
            .expect("odczyt tokenów");
        drop(statement);
        drop(conn);

        assert_eq!(tokens.len(), 2);
        assert_ne!(tokens[0], tokens[1], "tokeny muszą się różnić");
    }

    #[test]
    fn requests_land_in_the_queue_with_track_metadata() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        store_track(
            &db,
            "C:\\muzyka\\kombi.mp3",
            "Słodkiego, miłego życia",
            "Kombi",
        );

        let kiosk = db.kiosk_by_name("Kiosk 1").expect("parowanie");
        let track_id = db.search_tracks("kombi", 10).expect("szukanie")[0].id;

        db.insert_request(track_id, "Dla Kasi", Some("Ania"), Some(kiosk), Some(11))
            .expect("zapis prośby");

        let queue = db
            .requests_with_status(RequestStatus::Submitted, 10)
            .expect("kolejka");

        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].dedication, "Dla Kasi");
        assert_eq!(queue[0].guest_name, Some("Ania".to_string()));
        assert_eq!(queue[0].title, "Słodkiego, miłego życia");
        assert_eq!(queue[0].artist, "Kombi");
        assert_eq!(queue[0].status, RequestStatus::Submitted);

        assert!(
            db.requests_with_status(RequestStatus::Done, 10)
                .expect("kolejka")
                .is_empty()
        );
    }

    #[test]
    fn an_unknown_track_cannot_be_requested() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        assert!(!db.track_exists(999).expect("sprawdzenie utworu"));
        assert!(
            db.insert_request(999, "Dla Kasi", None, None, None)
                .is_err(),
            "klucz obcy musi odrzucić prośbę o nieistniejący utwór"
        );
    }

    #[test]
    fn the_same_kiosk_cannot_send_the_same_request_twice() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        store_track(&db, "C:\\muzyka\\a.mp3", "Alfa", "Zespół");

        let kiosk = db.kiosk_by_name("Kiosk 1").expect("parowanie");
        let now = now_ms();

        db.insert_request(1, "Sto lat!", None, Some(kiosk), Some(1))
            .expect("zapis prośby");

        assert!(
            db.duplicate_request_exists(kiosk, 1, "Sto lat!", now - 1_000)
                .expect("sprawdzenie duplikatu"),
            "ta sama prośba w oknie czasowym to duplikat"
        );
        assert!(
            !db.duplicate_request_exists(kiosk, 1, "Sto lat!", now + 1_000)
                .expect("sprawdzenie duplikatu"),
            "wpis z przed okna nie jest duplikatem"
        );
        assert!(
            !db.duplicate_request_exists(kiosk, 1, "Inny tekst", now - 1_000)
                .expect("sprawdzenie duplikatu"),
            "inna treść to nie duplikat"
        );
    }

    /// Zakłada utwór i jedną prośbę, zwracając jej identyfikator — wspólny początek testów kolejek.
    fn a_request(db: &Db, dedication: &str, kiosk_request_id: Option<u64>) -> i64 {
        if db.count_tracks().expect("liczba utworów") == 0 {
            store_track(db, "C:\\muzyka\\a.mp3", "Alfa", "Zespół");
        }

        let track_id = db.search_tracks("alfa", 10).expect("szukanie")[0].id;

        db.insert_request(track_id, dedication, None, None, kiosk_request_id)
            .expect("zapis prośby")
            .0
    }

    #[test]
    fn approving_moves_a_request_to_the_ready_queue() {
        let db = Db::open_in_memory().expect("baza w pamięci");
        let request = a_request(&db, "Dla Kasi i Marka", Some(5));

        db.approve_request(request).expect("zatwierdzenie");

        assert!(
            db.requests_with_status(RequestStatus::Submitted, 10)
                .expect("kolejka przeglądu")
                .is_empty(),
            "zatwierdzona prośba nie może zostać w kolejce przeglądu"
        );

        let ready = db.ready_requests(10).expect("kolejka gotowych");

        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, request);
        assert_eq!(ready[0].status, RequestStatus::Approved);
        assert_eq!(ready[0].dedication, "Dla Kasi i Marka");
        assert_eq!(ready[0].title, "Alfa");

        let target = db
            .request_target(request)
            .expect("adres prośby")
            .expect("prośba istnieje");

        assert_eq!(target.kiosk_request_id, Some(5));
        assert_eq!(
            target.kiosk_id, None,
            "prośba przyszła z interfejsu, nie z kiosku"
        );
    }

    #[test]
    fn a_request_can_only_be_approved_once() {
        let db = Db::open_in_memory().expect("baza w pamięci");
        let request = a_request(&db, "Sto lat!", None);

        db.approve_request(request).expect("pierwsze zatwierdzenie");

        assert!(
            matches!(
                db.approve_request(request),
                Err(DbError::InvalidTransition { .. })
            ),
            "drugie zatwierdzenie przesunęłoby prośbę na koniec kolejki"
        );
    }

    #[test]
    fn rejecting_works_from_both_queues_and_lands_in_history() {
        let db = Db::open_in_memory().expect("baza w pamięci");
        let from_review = a_request(&db, "Pierwsza", None);
        let from_ready = a_request(&db, "Druga", None);

        db.approve_request(from_ready).expect("zatwierdzenie");
        db.reject_request(from_review)
            .expect("odrzucenie z przeglądu");
        db.reject_request(from_ready)
            .expect("odrzucenie z gotowych");

        assert!(
            db.requests_with_status(RequestStatus::Submitted, 10)
                .expect("kolejka przeglądu")
                .is_empty()
        );
        assert!(
            db.ready_requests(10).expect("kolejka gotowych").is_empty(),
            "odrzucona prośba nie może czekać na wykonanie"
        );

        let history = db.request_history(10).expect("historia");

        assert_eq!(history.len(), 2);
        assert!(
            history
                .iter()
                .all(|entry| entry.status == RequestStatus::Rejected)
        );
    }

    #[test]
    fn history_shows_done_and_rejected_newest_first() {
        let db = Db::open_in_memory().expect("baza w pamięci");
        let older = a_request(&db, "Starsza", None);
        let newer = a_request(&db, "Nowsza", None);

        db.approve_request(older).expect("zatwierdzenie");
        db.reject_request(newer).expect("odrzucenie");

        // Czasy ustawiamy wprost — `now_ms()` potrafi zwrócić tę samą milisekundę dla obu zmian.
        let conn = db.lock().expect("blokada bazy");
        conn.execute(
            "UPDATE requests SET updated_at = 1_000 WHERE id = ?1",
            params![older],
        )
        .expect("czas starszej prośby");
        conn.execute(
            "UPDATE requests SET status = 'done', updated_at = 2_000 WHERE id = ?1",
            params![older],
        )
        .expect("wykonana prośba");
        conn.execute(
            "UPDATE requests SET updated_at = 3_000 WHERE id = ?1",
            params![newer],
        )
        .expect("czas nowszej prośby");
        drop(conn);

        let history = db.request_history(10).expect("historia");

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].id, newer, "ostatnia zmiana jest pierwsza");
        assert_eq!(history[0].status, RequestStatus::Rejected);
        assert_eq!(history[1].id, older);
        assert_eq!(history[1].status, RequestStatus::Done);
    }

    #[test]
    fn editing_a_dedication_keeps_the_original_text() {
        let db = Db::open_in_memory().expect("baza w pamięci");
        let request = a_request(&db, "dla kasi", None);

        db.set_dedication(request, "Dla Kasi i Marka — sto lat!")
            .expect("edycja dedykacji");

        let queue = db
            .requests_with_status(RequestStatus::Submitted, 10)
            .expect("kolejka");

        assert_eq!(queue[0].dedication, "Dla Kasi i Marka — sto lat!");

        let conn = db.lock().expect("blokada bazy");
        let original: String = conn
            .query_row(
                "SELECT dedication_original FROM requests WHERE id = ?1",
                params![request],
                |row| row.get(0),
            )
            .expect("tekst oryginalny");
        drop(conn);

        assert_eq!(original, "dla kasi", "tekst gościa zostaje w bazie");
    }

    #[test]
    fn the_ready_queue_keeps_the_order_the_dj_set() {
        let db = Db::open_in_memory().expect("baza w pamięci");
        let first = a_request(&db, "Pierwsza", None);
        let second = a_request(&db, "Druga", None);
        let third = a_request(&db, "Trzecia", None);

        for request in [first, second, third] {
            db.approve_request(request).expect("zatwierdzenie");
        }

        let order = |db: &Db| -> Vec<i64> {
            db.ready_requests(10)
                .expect("kolejka gotowych")
                .into_iter()
                .map(|entry| entry.id)
                .collect()
        };

        assert_eq!(order(&db), vec![first, second, third]);

        assert!(db.move_request(third, true).expect("przesunięcie"));
        assert_eq!(order(&db), vec![first, third, second]);

        assert!(
            !db.move_request(first, true).expect("początek kolejki"),
            "prośba z początku nie ma dokąd iść"
        );
        assert!(
            !db.move_request(second, false).expect("koniec kolejki"),
            "prośba z końca nie ma dokąd iść"
        );
        assert_eq!(order(&db), vec![first, third, second]);
    }

    #[test]
    fn only_ready_requests_can_be_reordered() {
        let db = Db::open_in_memory().expect("baza w pamięci");
        let request = a_request(&db, "Czeka na przegląd", None);

        assert!(
            !db.move_request(request, true)
                .expect("przesunięcie spoza kolejki"),
            "prośba bez zatwierdzenia nie stoi w kolejce gotowych"
        );
    }

    #[test]
    fn a_missing_request_cannot_be_changed() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        assert!(matches!(
            db.approve_request(999),
            Err(DbError::RequestNotFound(999))
        ));
        assert!(matches!(
            db.reject_request(999),
            Err(DbError::RequestNotFound(999))
        ));
        assert!(matches!(
            db.set_dedication(999, "tekst"),
            Err(DbError::RequestNotFound(999))
        ));
        assert_eq!(db.request_target(999).expect("adres prośby"), None);
    }

    #[test]
    fn a_finished_request_cannot_be_approved_again() {
        let db = Db::open_in_memory().expect("baza w pamięci");
        let request = a_request(&db, "Sto lat!", None);

        db.reject_request(request).expect("odrzucenie");

        assert!(matches!(
            db.approve_request(request),
            Err(DbError::InvalidTransition { .. })
        ));
        assert!(matches!(
            db.reject_request(request),
            Err(DbError::InvalidTransition { .. })
        ));
    }
}
