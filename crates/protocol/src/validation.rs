//! Walidacja i normalizacja danych przychodzących z sieci.
//!
//! Zasada z PLAN.md: nigdy nie ufamy kioskowi. Walidacja sprawdza typ komunikatu
//! (robi to już deserializacja), limity długości i sensowność identyfikatorów.

use crate::message::{
    KioskMessage, Limits, MAX_DEDICATION_CHARS, MAX_GUEST_NAME_CHARS, MAX_KIOSK_NAME_CHARS,
    MAX_SEARCH_QUERY_CHARS, PIN_LEN, PROTOCOL_VERSION,
};

/// Powód odrzucenia komunikatu lub danych w nim zawartych.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("nieobsługiwana wersja protokołu: {0}")]
    UnsupportedProtocolVersion(u16),

    #[error("kod PIN musi mieć dokładnie {PIN_LEN} cyfry")]
    InvalidPin,

    #[error("nazwa kiosku jest za długa (limit {MAX_KIOSK_NAME_CHARS} znaków)")]
    KioskNameTooLong,

    #[error("puste zapytanie wyszukiwania")]
    EmptySearchQuery,

    #[error("zapytanie wyszukiwania jest za długie (limit {MAX_SEARCH_QUERY_CHARS} znaków)")]
    SearchQueryTooLong,

    #[error("identyfikator utworu musi być dodatni")]
    InvalidTrackId,

    #[error("dedykacja jest pusta")]
    EmptyDedication,

    #[error("dedykacja jest za długa (limit {MAX_DEDICATION_CHARS} znaków)")]
    DedicationTooLong,

    #[error("imię gościa jest za długie (limit {MAX_GUEST_NAME_CHARS} znaków)")]
    GuestNameTooLong,

    #[error("tekst zawiera niedozwolone znaki kontrolne")]
    ControlCharacter,
}

/// Normalizuje tekst: obcina końce, zamienia ciągi białych znaków na pojedyncze spacje
/// i odrzuca znaki kontrolne (np. `\0`), których gość nie wpisze celowo.
pub fn normalize_text(input: &str) -> Result<String, ValidationError> {
    let mut out = String::with_capacity(input.len());
    let mut space_pending = false;

    for ch in input.chars() {
        if ch.is_whitespace() {
            space_pending = !out.is_empty();
            continue;
        }

        if ch.is_control() {
            return Err(ValidationError::ControlCharacter);
        }

        if space_pending {
            out.push(' ');
            space_pending = false;
        }

        out.push(ch);
    }

    Ok(out)
}

/// Liczba znaków (a nie bajtów) — polskie znaki zajmują po dwa bajty.
pub fn count_chars(text: &str) -> usize {
    text.chars().count()
}

/// Waliduje i normalizuje dedykację gościa z domyślnym limitem.
pub fn validate_dedication(input: &str) -> Result<String, ValidationError> {
    validate_dedication_with(input, MAX_DEDICATION_CHARS)
}

/// Waliduje i normalizuje dedykację gościa z limitem z ustawień DJ-a.
pub fn validate_dedication_with(input: &str, max_chars: usize) -> Result<String, ValidationError> {
    let normalized = normalize_text(input)?;

    if normalized.is_empty() {
        return Err(ValidationError::EmptyDedication);
    }

    if count_chars(&normalized) > max_chars {
        return Err(ValidationError::DedicationTooLong);
    }

    Ok(normalized)
}

/// Waliduje i normalizuje opcjonalne imię gościa z domyślnym limitem.
pub fn validate_guest_name(input: &str) -> Result<Option<String>, ValidationError> {
    validate_guest_name_with(input, MAX_GUEST_NAME_CHARS)
}

/// Waliduje i normalizuje opcjonalne imię gościa. Pusty tekst oznacza brak imienia.
pub fn validate_guest_name_with(
    input: &str,
    max_chars: usize,
) -> Result<Option<String>, ValidationError> {
    let normalized = normalize_text(input)?;

    if count_chars(&normalized) > max_chars {
        return Err(ValidationError::GuestNameTooLong);
    }

    Ok(if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    })
}

/// Waliduje i normalizuje zapytanie wyszukiwania z domyślnym limitem.
pub fn validate_search_query(input: &str) -> Result<String, ValidationError> {
    validate_search_query_with(input, MAX_SEARCH_QUERY_CHARS)
}

/// Waliduje i normalizuje zapytanie wyszukiwania z limitem z ustawień DJ-a.
pub fn validate_search_query_with(
    input: &str,
    max_chars: usize,
) -> Result<String, ValidationError> {
    let normalized = normalize_text(input)?;

    if normalized.is_empty() {
        return Err(ValidationError::EmptySearchQuery);
    }

    if count_chars(&normalized) > max_chars {
        return Err(ValidationError::SearchQueryTooLong);
    }

    Ok(normalized)
}

