//! Logowanie aplikacji DJ-a: konsola oraz plik w katalogu logów aplikacji.
//!
//! Poziom logowania można nadpisać zmienną środowiskową `RUST_LOG` (np. `RUST_LOG=debug`).

use std::path::{Path, PathBuf};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Błąd konfiguracji logowania.
#[derive(Debug, thiserror::Error)]
pub enum LoggingError {
    #[error("nie udało się utworzyć katalogu logów {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("nie udało się zainicjalizować logowania: {0}")]
    Init(String),
}

/// Inicjalizuje globalnego subskrybenta logów i zwraca strażnika.
///
/// Strażnik **musi żyć przez cały czas działania programu** — jego upuszczenie zamyka wątek
/// zapisujący logi do pliku i końcówka logów przepada.
pub fn init(log_dir: &Path) -> Result<WorkerGuard, LoggingError> {
    std::fs::create_dir_all(log_dir).map_err(|source| LoggingError::CreateDir {
        path: log_dir.to_path_buf(),
        source,
    })?;

    let file_appender = tracing_appender::rolling::daily(log_dir, "anon-dj.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(
            // W pliku nie chcemy ANSI-owych kolorów — notatnik pokazałby krzaki.
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file_writer),
        )
        .try_init()
        .map_err(|error| LoggingError::Init(error.to_string()))?;

    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_the_log_directory() {
        let dir = std::env::temp_dir().join("anon-dj-logging-test");

        // `try_init` może zwrócić błąd, jeśli subskrybent był już ustawiony w tym procesie,
        // ale sam katalog powinien powstać niezależnie od tego.
        let _ = init(&dir);

        assert!(dir.is_dir(), "katalog logów powinien istnieć");
    }
}
