//! Typowane ustawienia aplikacji DJ-a.
//!
//! PIN parowania i limity długości tekstów są **konfigurowalne** — nic nie jest zaszyte na sztywno
//! poza wartościami domyślnymi, od których startuje świeża instalacja. Ustawienia leżą w tabeli
//! `settings` jako pary klucz–wartość, a ten moduł dokłada typy, zakresy i walidację.
//!
//! Limity, które DJ ustawi, jadą do kiosku przy parowaniu (`DjMessage::HelloOk`), żeby gość
//! widział ten sam limit w interfejsie. Walidacja i tak zawsze idzie po stronie DJ-a.

use protocol::{Limits, PIN_LEN, validate_pin};
use tracing::warn;

use crate::db::{Db, DbError};

/// Klucz ustawienia z PIN-em parowania kiosków.
pub const KEY_PIN: &str = "security.pin";
/// Klucz ustawienia z limitem długości dedykacji.
pub const KEY_DEDICATION_MAX_CHARS: &str = "limits.dedication_max_chars";
/// Klucz ustawienia z limitem długości imienia gościa.
pub const KEY_GUEST_NAME_MAX_CHARS: &str = "limits.guest_name_max_chars";
/// Klucz ustawienia z limitem długości zapytania wyszukiwania.
pub const KEY_SEARCH_QUERY_MAX_CHARS: &str = "limits.search_query_max_chars";
/// Klucz ustawienia z portem lokalnego serwera dla kiosków.
pub const KEY_PORT: &str = "server.port";

/// PIN, od którego startuje świeża instalacja. DJ może i powinien go zmienić w ustawieniach.
pub const DEFAULT_PIN: &str = "123456";

/// Port serwera kiosków, od którego startuje świeża instalacja. Krótki i rzadko używany,
/// żeby nie zderzyć się z innymi usługami na komputerze DJ-a.
pub const DEFAULT_PORT: u16 = 8790;

/// Dopuszczalny zakres portu. Porty poniżej 1024 wymagają uprawnień administratora,
/// a 0 kazałoby systemowi wybrać port losowy — DJ nie miałby czego wpisać w kiosku.
pub const PORT_RANGE: (u16, u16) = (1024, 65_535);

/// Dopuszczalny zakres limitu dedykacji. Za krótki ucina gościowi tekst, za długi każe mu czekać
/// przy kiosku — oba skrajne przypadki szkodzą imprezie.
pub const DEDICATION_MAX_CHARS_RANGE: (u32, u32) = (50, 2_000);
/// Zakres limitu imienia gościa (0 = pole wyłączone).
pub const GUEST_NAME_MAX_CHARS_RANGE: (u32, u32) = (0, 80);
/// Zakres limitu zapytania wyszukiwania.
pub const SEARCH_QUERY_MAX_CHARS_RANGE: (u32, u32) = (10, 200);

/// Błąd ustawień.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("błąd bazy danych: {0}")]
    Db(#[from] DbError),

    #[error("kod PIN musi mieć dokładnie {PIN_LEN} cyfr")]
    InvalidPin,

    #[error("limit „{key}” musi mieścić się w zakresie {min}–{max}, a ustawiono {value}")]
    OutOfRange {
        key: &'static str,
        min: u32,
        max: u32,
        value: u32,
    },
}

impl SettingsError {
    fn out_of_range(key: &'static str, range: (u32, u32), value: u32) -> Self {
        Self::OutOfRange {
            key,
            min: range.0,
            max: range.1,
            value,
        }
    }
}

/// Efektywne ustawienia aplikacji DJ-a.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub pin: String,
    /// Port, na którym aplikacja DJ-a nasłuchuje kiosków.
    pub port: u16,
    pub limits: Limits,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            pin: DEFAULT_PIN.to_string(),
            port: DEFAULT_PORT,
            limits: Limits::default(),
        }
    }
}

