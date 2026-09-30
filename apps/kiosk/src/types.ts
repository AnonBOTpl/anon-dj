/**
 * Typy wymieniane między interfejsem kiosku a warstwą Rust. Muszą zgadzać się z crate `protocol`
 * i z typami w `src-tauri/src/net.rs`.
 */

/** Limity tekstów ustawione przez DJ-a. Kiosk pokazuje gościowi ten sam limit, co walidacja DJ-a. */
export type Limits = {
  dedication_max_chars: number;
  guest_name_max_chars: number;
  search_query_max_chars: number;
};

/** Metadane utworu. Kiosk nigdy nie dostaje ścieżki do pliku. */
export type TrackInfo = {
  id: number;
  title: string;
  artist: string;
  album?: string | null;
  duration_ms?: number | null;
};

export type RequestStatus = "submitted" | "approved" | "playing" | "done" | "rejected";

export type ErrorCode =
  | "unauthorized"
  | "invalid_message"
  | "invalid_request"
  | "too_long"
  | "rate_limited"
  | "duplicate_request"
  | "internal";

/** Stan połączenia z aplikacją DJ-a — zgadza się z `ConnectionState` w warstwie Rust. */
export type ConnectionState =
  | { state: "disconnected" }
  | { state: "connecting" }
  | {
      state: "connected";
      server_name: string;
      kiosk_id: number;
      limits: Limits;
    };
