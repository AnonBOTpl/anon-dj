//! Skanowanie folderów z muzyką i odczyt metadanych (PLAN.md, sekcje 4.1 i 10).
//!
//! Skanowanie chodzi po dysku i otwiera pliki, więc zawsze idzie przez wątek roboczy — nigdy
//! przez wątek interfejsu (AGENTS.md, zasady architektury). Baza trzyma wyłącznie metadane
//! i ścieżki; pliki muzyczne nie są nigdzie kopiowane.

use std::path::{Path, PathBuf};

use lofty::prelude::{Accessor, AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use tracing::debug;
use walkdir::WalkDir;

use crate::db::{Db, DbError, TrackRecord};

/// Ile utworów zwraca jedno wyszukiwanie w bibliotece DJ-a. Kiosk ma własny, mniejszy limit
/// (`protocol::MAX_SEARCH_RESULTS`) — DJ przegląda bibliotekę inaczej niż gość.
pub const MAX_SEARCH_RESULTS: u32 = 200;

/// Co ile plików meldujemy postęp. Zdarzenie na każdy plik zalałoby interfejs przy dużej bibliotece.
pub const PROGRESS_EVERY_FILES: u64 = 100;

/// Rozszerzenia plików, które traktujemy jako muzykę.
///
/// Lista odpowiada formatom obsługiwanym przez `lofty` — plik z innym rozszerzeniem i tak nie
/// dałby się odczytać, a próba otwierania każdego pliku w folderze jest niepotrzebnie wolna.
/// Celowo nie ma tu `wma` (nieobsługiwane przez `lofty`) ani `mp4` (pod tym rozszerzeniem
/// równie często leży wideo).
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "opus", "m4a", "m4b", "aac", "wav", "wave", "aif", "aiff", "aifc",
    "ape", "wv", "mpc",
];

/// Błąd warstwy biblioteki.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("błąd bazy danych: {0}")]
    Db(#[from] DbError),

    #[error("nie udało się odczytać metadanych: {0}")]
    Metadata(#[from] lofty::error::FileParseError),

    #[error("nie udało się odczytać pliku: {0}")]
    Io(#[from] std::io::Error),

    #[error("ścieżka {path} nie istnieje albo nie jest folderem")]
    NotAFolder { path: PathBuf },

    #[error("skanowanie już trwa")]
    AlreadyScanning,

    #[error("nie wskazano żadnego folderu z muzyką")]
    NoFolders,
}

/// Metadane odczytane z pliku. Puste wartości są w porządku — nie każdy plik ma tagi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
}

/// Podsumowanie jednego przebiegu skanowania.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanSummary {
    /// Pliki z rozszerzeniem audio, które napotkaliśmy.
    pub files_seen: u64,
    /// Pliki zapisane w bazie.
    pub indexed: u64,
    /// Pliki pominięte: uszkodzone albo bez czytelnych metadanych.
    pub skipped: u64,
}

impl ScanSummary {
    /// Dolicza wynik skanowania kolejnego folderu.
    pub fn merge(&mut self, other: &Self) {
        self.files_seen += other.files_seen;
        self.indexed += other.indexed;
        self.skipped += other.skipped;
    }
}

/// Skanuje jeden folder i zapisuje metadane znalezionych utworów.
///
/// `on_progress` jest wołane co `PROGRESS_EVERY_FILES` plików i na końcu folderu; dostaje
/// podsumowanie **bieżącego** folderu.
pub fn scan_folder(
    db: &Db,
    folder: &Path,
    mut on_progress: impl FnMut(&ScanSummary),
) -> Result<ScanSummary, LibraryError> {
    if !folder.is_dir() {
        return Err(LibraryError::NotAFolder {
            path: folder.to_path_buf(),
        });
    }

    let mut summary = ScanSummary::default();
    let mut since_last_report = 0_u64;

    for entry in WalkDir::new(folder).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            // Problemy z pojedynczym wpisem (brak dostępu, zerwane dowiązanie) nie mogą
            // przewrócić całego skanowania.
            Err(error) => {
                debug!(error = %error, "pominięto wpis podczas skanowania");

                continue;
            }
        };

        if !entry.file_type().is_file() || !is_audio_file(entry.path()) {
            continue;
        }

        summary.files_seen += 1;

        match index_file(db, entry.path()) {
            Ok(()) => summary.indexed += 1,
            Err(error) => {
                debug!(path = %entry.path().display(), error = %error, "pominięto plik");

                summary.skipped += 1;
            }
        }

        since_last_report += 1;
        if since_last_report >= PROGRESS_EVERY_FILES {
            since_last_report = 0;
            on_progress(&summary);
        }
    }

    on_progress(&summary);

    Ok(summary)
}