impl AppSettings {
    /// Wczytuje ustawienia z bazy.
    ///
    /// Brakujące lub nieparsowalne wpisy zastępujemy wartościami domyślnymi (z ostrzeżeniem w logu),
    /// ale wartość poza dopuszczalnym zakresem to błąd — chcemy o tym wiedzieć, zamiast działać
    /// z limitem, którego nikt się nie spodziewa.
    pub fn load(db: &Db) -> Result<Self, SettingsError> {
        let defaults = Self::default();

        let settings = Self {
            pin: db.setting(KEY_PIN)?.unwrap_or(defaults.pin),
            port: read_port(db, KEY_PORT)?.unwrap_or(defaults.port),
            limits: Limits {
                dedication_max_chars: read_u32(db, KEY_DEDICATION_MAX_CHARS)?
                    .unwrap_or(defaults.limits.dedication_max_chars),
                guest_name_max_chars: read_u32(db, KEY_GUEST_NAME_MAX_CHARS)?
                    .unwrap_or(defaults.limits.guest_name_max_chars),
                search_query_max_chars: read_u32(db, KEY_SEARCH_QUERY_MAX_CHARS)?
                    .unwrap_or(defaults.limits.search_query_max_chars),
            },
        };

        settings.validate()?;

        Ok(settings)
    }

    /// Zapisuje ustawienia po sprawdzeniu ich poprawności.
    pub fn save(&self, db: &Db) -> Result<(), SettingsError> {
        self.validate()?;

        db.set_setting(KEY_PIN, &self.pin)?;
        db.set_setting(KEY_PORT, &self.port.to_string())?;
        db.set_setting(
            KEY_DEDICATION_MAX_CHARS,
            &self.limits.dedication_max_chars.to_string(),
        )?;
        db.set_setting(
            KEY_GUEST_NAME_MAX_CHARS,
            &self.limits.guest_name_max_chars.to_string(),
        )?;
        db.set_setting(
            KEY_SEARCH_QUERY_MAX_CHARS,
            &self.limits.search_query_max_chars.to_string(),
        )?;

        Ok(())
    }

    /// Sprawdza PIN, port i zakresy limitów.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if validate_pin(&self.pin).is_err() {
            return Err(SettingsError::InvalidPin);
        }

        check_port(self.port)?;

        check_range(
            KEY_DEDICATION_MAX_CHARS,
            DEDICATION_MAX_CHARS_RANGE,
            self.limits.dedication_max_chars,
        )?;
        check_range(
            KEY_GUEST_NAME_MAX_CHARS,
            GUEST_NAME_MAX_CHARS_RANGE,
            self.limits.guest_name_max_chars,
        )?;
        check_range(
            KEY_SEARCH_QUERY_MAX_CHARS,
            SEARCH_QUERY_MAX_CHARS_RANGE,
            self.limits.search_query_max_chars,
        )?;

        Ok(())
    }
}

fn check_port(port: u16) -> Result<(), SettingsError> {
    if port < PORT_RANGE.0 || port > PORT_RANGE.1 {
        return Err(SettingsError::OutOfRange {
            key: KEY_PORT,
            min: u32::from(PORT_RANGE.0),
            max: u32::from(PORT_RANGE.1),
            value: u32::from(port),
        });
    }

    Ok(())
}

fn check_range(key: &'static str, range: (u32, u32), value: u32) -> Result<(), SettingsError> {
    if value < range.0 || value > range.1 {
        return Err(SettingsError::out_of_range(key, range, value));
    }

    Ok(())
}

/// Odczyt liczby z ustawień. Wartość nieparsowalna jest traktowana jak brak wpisu,
/// żeby literówka w bazie nie zablokowała aplikacji przed imprezą.
/// Odczyt portu. `read_u32` + zawężenie do `u16` — wpis z bazy nie może wywalić aplikacji.
fn read_port(db: &Db, key: &str) -> Result<Option<u16>, SettingsError> {
    match read_u32(db, key)? {
        Some(value) => match u16::try_from(value) {
            Ok(port) => Ok(Some(port)),
            Err(_) => {
                warn!(key, value, "port spoza zakresu, używam domyślnego");

                Ok(None)
            }
        },
        None => Ok(None),
    }
}

