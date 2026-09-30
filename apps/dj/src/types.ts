/**
 * Typy wymieniane między interfejsem DJ-a a warstwą Rust. Muszą zgadzać się z `QueuedRequest`
 * w `src-tauri/src/db.rs` oraz ze statusami w crate `protocol`.
 */

/** Status prośby gościa (PLAN.md, sekcja 6). */
export type RequestStatus = "submitted" | "approved" | "playing" | "done" | "rejected";

/** Prośba gościa widziana przez DJ-a. */
export type QueuedRequest = {
  id: number;
  track_id: number;
  title: string;
  artist: string;
  /** Tekst po edycji DJ-a; dopóki jej nie ma — tekst wpisany przez gościa. */
  dedication: string;
  guest_name: string | null;
  status: RequestStatus;
  created_at: number;
  /** Czas ostatniej zmiany — po nim sortowana jest historia. */
  updated_at: number;
};

/** Limity tekstów ustawione przez DJ-a — zgadzają się z typem `Limits` w `crate protocol`. */
export type Limits = {
  dedication_max_chars: number;
  guest_name_max_chars: number;
  search_query_max_chars: number;
};

/** Ustawienia DJ-a — zgadzają się z typem `AppSettings` w `src-tauri/src/settings.rs`. */
export type AppSettings = {
  pin: string;
  port: number;
  confirmation_seconds: number;
  limits: Limits;
};
