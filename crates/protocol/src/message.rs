//! Typy wiadomości wymienianych między aplikacją DJ-a a kioskiem.
//!
//! Kierunek przepływu jest zaszyty w typie: `KioskMessage` wysyła wyłącznie kiosk,
//! `DjMessage` wyłącznie aplikacja DJ-a. Kiosk nie ma żadnego komunikatu, który
//! sterowałby aplikacją DJ-a (PLAN.md, sekcja 12).

use serde::{Deserialize, Serialize};

/// Wersja protokołu. Podnosimy przy każdej niekompatybilnej zmianie formatu.
pub const PROTOCOL_VERSION: u16 = 1;

/// Twardy limit rozmiaru surowego komunikatu w bajtach. Chroni przed zalaniem pamięci.
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024;

/// Maksymalna długość dedykacji w **znakach** (nie w bajtach — polskie znaki są dwubajtowe).
pub const MAX_DEDICATION_CHARS: usize = 400;

/// Maksymalna długość opcjonalnego imienia gościa.
pub const MAX_GUEST_NAME_CHARS: usize = 40;

/// Maksymalna długość zapytania wyszukiwania.
pub const MAX_SEARCH_QUERY_CHARS: usize = 80;

/// Maksymalna liczba utworów w jednej odpowiedzi na wyszukiwanie.
pub const MAX_SEARCH_RESULTS: usize = 50;

/// Maksymalna długość nazwy kiosku podawanej przy łączeniu.
pub const MAX_KIOSK_NAME_CHARS: usize = 40;

/// Wymagana liczba cyfr w kodzie PIN parowania.
pub const PIN_LEN: usize = 6;

/// Limity długości tekstów, które DJ może zmienić w ustawieniach.
///
/// Aplikacja DJ-a przekazuje je kioskowi przy parowaniu, żeby ten sam limit obowiązywał
/// w interfejsie gościa i w walidacji po stronie DJ-a. Walidacja zawsze idzie po stronie
/// DJ-a — kiosk jest tylko wygodą dla gościa, nie źródłem prawdy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub dedication_max_chars: u32,
    pub guest_name_max_chars: u32,
    pub search_query_max_chars: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            dedication_max_chars: MAX_DEDICATION_CHARS as u32,
            guest_name_max_chars: MAX_GUEST_NAME_CHARS as u32,
            search_query_max_chars: MAX_SEARCH_QUERY_CHARS as u32,
        }
    }
}

/// Metadane utworu wysyłane do kiosku. Kiosk **nigdy** nie dostaje ścieżki do pliku.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackInfo {
    pub id: i64,
    pub title: String,
    pub artist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u32>,
}

/// Status prośby (PLAN.md, sekcja 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    Submitted,
    Approved,
    Playing,
    Done,
    Rejected,
}

/// Kod błędu wysyłany do kiosku. Kiosk tłumaczy kod na polski komunikat — teksty nie jadą po sieci.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    InvalidMessage,
    InvalidRequest,
    TooLong,
    RateLimited,
    DuplicateRequest,
    Internal,
}

/// Wiadomości wysyłane przez kiosk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KioskMessage {
    /// Identyfikacja kiosku i podanie kodu PIN przy łączeniu.
    Hello {
        protocol_version: u16,
        pin: String,
        kiosk_name: String,
    },
    /// Wyszukanie utworu w katalogu DJ-a.
    Search { request_id: u64, query: String },
    /// Zgłoszenie prośby: utwór, dedykacja, opcjonalne imię gościa.
    SubmitRequest {
        request_id: u64,
        track_id: i64,
        dedication: String,
        guest_name: Option<String>,
    },
}

/// Wiadomości wysyłane przez aplikację DJ-a.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DjMessage {
    /// Potwierdzenie parowania wraz z limitami ustawionymi przez DJ-a.
    HelloOk {
        protocol_version: u16,
        kiosk_id: i64,
        server_name: String,
        limits: Limits,
    },
    /// Wyniki wyszukiwania (tylko metadane).
    SearchResults {
        request_id: u64,
        tracks: Vec<TrackInfo>,
    },
    /// Prośba przyjęta do kolejki przeglądu.
    RequestReceived {
        request_id: u64,
        status: RequestStatus,
    },
    /// Zmiana statusu prośby.
    RequestStatus {
        request_id: u64,
        status: RequestStatus,
    },
    /// Błąd: walidacja, parowanie, limit tempa.
    Error {
        request_id: Option<u64>,
        code: ErrorCode,
    },
}

/// Błąd parsowania lub serializacji komunikatu.
#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    #[error("wiadomość jest za duża: {0} B, limit {1} B")]
    TooLarge(usize, usize),

    #[error("nieznany lub uszkodzony komunikat: {0}")]
    Malformed(#[from] serde_json::Error),
}

impl KioskMessage {
    /// Parsuje surowy tekst odebrany z sieci, pilnując limitu rozmiaru.
    pub fn from_json(raw: &str) -> Result<Self, MessageError> {
        if raw.len() > MAX_MESSAGE_BYTES {
            return Err(MessageError::TooLarge(raw.len(), MAX_MESSAGE_BYTES));
        }

        Ok(serde_json::from_str(raw)?)
    }

    /// Serializuje komunikat do tekstu.
    pub fn to_json(&self) -> Result<String, MessageError> {
        Ok(serde_json::to_string(self)?)
    }

