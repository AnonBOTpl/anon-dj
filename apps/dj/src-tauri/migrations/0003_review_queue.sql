-- Kolejki przeglądu i gotowych do wykonania (PLAN.md, sekcja 6).
--
-- `position` ustala kolejność wykonywania zatwierdzonych prośb — DJ może ją zmieniać,
-- a pusty wiersz oznacza prośbę, która nie stoi w kolejce gotowych.
--
-- `kiosk_request_id` to identyfikator, którym kiosk oznaczył swoją prośbę. Bez niego nie da się
-- powiedzieć kioskowi, że DJ zatwierdził albo odrzucił właśnie tę dedykację.

ALTER TABLE requests ADD COLUMN kiosk_request_id INTEGER;
ALTER TABLE requests ADD COLUMN position INTEGER;

-- Kolejka gotowych jest zawsze czytana po statusie i kolejności.
CREATE INDEX IF NOT EXISTS idx_requests_status_position ON requests(status, position);
