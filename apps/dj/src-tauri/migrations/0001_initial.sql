-- Schemat początkowy ANON DJ (PLAN.md, sekcja 9).
-- Baza trzyma wyłącznie metadane i ścieżki — pliki muzyczne nigdy nie są kopiowane.
-- Znaczniki czasu zapisujemy jako liczbę milisekund od epoki.

CREATE TABLE IF NOT EXISTS tracks (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    album TEXT,
    duration_ms INTEGER,
    added_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS kiosks (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    token TEXT NOT NULL UNIQUE,
    last_seen_at INTEGER
);

CREATE TABLE IF NOT EXISTS requests (
    id INTEGER PRIMARY KEY,
    track_id INTEGER NOT NULL REFERENCES tracks(id),
    dedication_original TEXT NOT NULL,
    dedication_edited TEXT,
    guest_name TEXT,
    kiosk_id INTEGER REFERENCES kiosks(id),
    status TEXT NOT NULL,
    tts_clip_path TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    executed_at INTEGER
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Kolejka przeglądu i gotowe do wykonania prośby są filtrowane po statusie.
CREATE INDEX IF NOT EXISTS idx_requests_status ON requests(status);

-- Wyszukiwanie w bibliotece idzie po wykonawcy i tytule.
CREATE INDEX IF NOT EXISTS idx_tracks_artist_title ON tracks(artist, title);
