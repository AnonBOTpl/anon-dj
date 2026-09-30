//! Lokalna baza SQLite aplikacji DJ-a.
//!
//! Schemat trzymamy w migracjach uruchamianych przy starcie (numer wersji w `PRAGMA user_version`).
//! Baza leży w katalogu danych aplikacji i nie zawiera plików muzycznych — tylko metadane i ścieżki
//! (PLAN.md, sekcja 9).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};

/// Migracje w kolejności stosowania. Każda następna podnosi `user_version` o jeden.
const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_initial.sql")];

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

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, DbError> {
        // Zatruty mutex oznacza panikę w innym wątku — traktujemy to jako błąd bazy,
        // nigdy nie panikujemy w kodzie aplikacji.
        self.conn.lock().map_err(|_| DbError::Poisoned)
    }
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    /// Znacznik czasu w milisekundach od epoki, taki jak w schemacie.
    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default()
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

        for table in ["tracks", "requests", "kiosks", "settings"] {
            assert!(table_exists(&db, table), "brak tabeli {table}");
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
}
