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
    pub fn insert_request(
        &self,
        track_id: i64,
        dedication: &str,
        guest_name: Option<&str>,
        kiosk_id: Option<i64>,
    ) -> Result<(i64, i64), DbError> {
        let conn = self.lock()?;
        let created_at = now_ms();

        let id = conn.query_row(
            "INSERT INTO requests
                 (track_id, dedication_original, guest_name, kiosk_id, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             RETURNING id",
            params![
                track_id,
                dedication,
                guest_name,
                kiosk_id,
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

        let mut statement = conn.prepare(
            "SELECT r.id, r.track_id, t.title, t.artist,
                    COALESCE(r.dedication_edited, r.dedication_original),
                    r.guest_name, r.status, r.created_at
             FROM requests r
             JOIN tracks t ON t.id = r.track_id
             WHERE r.status = ?1
             ORDER BY r.created_at
             LIMIT ?2",
        )?;

        let rows = statement.query_map(params![status.as_str(), limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })?;

        let mut requests = Vec::new();

        for row in rows {
            let (id, track_id, title, artist, dedication, guest_name, stored_status, created_at) =
                row?;
            let status = RequestStatus::parse(&stored_status)
                .ok_or(DbError::UnknownStatus(stored_status))?;

            requests.push(QueuedRequest {
                id,
                track_id,
                title,
                artist,
                dedication,
                guest_name,
                status,
                created_at,
            });
        }

        Ok(requests)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, DbError> {
        // Zatruty mutex oznacza panikę w innym wątku — traktujemy to jako błąd bazy,
        // nigdy nie panikujemy w kodzie aplikacji.
        self.conn.lock().map_err(|_| DbError::Poisoned)
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

        db.insert_request(track_id, "Dla Kasi", Some("Ania"), Some(kiosk))
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
            db.insert_request(999, "Dla Kasi", None, None).is_err(),
            "klucz obcy musi odrzucić prośbę o nieistniejący utwór"
        );
    }

    #[test]
    fn the_same_kiosk_cannot_send_the_same_request_twice() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        store_track(&db, "C:\\muzyka\\a.mp3", "Alfa", "Zespół");

        let kiosk = db.kiosk_by_name("Kiosk 1").expect("parowanie");
        let now = now_ms();

        db.insert_request(1, "Sto lat!", None, Some(kiosk))
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
}