/// Sprawdza kod PIN parowania: dokładnie `PIN_LEN` cyfr.
pub fn validate_pin(pin: &str) -> Result<(), ValidationError> {
    let is_valid = count_chars(pin) == PIN_LEN && pin.chars().all(|ch| ch.is_ascii_digit());

    if is_valid {
        Ok(())
    } else {
        Err(ValidationError::InvalidPin)
    }
}

impl KioskMessage {
    /// Sprawdza komunikat z domyślnymi limitami protokołu.
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.validate_with(&Limits::default())
    }

    /// Sprawdza cały komunikat przychodzący z kiosku z limitami z ustawień DJ-a.
    pub fn validate_with(&self, limits: &Limits) -> Result<(), ValidationError> {
        match self {
            Self::Hello {
                protocol_version,
                pin,
                kiosk_name,
            } => {
                if *protocol_version != PROTOCOL_VERSION {
                    return Err(ValidationError::UnsupportedProtocolVersion(
                        *protocol_version,
                    ));
                }

                validate_pin(pin)?;

                if count_chars(kiosk_name) > MAX_KIOSK_NAME_CHARS {
                    return Err(ValidationError::KioskNameTooLong);
                }

                Ok(())
            }
            // Podpowiedzi walidujemy tak samo jak wyszukiwanie: to ta sama fraza i te same
            // limity, a osobny komunikat wynika z osobnego budżetu tempa, nie z innych reguł.
            Self::Search { query, .. } | Self::Suggest { query, .. } => {
                validate_search_query_with(query, limits.search_query_max_chars as usize)?;

                Ok(())
            }
            Self::SubmitRequest {
                track_id,
                dedication,
                guest_name,
                ..
            } => {
                if *track_id <= 0 {
                    return Err(ValidationError::InvalidTrackId);
                }

                validate_dedication_with(dedication, limits.dedication_max_chars as usize)?;

                if let Some(guest_name) = guest_name {
                    validate_guest_name_with(guest_name, limits.guest_name_max_chars as usize)?;
                }

                Ok(())
            }
            // Znacznik życia nie niesie treści — nie ma czego sprawdzać.
            Self::Ping => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(protocol_version: u16, pin: &str) -> KioskMessage {
        KioskMessage::Hello {
            protocol_version,
            pin: pin.to_string(),
            kiosk_name: "Kiosk 1".to_string(),
        }
    }

    fn submit(track_id: i64, dedication: &str, guest_name: Option<&str>) -> KioskMessage {
        KioskMessage::SubmitRequest {
            request_id: 1,
            track_id,
            dedication: dedication.to_string(),
            guest_name: guest_name.map(str::to_string),
        }
    }

    #[test]
    fn a_heartbeat_passes_validation_and_is_marked_as_one() {
        let ping = KioskMessage::Ping;

        assert!(ping.validate().is_ok(), "znacznik życia nie ma czego łamać");
        assert!(ping.is_heartbeat());
        assert!(
            !KioskMessage::Search {
                request_id: 1,
                query: "kombi".to_string(),
            }
            .is_heartbeat()
        );
        assert_eq!(ping.request_id(), None);
    }

    #[test]
    fn normalize_collapses_whitespace_and_trims() {
        assert_eq!(
            normalize_text("  Kasia   i \n Marek \t sto lat  ").expect("normalizacja"),
            "Kasia i Marek sto lat"
        );
    }

    #[test]
    fn normalize_rejects_control_characters() {
        assert_eq!(
            normalize_text("sto\u{0}lat"),
            Err(ValidationError::ControlCharacter)
        );
    }

    #[test]
    fn dedication_limit_counts_characters_not_bytes() {
        let allowed = "ż".repeat(MAX_DEDICATION_CHARS);
        assert_eq!(allowed.len(), MAX_DEDICATION_CHARS * 2);
        assert!(validate_dedication(&allowed).is_ok());

        let too_long = "ż".repeat(MAX_DEDICATION_CHARS + 1);
        assert_eq!(
            validate_dedication(&too_long),
            Err(ValidationError::DedicationTooLong)
        );
    }

    #[test]
    fn dedication_must_not_be_empty_or_whitespace_only() {
        assert_eq!(
            validate_dedication(""),
            Err(ValidationError::EmptyDedication)
        );
        assert_eq!(
            validate_dedication("   \n\t "),
            Err(ValidationError::EmptyDedication)
        );
    }

    #[test]
    fn dedication_is_returned_normalized() {
        let dedication = validate_dedication("  Sto   lat!\nDla Kasi  ").expect("walidacja");

        assert_eq!(dedication, "Sto lat! Dla Kasi");
    }

    #[test]
    fn guest_name_is_optional() {
        assert_eq!(validate_guest_name(""), Ok(None));
        assert_eq!(validate_guest_name("  "), Ok(None));
        assert_eq!(validate_guest_name(" Ania "), Ok(Some("Ania".to_string())));
    }

    #[test]
    fn guest_name_has_a_limit() {
        let too_long = "a".repeat(MAX_GUEST_NAME_CHARS + 1);

        assert_eq!(
            validate_guest_name(&too_long),
            Err(ValidationError::GuestNameTooLong)
        );
    }

    #[test]
    fn search_query_is_validated() {
        assert_eq!(
            validate_search_query("   "),
            Err(ValidationError::EmptySearchQuery)
        );
        assert_eq!(
            validate_search_query(&"a".repeat(MAX_SEARCH_QUERY_CHARS + 1)),
            Err(ValidationError::SearchQueryTooLong)
        );
        assert_eq!(validate_search_query(" Kombi "), Ok("Kombi".to_string()));
    }

    #[test]
    fn pin_must_be_exactly_six_digits() {
        assert_eq!(validate_pin("123456"), Ok(()));
        assert_eq!(validate_pin("12345"), Err(ValidationError::InvalidPin));
        assert_eq!(validate_pin("1234567"), Err(ValidationError::InvalidPin));
        assert_eq!(validate_pin("12345a"), Err(ValidationError::InvalidPin));
        assert_eq!(validate_pin("12 456"), Err(ValidationError::InvalidPin));
    }

    #[test]
    fn hello_requires_current_protocol_version() {
        assert_eq!(hello(PROTOCOL_VERSION, "123456").validate(), Ok(()));
        assert_eq!(
            hello(PROTOCOL_VERSION + 1, "123456").validate(),
            Err(ValidationError::UnsupportedProtocolVersion(
                PROTOCOL_VERSION + 1
            ))
        );
    }

    #[test]
    fn hello_validates_pin_and_kiosk_name() {
        assert_eq!(
            hello(PROTOCOL_VERSION, "12345").validate(),
            Err(ValidationError::InvalidPin)
        );

        let kiosk_name_too_long = KioskMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            pin: "123456".to_string(),
            kiosk_name: "a".repeat(MAX_KIOSK_NAME_CHARS + 1),
        };

        assert_eq!(
            kiosk_name_too_long.validate(),
            Err(ValidationError::KioskNameTooLong)
        );
    }

    #[test]
    fn submit_request_validates_track_and_dedication() {
        assert_eq!(submit(42, "Sto lat!", None).validate(), Ok(()));
        assert_eq!(
            submit(0, "Sto lat!", None).validate(),
            Err(ValidationError::InvalidTrackId)
        );
        assert_eq!(
            submit(-1, "Sto lat!", None).validate(),
            Err(ValidationError::InvalidTrackId)
        );
        assert_eq!(
            submit(42, "  ", None).validate(),
            Err(ValidationError::EmptyDedication)
        );
        assert_eq!(
            submit(42, "Sto lat!", Some(&"a".repeat(MAX_GUEST_NAME_CHARS + 1))).validate(),
            Err(ValidationError::GuestNameTooLong)
        );
    }

    #[test]
    fn search_message_validates_query() {
        let message = KioskMessage::Search {
            request_id: 3,
            query: "  ".to_string(),
        };

        assert_eq!(message.validate(), Err(ValidationError::EmptySearchQuery));
    }

    #[test]
    fn suggest_message_validates_the_same_query_rules_as_search() {
        let empty = KioskMessage::Suggest {
            request_id: 1,
            query: "  ".to_string(),
        };
        assert_eq!(empty.validate(), Err(ValidationError::EmptySearchQuery));

        let normalized = KioskMessage::Suggest {
            request_id: 1,
            query: " Kombi ".to_string(),
        };
        assert_eq!(normalized.validate(), Ok(()));
    }

    #[test]
    fn limits_from_dj_settings_are_used_instead_of_protocol_defaults() {
        let limits = Limits {
            dedication_max_chars: 20,
            guest_name_max_chars: 5,
            search_query_max_chars: 3,
        };

        let too_long_dedication = "a".repeat(21);
        assert_eq!(
            submit(42, &too_long_dedication, None).validate_with(&limits),
            Err(ValidationError::DedicationTooLong)
        );
        assert_eq!(
            submit(42, "Krótka dedykacja", None).validate_with(&limits),
            Ok(())
        );

        assert_eq!(
            submit(42, "Krótka", Some("Anetka")).validate_with(&limits),
            Err(ValidationError::GuestNameTooLong)
        );

        let query = KioskMessage::Search {
            request_id: 1,
            query: "abcdef".to_string(),
        };
        assert_eq!(
            query.validate_with(&limits),
            Err(ValidationError::SearchQueryTooLong)
        );
    }
}
