//! Wspólne typy i walidacja wiadomości dla ANON DJ.
//!
//! To jedyne miejsce, w którym żyją typy wiadomości wymienianych między aplikacją
//! DJ-a a kioskiem (PLAN.md, sekcja 4.3). Obie aplikacje korzystają z tego crate'a —
//! żadna nie definiuje własnych kopii.

pub mod message;
pub mod validation;

pub use message::{
    DjMessage, ErrorCode, KioskMessage, Limits, MAX_DEDICATION_CHARS, MAX_GUEST_NAME_CHARS,
    MAX_KIOSK_NAME_CHARS, MAX_MESSAGE_BYTES, MAX_SEARCH_QUERY_CHARS, MAX_SEARCH_RESULTS,
    MessageError, PIN_LEN, PROTOCOL_VERSION, RequestStatus, TrackInfo,
};
pub use validation::{
    ValidationError, count_chars, normalize_text, validate_dedication, validate_dedication_with,
    validate_guest_name, validate_guest_name_with, validate_pin, validate_search_query,
    validate_search_query_with,
};