/// Odczytuje metadane pliku i zapisuje je w bazie (nadpisując wpis o tej samej ścieżce).
fn index_file(db: &Db, path: &Path) -> Result<(), LibraryError> {
    let metadata = read_metadata(path)?;

    // Ścieżka w bazie musi być tekstem: `rusqlite` nie przyjmie surowych bajtów, a na Windows
    // nie-UTF-8 zdarza się rzadko (i taki plik po prostu pominiemy, licząc go jako błąd odczytu).
    let record = TrackRecord::new(
        path.to_string_lossy().to_string(),
        metadata.title,
        metadata.artist,
        metadata.album,
        metadata.duration_ms,
    );

    db.upsert_track(&record)?;

    Ok(())
}

/// Rozpoznaje plik audio po rozszerzeniu.
pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            let lowercased = extension.to_ascii_lowercase();

            AUDIO_EXTENSIONS.contains(&lowercased.as_str())
        })
        .unwrap_or(false)
}

/// Czyta metadane pliku.
///
/// Tytułu nie da się zgubić: gdy tag milczy, bierzemy nazwę pliku. Wykonawca może zostać pusty —
/// tekst zastępczy („Nieznany wykonawca”) dokłada interfejs, żeby napisy były w jednym miejscu.
pub fn read_metadata(path: &Path) -> Result<TrackMetadata, LibraryError> {
    let tagged_file = Probe::open(path)?.guess_file_type()?.read()?;

    let duration_ms = i64::try_from(tagged_file.properties().duration().as_millis()).ok();
    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag());

    let title = tag
        .and_then(|tag| tag.title())
        .as_deref()
        .and_then(clean_tag_value)
        .or_else(|| file_stem(path))
        .unwrap_or_default();

    let artist = tag
        .and_then(|tag| tag.artist())
        .as_deref()
        .and_then(clean_tag_value)
        .unwrap_or_default();

    let album = tag
        .and_then(|tag| tag.album())
        .as_deref()
        .and_then(clean_tag_value);

    Ok(TrackMetadata {
        title,
        artist,
        album,
        duration_ms,
    })
}

/// Ścieżka folderu w jednej, przewidywalnej postaci: bez białych znaków na końcach, ze zwykłymi
/// ukośnikami Windows i bez końcowego separatora (poza samym dyskiem, np. `C:\`) — dzięki temu
/// porównania i wzorce `LIKE` nie zależą od tego, jak folder został wskazany w oknie wyboru.
pub fn normalize_folder_path(raw: &str) -> String {
    let unified = raw.trim().replace('/', "\\");
    let trimmed = unified.trim_end_matches('\\');

    match trimmed.len() {
        0 => unified,
        2 if trimmed.ends_with(':') => format!("{trimmed}\\"),
        _ => trimmed.to_string(),
    }
}

/// Nazwa pliku bez rozszerzenia — ostatnia deska ratunku, gdy plik nie ma tytułu w tagach.
fn file_stem(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(clean_tag_value)
}

