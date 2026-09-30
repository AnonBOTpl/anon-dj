-- Biblioteka muzyczna: wyszukiwanie po tytule i wykonawcy oraz foldery wybrane do skanowania.
--
-- SQLite porównuje `LIKE` bez rozróżniania wielkości liter tylko dla ASCII — dla niego „Ł” i „ł”
-- to dwa różne znaki. Dlatego każdy utwór dostaje dodatkowe, znormalizowane kolumny wypełniane
-- po stronie Rusta (Unicode-aware `to_lowercase`) i to po nich idzie wyszukiwanie.

ALTER TABLE tracks ADD COLUMN title_fold TEXT NOT NULL DEFAULT '';
ALTER TABLE tracks ADD COLUMN artist_fold TEXT NOT NULL DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_tracks_title_fold ON tracks(title_fold);
CREATE INDEX IF NOT EXISTS idx_tracks_artist_fold ON tracks(artist_fold);

-- Foldery, które DJ wskazał do skanowania. Trzymamy tylko ścieżki — pliki muzyczne nigdy
-- nie są kopiowane ani przenoszone (PLAN.md, sekcja 9).
CREATE TABLE IF NOT EXISTS library_folders (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    added_at INTEGER NOT NULL
);