fn read_u32(db: &Db, key: &str) -> Result<Option<u32>, SettingsError> {
    match db.setting(key)? {
        Some(raw) => match raw.trim().parse::<u32>() {
            Ok(value) => Ok(Some(value)),
            Err(error) => {
                warn!(key, raw = %raw, error = %error, "nieparsowalna wartość ustawienia, używam domyślnej");

                Ok(None)
            }
        },
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[test]
    fn defaults_are_valid_and_match_protocol_limits() {
        let settings = AppSettings::default();

        assert!(settings.validate().is_ok());
        assert_eq!(settings.limits, Limits::default());
        assert_eq!(settings.pin, DEFAULT_PIN);
    }

    #[test]
    fn fresh_database_gives_defaults() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        let settings = AppSettings::load(&db).expect("wczytanie ustawień");

        assert_eq!(settings, AppSettings::default());
    }

    #[test]
    fn saved_settings_come_back_unchanged() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        let settings = AppSettings {
            pin: "654321".to_string(),
            port: 9000,
            limits: Limits {
                dedication_max_chars: 300,
                guest_name_max_chars: 25,
                search_query_max_chars: 60,
            },
        };

        settings.save(&db).expect("zapis ustawień");

        assert_eq!(AppSettings::load(&db).expect("odczyt ustawień"), settings);
    }

    #[test]
    fn invalid_pin_is_rejected_on_load_and_save() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        let settings = AppSettings {
            pin: "12345".to_string(),
            port: DEFAULT_PORT,
            limits: Limits::default(),
        };

        assert!(matches!(
            settings.validate(),
            Err(SettingsError::InvalidPin)
        ));
        assert!(matches!(settings.save(&db), Err(SettingsError::InvalidPin)));

        db.set_setting(KEY_PIN, "abc").expect("zapis do bazy");
        assert!(matches!(
            AppSettings::load(&db),
            Err(SettingsError::InvalidPin)
        ));
    }

    #[test]
    fn limits_outside_the_allowed_range_are_rejected() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        let too_short = AppSettings {
            pin: DEFAULT_PIN.to_string(),
            port: DEFAULT_PORT,
            limits: Limits {
                dedication_max_chars: 10,
                ..Limits::default()
            },
        };

        assert!(matches!(
            too_short.validate(),
            Err(SettingsError::OutOfRange {
                key: KEY_DEDICATION_MAX_CHARS,
                ..
            })
        ));

        db.set_setting(KEY_SEARCH_QUERY_MAX_CHARS, "9999")
            .expect("zapis do bazy");

        assert!(matches!(
            AppSettings::load(&db),
            Err(SettingsError::OutOfRange {
                key: KEY_SEARCH_QUERY_MAX_CHARS,
                ..
            })
        ));
    }

    #[test]
    fn port_must_be_within_the_allowed_range() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        let too_low = AppSettings {
            port: 80,
            ..AppSettings::default()
        };

        assert!(matches!(
            too_low.validate(),
            Err(SettingsError::OutOfRange { key: KEY_PORT, .. })
        ));
        assert!(matches!(
            too_low.save(&db),
            Err(SettingsError::OutOfRange { key: KEY_PORT, .. })
        ));

        db.set_setting(KEY_PORT, "70000").expect("zapis do bazy");

        let loaded = AppSettings::load(&db).expect("wczytanie ustawień");

        assert_eq!(loaded.port, DEFAULT_PORT, "port spoza zakresu → domyślny");
    }

    #[test]
    fn unparsable_setting_falls_back_to_the_default() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        db.set_setting(KEY_DEDICATION_MAX_CHARS, "dwieście")
            .expect("zapis do bazy");

        let settings = AppSettings::load(&db).expect("wczytanie ustawień");

        assert_eq!(
            settings.limits.dedication_max_chars,
            Limits::default().dedication_max_chars
        );
    }
}