    /// Identyfikator prośby, jeśli komunikat go niesie — do logów i odpowiedzi z błędem.
    pub fn request_id(&self) -> Option<u64> {
        match self {
            Self::Hello { .. } => None,
            Self::Search { request_id, .. } | Self::SubmitRequest { request_id, .. } => {
                Some(*request_id)
            }
        }
    }
}

impl DjMessage {
    /// Parsuje surowy tekst odebrany z sieci, pilnując limitu rozmiaru.
    pub fn from_json(raw: &str) -> Result<Self, MessageError> {
        if raw.len() > MAX_MESSAGE_BYTES {
            return Err(MessageError::TooLarge(raw.len(), MAX_MESSAGE_BYTES));
        }

        Ok(serde_json::from_str(raw)?)
    }

    /// Serializuje komunikat do tekstu.
    pub fn to_json(&self) -> Result<String, MessageError> {
        Ok(serde_json::to_string(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kiosk_hello_round_trip() {
        let message = KioskMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            pin: "123456".to_string(),
            kiosk_name: "Kiosk przy wejściu".to_string(),
        };

        let json = message.to_json().expect("serializacja");
        let parsed = KioskMessage::from_json(&json).expect("deserializacja");

        assert_eq!(message, parsed);
        assert!(json.contains("\"type\":\"hello\""));
    }

    #[test]
    fn kiosk_submit_request_round_trip() {
        let message = KioskMessage::SubmitRequest {
            request_id: 7,
            track_id: 42,
            dedication: "Kasia i Marek — sto lat!".to_string(),
            guest_name: Some("Ania".to_string()),
        };

        let json = message.to_json().expect("serializacja");
        let parsed = KioskMessage::from_json(&json).expect("deserializacja");

        assert_eq!(message, parsed);
        assert_eq!(parsed.request_id(), Some(7));
    }

    #[test]
    fn kiosk_search_round_trip() {
        let message = KioskMessage::Search {
            request_id: 1,
            query: "Kombi".to_string(),
        };

        let json = message.to_json().expect("serializacja");
        let parsed = KioskMessage::from_json(&json).expect("deserializacja");

        assert_eq!(message, parsed);
    }

    #[test]
    fn dj_hello_ok_carries_limits() {
        let message = DjMessage::HelloOk {
            protocol_version: PROTOCOL_VERSION,
            kiosk_id: 1,
            server_name: "ANON DJ".to_string(),
            limits: Limits {
                dedication_max_chars: 250,
                guest_name_max_chars: 30,
                search_query_max_chars: 60,
            },
        };

        let json = message.to_json().expect("serializacja");
        let parsed = DjMessage::from_json(&json).expect("deserializacja");

        assert_eq!(message, parsed);
    }

    #[test]
    fn default_limits_match_the_protocol_constants() {
        let limits = Limits::default();

        assert_eq!(limits.dedication_max_chars, MAX_DEDICATION_CHARS as u32);
        assert_eq!(limits.guest_name_max_chars, MAX_GUEST_NAME_CHARS as u32);
        assert_eq!(limits.search_query_max_chars, MAX_SEARCH_QUERY_CHARS as u32);
    }

    #[test]
    fn dj_search_results_round_trip() {
        let message = DjMessage::SearchResults {
            request_id: 1,
            tracks: vec![TrackInfo {
                id: 3,
                title: "Słodkiego, miłego życia".to_string(),
                artist: "Kombi".to_string(),
                album: None,
                duration_ms: Some(255_000),
            }],
        };

        let json = message.to_json().expect("serializacja");
        let parsed = DjMessage::from_json(&json).expect("deserializacja");

        assert_eq!(message, parsed);
        assert!(
            !json.contains("album"),
            "puste pola opcjonalne nie jadą po sieci"
        );
    }

    #[test]
    fn dj_status_and_error_round_trip() {
        let status = DjMessage::RequestStatus {
            request_id: 5,
            status: RequestStatus::Approved,
        };
        let json = status.to_json().expect("serializacja");
        assert!(json.contains("\"approved\""));
        assert_eq!(status, DjMessage::from_json(&json).expect("deserializacja"));

        let error = DjMessage::Error {
            request_id: None,
            code: ErrorCode::RateLimited,
        };
        let json = error.to_json().expect("serializacja");
        assert!(json.contains("\"rate_limited\""));
        assert_eq!(error, DjMessage::from_json(&json).expect("deserializacja"));
    }

    #[test]
    fn unknown_type_is_rejected() {
        let result = KioskMessage::from_json(r#"{"type":"nuke_the_dj"}"#);

        assert!(matches!(result, Err(MessageError::Malformed(_))));
    }

    #[test]
    fn malformed_json_is_rejected() {
        assert!(matches!(
            KioskMessage::from_json("{nie json"),
            Err(MessageError::Malformed(_))
        ));
        assert!(matches!(
            KioskMessage::from_json(r#"{"type":"hello","protocol_version":"jeden"}"#),
            Err(MessageError::Malformed(_))
        ));
    }

    #[test]
    fn oversized_message_is_rejected_before_parsing() {
        let raw = "a".repeat(MAX_MESSAGE_BYTES + 1);

        assert!(matches!(
            KioskMessage::from_json(&raw),
            Err(MessageError::TooLarge(_, MAX_MESSAGE_BYTES))
        ));
    }

    #[test]
    fn dj_message_cannot_be_parsed_as_kiosk_message() {
        let dj = DjMessage::RequestStatus {
            request_id: 1,
            status: RequestStatus::Done,
        };
        let json = dj.to_json().expect("serializacja");

        assert!(KioskMessage::from_json(&json).is_err());
    }
}