/// Porządkuje wartość z taga: usuwa znaki sterujące (potrafią rozsypać listę w interfejsie)
/// i zwija powtórzone białe znaki. Wartość pusta to `None`, nie pusty napis.
fn clean_tag_value(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");

    (!collapsed.is_empty()).then_some(collapsed)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Tworzy unikalny katalog roboczy dla testu.
    fn temp_dir(name: &str) -> PathBuf {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("anon-dj-{}-{name}-{unique}", std::process::id()));

        fs::create_dir_all(&dir).expect("katalog testowy");

        dir
    }

    /// Zapisuje minimalny, poprawny plik WAV z ciszą — dzięki temu testy nie wożą binarnych
    /// próbek w repozytorium, a mimo to przechodzą przez prawdziwy parser `lofty`.
    fn write_silent_wav(path: &Path) {
        const SAMPLE_RATE: u32 = 8_000;
        const SAMPLES: u32 = 800; // 0,1 s

        let mut bytes = Vec::with_capacity(44 + SAMPLES as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + SAMPLES).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1_u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes()); // bajtów na sekundę
        bytes.extend_from_slice(&1_u16.to_le_bytes()); // wyrównanie bloku
        bytes.extend_from_slice(&8_u16.to_le_bytes()); // bitów na próbkę
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&SAMPLES.to_le_bytes());
        bytes.extend(std::iter::repeat_n(128_u8, SAMPLES as usize));

        fs::write(path, bytes).expect("zapis pliku WAV");
    }

    #[test]
    fn audio_files_are_recognized_by_extension() {
        assert!(is_audio_file(Path::new("C:\\muzyka\\utwor.MP3")));
        assert!(is_audio_file(Path::new("utwor.flac")));
        assert!(is_audio_file(Path::new("utwor.m4a")));

        assert!(!is_audio_file(Path::new("okladka.jpg")));
        assert!(!is_audio_file(Path::new("playlista.m3u")));
        assert!(!is_audio_file(Path::new("bez_rozszerzenia")));
    }

    #[test]
    fn folder_paths_are_normalized_for_windows() {
        assert_eq!(
            normalize_folder_path("  C:/Muzyka/Wesela/  "),
            "C:\\Muzyka\\Wesela"
        );
        assert_eq!(normalize_folder_path("C:\\Muzyka\\"), "C:\\Muzyka");
        assert_eq!(normalize_folder_path("D:/"), "D:\\");
        assert_eq!(normalize_folder_path("C:\\"), "C:\\");
    }

    #[test]
    fn tag_values_are_cleaned_and_empty_ones_are_dropped() {
        assert_eq!(clean_tag_value("  Kombi  "), Some("Kombi".to_string()));
        assert_eq!(
            clean_tag_value("Kombi\r\nLive"),
            Some("Kombi Live".to_string())
        );
        assert_eq!(clean_tag_value("A\u{7}B"), Some("A B".to_string()));
        assert_eq!(clean_tag_value("   "), None);
    }

    #[test]
    fn scanning_indexes_files_and_falls_back_to_the_file_name() {
        let dir = temp_dir("scan");
        write_silent_wav(&dir.join("Slodki utwor.wav"));

        let db = Db::open_in_memory().expect("baza w pamięci");
        let summary = scan_folder(&db, &dir, |_| {}).expect("skanowanie");

        assert_eq!(summary.files_seen, 1);
        assert_eq!(summary.indexed, 1);
        assert_eq!(summary.skipped, 0);

        let found = db
            .search_tracks("slodki", MAX_SEARCH_RESULTS)
            .expect("wyszukiwanie");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Slodki utwor");
        assert_eq!(
            found[0].artist, "",
            "brak wykonawcy zostaje pusty — tekst zastępczy dokłada interfejs"
        );
        assert!(found[0].duration_ms.is_some(), "czas trwania z właściwości");

        fs::remove_dir_all(&dir).expect("sprzątanie");
    }

    #[test]
    fn unreadable_files_are_counted_and_skipped() {
        let dir = temp_dir("broken");
        fs::write(dir.join("zepsuty.mp3"), b"to nie jest plik mp3").expect("zapis pliku");

        let db = Db::open_in_memory().expect("baza w pamięci");
        let summary = scan_folder(&db, &dir, |_| {}).expect("skanowanie");

        assert_eq!(summary.files_seen, 1);
        assert_eq!(summary.indexed, 0);
        assert_eq!(summary.skipped, 1);
        assert_eq!(db.count_tracks().expect("liczba utworów"), 0);

        fs::remove_dir_all(&dir).expect("sprzątanie");
    }

    /// Kontrola na prawdziwych plikach DJ-a. Odpala się tylko wtedy, gdy zmienna środowiskowa
    /// `ANON_DJ_TEST_MUSIC_DIR` wskazuje folder z muzyką — repozytorium nie wozi cudzych plików,
    /// więc bez tej zmiennej test nie ma czego sprawdzać i kończy się od razu.
    ///
    /// Uruchamianie:
    /// `ANON_DJ_TEST_MUSIC_DIR=... cargo test -p dj real_files -- --nocapture`
    #[test]
    fn real_files_from_the_dj_are_indexed() {
        let Ok(directory) = std::env::var("ANON_DJ_TEST_MUSIC_DIR") else {
            return;
        };

        let directory = PathBuf::from(directory);

        if !directory.is_dir() {
            return;
        }

        let db = Db::open_in_memory().expect("baza w pamięci");
        let summary = scan_folder(&db, &directory, |_| {}).expect("skanowanie");

        assert!(summary.files_seen > 0, "folder nie zawiera plików audio");
        assert_eq!(
            summary.skipped, 0,
            "każdy prawdziwy plik powinien dać się odczytać"
        );

        let tracks = db.search_tracks("", MAX_SEARCH_RESULTS).expect("lista");

        assert_eq!(
            tracks.len(),
            usize::try_from(summary.indexed).unwrap_or_default()
        );

        for track in &tracks {
            println!(
                "{} | {} | {:?} ms",
                track.title, track.artist, track.duration_ms
            );

            assert!(!track.title.is_empty(), "utwór bez tytułu");
            assert!(track.duration_ms.is_some(), "utwór bez czasu trwania");
        }

        // Tytuł z prawdziwego pliku bywa polski — musi dać się znaleźć po własnym tekście.
        if let Some(first) = tracks.first() {
            let found = db
                .search_tracks(&first.title.to_lowercase(), MAX_SEARCH_RESULTS)
                .expect("wyszukiwanie");

            assert!(
                found.iter().any(|track| track.id == first.id),
                "utwór „{}” nie został znaleziony po własnym tytule",
                first.title
            );
        }
    }

    #[test]
    fn scanning_a_missing_folder_is_an_error() {
        let db = Db::open_in_memory().expect("baza w pamięci");
        let missing = std::env::temp_dir().join("anon-dj-nie-ma-takiego-folderu");

        assert!(matches!(
            scan_folder(&db, &missing, |_| {}),
            Err(LibraryError::NotAFolder { .. })
        ));
    }
}
