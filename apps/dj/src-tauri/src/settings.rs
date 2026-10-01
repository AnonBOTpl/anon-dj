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
use crate::tts::VoiceTuning;

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
/// Klucz ustawienia z czasem powrotu ekranu potwierdzenia w kiosku.
pub const KEY_CONFIRMATION_SECONDS: &str = "kiosk.confirmation_seconds";
/// Klucz ustawienia z wybranym głosem lektora dedykacji.
pub const KEY_TTS_VOICE: &str = "tts.voice";
/// Klucz ustawienia z tempem mowy lektora.
pub const KEY_TTS_LENGTH_SCALE: &str = "tts.length_scale";
/// Klucz ustawienia ze zmiennością barwy lektora.
pub const KEY_TTS_NOISE_SCALE: &str = "tts.noise_scale";
/// Klucz ustawienia ze zmiennością rytmu lektora.
pub const KEY_TTS_NOISE_W: &str = "tts.noise_w";
/// Klucz ustawienia z głośnością lektora.
pub const KEY_TTS_VOLUME: &str = "tts.volume";
/// Klucz ustawienia z pauzą między zdaniami lektora.
pub const KEY_TTS_SENTENCE_SILENCE: &str = "tts.sentence_silence_ms";
/// Klucz ustawienia z urządzeniem wyjściowym voice-overu. Trzymamy **identyfikator**
/// urządzenia, bo przeżywa restart systemu i ponowne podłączenie karty.
pub const KEY_AUDIO_OUTPUT_DEVICE: &str = "audio.output_device_id";
/// Klucz ustawienia z adresem API odtwarzacza (beefweb w foobar2000).
pub const KEY_PLAYER_BASE_URL: &str = "player.base_url";
/// Klucz ustawienia z poziomem ściszenia muzyki pod lektorem (procent amplitudy).
pub const KEY_EXECUTE_DUCK_PERCENT: &str = "execute.duck_percent";
/// Klucz ustawienia z czasem ściszania do poziomu ducku.
pub const KEY_EXECUTE_DUCK_RAMP_MS: &str = "execute.duck_ramp_ms";
/// Klucz ustawienia z pauzą między końcem dedykacji a wyciszeniem starego utworu.
pub const KEY_EXECUTE_GAP_MS: &str = "execute.gap_ms";
/// Klucz ustawienia z czasem wyciszania starego utworu.
pub const KEY_EXECUTE_FADE_OUT_MS: &str = "execute.fade_out_ms";
/// Klucz ustawienia z czasem wchodzenia zamówionego utworu do normalnej głośności.
pub const KEY_EXECUTE_START_RAMP_MS: &str = "execute.start_ramp_ms";
/// Klucz z głośnością normalną odtwarzacza na czas sekwencji wykonania.
///
/// To nie jest ustawienie DJ-a, a **znacznik stanu**: niepusty wpis znaczy „sekwencja właśnie
/// leci”, więc po nagłym zamknięciu aplikacji wiemy, do jakiego poziomu wrócić.
pub const KEY_EXECUTE_NORMAL_VOLUME_DB: &str = "execute.normal_volume_db";

/// PIN, od którego startuje świeża instalacja. DJ może i powinien go zmienić w ustawieniach.
pub const DEFAULT_PIN: &str = "123456";

/// Port serwera kiosków, od którego startuje świeża instalacja. Krótki i rzadko używany,
/// żeby nie zderzyć się z innymi usługami na komputerze DJ-a.
pub const DEFAULT_PORT: u16 = 8790;

/// Dopuszczalny zakres portu. Porty poniżej 1024 wymagają uprawnień administratora,
/// a 0 kazałoby systemowi wybrać port losowy — DJ nie miałby czego wpisać w kiosku.
pub const PORT_RANGE: (u16, u16) = (1024, 65_535);

/// Ile sekund ekran potwierdzenia w kiosku pokazuje się, zanim sam wróci do wyszukiwania.
/// Gość zdąży przeczytać, a kolejna osoba nie czeka — wartość do zmiany w ustawieniach.
pub const DEFAULT_CONFIRMATION_SECONDS: u32 = 20;

/// Dopuszczalny zakres czasu powrotu ekranu potwierdzenia. Poniżej 5 s gość nie zdąży
/// przeczytać, a powyżej 300 s kiosk stoi zablokowany dla kolejnej osoby.
pub const CONFIRMATION_SECONDS_RANGE: (u32, u32) = (5, 300);

/// Głos lektora, od którego startuje świeża instalacja. To tylko preferencja — jeśli wśród
/// zainstalowanych głosów go nie ma, aplikacja użyje pierwszego dostępnego (PLAN.md, sekcja 8).
pub const DEFAULT_TTS_VOICE: &str = "justyna";
/// Najdłuższa dopuszczalna nazwa głosu.
pub const TTS_VOICE_MAX_CHARS: usize = 64;

/// Adres API odtwarzacza, od którego startuje świeża instalacja. To domyślny port beefweb
/// (PLAN.md, sekcja 7) — świeża instalacja działa bez zaglądania do ustawień, o ile foobar2000
/// z beefwebem stoi właśnie tam.
pub const DEFAULT_PLAYER_BASE_URL: &str = "http://localhost:8880";
/// Najdłuższy dopuszczalny adres API odtwarzacza. Adres wpisuje DJ, więc ograniczamy go,
/// żeby literówka nie trafiła do klienta HTTP jako kilometrowy ciąg.
pub const PLAYER_BASE_URL_MAX_CHARS: usize = 200;

/// Najdłuższa dopuszczalna nazwa urządzenia wyjściowego. Nazwy kart dźwiękowych bywają długie
/// (producent, model, tryb), ale nie aż tak — a wpis bez ograniczenia to zaproszenie do błędu.
pub const AUDIO_DEVICE_MAX_CHARS: usize = 128;

/// Na jaką część głośności ściszamy muzykę pod lektorem, w procentach amplitudy.
/// Zapas na jedną trzecią — utwór ma być słyszalny pod głosem, a nie zniknąć.
pub const DEFAULT_DUCK_PERCENT: u32 = 33;
/// Domyślny czas ściszania do poziomu ducku.
pub const DEFAULT_DUCK_RAMP_MS: u32 = 600;
/// Domyślna pauza między końcem dedykacji a wyciszeniem starego utworu.
pub const DEFAULT_GAP_MS: u32 = 300;
/// Domyślny czas wyciszania starego utworu.
pub const DEFAULT_FADE_OUT_MS: u32 = 400;
/// Domyślny czas wchodzenia zamówionego utworu do normalnej głośności.
pub const DEFAULT_START_RAMP_MS: u32 = 800;

/// Zakres ściszenia pod lektorem. 0% to cisza, 100% to brak ściszenia.
pub const DUCK_PERCENT_RANGE: (u32, u32) = (0, 100);
/// Zakres czasu pojedynczego przejścia głośności. Zero oznacza skok, a przy imprezie liczy się
/// każda sekunda — dlatego górna granica jest krótka, a nie „na wszelki wypadek” długa.
pub const EXECUTE_MS_RANGE: (u32, u32) = (0, 5_000);

/// Zakres tempa mowy w promilach: 500 = pół tempa, 1000 = normalne, 2000 = dwa razy wolniej.
/// Powyżej 2000 lektor ciągnie tak, że dedykacja przestaje mieścić się w utworze.
pub const LENGTH_SCALE_MILLI_RANGE: (u32, u32) = (500, 2_000);
/// Zakres zmienności barwy (0 = monotomy, 1500 = maksymalna z modeli Piper).
pub const NOISE_SCALE_MILLI_RANGE: (u32, u32) = (0, 1_500);
/// Zakres zmienności rytmu.
pub const NOISE_W_MILLI_RANGE: (u32, u32) = (0, 1_500);
/// Zakres głośności lektora w procentach (0 = cisza, 200 = dwa razy głośniej).
pub const VOLUME_PERCENT_RANGE: (u32, u32) = (0, 200);
/// Zakres pauzy między zdaniami.
pub const SENTENCE_SILENCE_MS_RANGE: (u32, u32) = (0, 2_000);

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

    #[error("nazwa głosu lektora jest nieprawidłowa: „{0}”")]
    InvalidVoice(String),

    #[error("nazwa urządzenia wyjściowego jest za długa (limit {AUDIO_DEVICE_MAX_CHARS} znaków)")]
    InvalidAudioDevice,

    #[error("adres odtwarzacza musi zaczynać się od `http://` i nie zawierać spacji: „{0}”")]
    InvalidPlayerUrl(String),
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

/// Ustawienia lektora: wybrany głos i jego strojenie.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TtsSettings {
    /// Identyfikator głosu spośród zainstalowanych lokalnie (np. `justyna`).
    pub voice: String,
    pub tuning: VoiceTuning,
}

impl Default for TtsSettings {
    fn default() -> Self {
        Self {
            voice: DEFAULT_TTS_VOICE.to_string(),
            tuning: VoiceTuning::default(),
        }
    }
}

/// Ustawienia dźwięku: gdzie ma wyjść voice-over.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AudioSettings {
    /// Identyfikator urządzenia wyjściowego. **Pusty oznacza domyślne urządzenie systemowe** —
    /// i tak startuje świeża instalacja, żeby aplikacja działała bez zaglądania do ustawień.
    pub output_device_id: String,
}

/// Ustawienia odtwarzacza: gdzie stoi API beefweb.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlayerSettings {
    /// Bazowy adres API, np. `http://localhost:8880`. Bez ukośnika na końcu.
    pub base_url: String,
}

impl Default for PlayerSettings {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_PLAYER_BASE_URL.to_string(),
        }
    }
}

/// Ustawienia sekwencji wykonania (PLAN.md, sekcja 7): jak muzyka zachowuje się pod lektorem.
///
/// Wszystkie czasy to milisekundy — sekwencja musi umieć zagrać się w mgnieniu oka, więc jednostka
/// sekund byłaby zbyt gruba. Wartości są do strojenia na żywo; domyślne są bezpieczne.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExecutionSettings {
    /// Poziom ducku w procentach amplitudy (patrz [`DEFAULT_DUCK_PERCENT`]).
    pub duck_percent: u32,
    /// Jak długo ściszamy muzykę do poziomu ducku.
    pub duck_ramp_ms: u32,
    /// Pauza po dedykacji, zanim zaczniemy wyciszać stary utwór.
    pub gap_ms: u32,
    /// Jak długo wyciszamy stary utwór do zera.
    pub fade_out_ms: u32,
    /// Jak długo zamówiony utwór wjeżdża do normalnej głośności.
    pub start_ramp_ms: u32,
}

impl Default for ExecutionSettings {
    fn default() -> Self {
        Self {
            duck_percent: DEFAULT_DUCK_PERCENT,
            duck_ramp_ms: DEFAULT_DUCK_RAMP_MS,
            gap_ms: DEFAULT_GAP_MS,
            fade_out_ms: DEFAULT_FADE_OUT_MS,
            start_ramp_ms: DEFAULT_START_RAMP_MS,
        }
    }
}

/// Efektywne ustawienia aplikacji DJ-a.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub pin: String,
    /// Port, na którym aplikacja DJ-a nasłuchuje kiosków.
    pub port: u16,
    /// Po ilu sekundach ekran potwierdzenia w kiosku wraca sam do wyszukiwania.
    pub confirmation_seconds: u32,
    pub limits: Limits,
    /// Lektor dedykacji. `default` sprawia, że starszy interfejs bez tych pól nadal zapisze
    /// ustawienia bez błędu.
    #[serde(default)]
    pub tts: TtsSettings,
    /// Gdzie ma wyjść voice-over. `default` — starszy interfejs bez tego pola nadal zapisze
    /// ustawienia, i to z domyślnym urządzeniem systemowym.
    #[serde(default)]
    pub audio: AudioSettings,
    /// Odtwarzacz (foobar2000 + beefweb). `default` — starszy interfejs bez tego pola nadal
    /// zapisze ustawienia, i to z domyślnym adresem.
    #[serde(default)]
    pub player: PlayerSettings,
    /// Jak zachowuje się muzyka pod lektorem. `default` — starszy interfejs nadal zapisze
    /// ustawienia, i to z bezpiecznymi wartościami sekwencji.
    #[serde(default)]
    pub execute: ExecutionSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            pin: DEFAULT_PIN.to_string(),
            port: DEFAULT_PORT,
            confirmation_seconds: DEFAULT_CONFIRMATION_SECONDS,
            limits: Limits::default(),
            tts: TtsSettings::default(),
            audio: AudioSettings::default(),
            player: PlayerSettings::default(),
            execute: ExecutionSettings::default(),
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
            confirmation_seconds: read_u32(db, KEY_CONFIRMATION_SECONDS)?
                .unwrap_or(defaults.confirmation_seconds),
            limits: Limits {
                dedication_max_chars: read_u32(db, KEY_DEDICATION_MAX_CHARS)?
                    .unwrap_or(defaults.limits.dedication_max_chars),
                guest_name_max_chars: read_u32(db, KEY_GUEST_NAME_MAX_CHARS)?
                    .unwrap_or(defaults.limits.guest_name_max_chars),
                search_query_max_chars: read_u32(db, KEY_SEARCH_QUERY_MAX_CHARS)?
                    .unwrap_or(defaults.limits.search_query_max_chars),
            },
            tts: TtsSettings {
                voice: db.setting(KEY_TTS_VOICE)?.unwrap_or(defaults.tts.voice),
                tuning: VoiceTuning {
                    length_scale_milli: read_u32(db, KEY_TTS_LENGTH_SCALE)?
                        .unwrap_or(defaults.tts.tuning.length_scale_milli),
                    noise_scale_milli: read_u32(db, KEY_TTS_NOISE_SCALE)?
                        .unwrap_or(defaults.tts.tuning.noise_scale_milli),
                    noise_w_milli: read_u32(db, KEY_TTS_NOISE_W)?
                        .unwrap_or(defaults.tts.tuning.noise_w_milli),
                    volume_percent: read_u32(db, KEY_TTS_VOLUME)?
                        .unwrap_or(defaults.tts.tuning.volume_percent),
                    sentence_silence_ms: read_u32(db, KEY_TTS_SENTENCE_SILENCE)?
                        .unwrap_or(defaults.tts.tuning.sentence_silence_ms),
                },
            },
            audio: AudioSettings {
                output_device_id: db
                    .setting(KEY_AUDIO_OUTPUT_DEVICE)?
                    .unwrap_or(defaults.audio.output_device_id),
            },
            player: PlayerSettings {
                base_url: db
                    .setting(KEY_PLAYER_BASE_URL)?
                    .unwrap_or(defaults.player.base_url),
            },
            execute: ExecutionSettings {
                duck_percent: read_u32(db, KEY_EXECUTE_DUCK_PERCENT)?
                    .unwrap_or(defaults.execute.duck_percent),
                duck_ramp_ms: read_u32(db, KEY_EXECUTE_DUCK_RAMP_MS)?
                    .unwrap_or(defaults.execute.duck_ramp_ms),
                gap_ms: read_u32(db, KEY_EXECUTE_GAP_MS)?.unwrap_or(defaults.execute.gap_ms),
                fade_out_ms: read_u32(db, KEY_EXECUTE_FADE_OUT_MS)?
                    .unwrap_or(defaults.execute.fade_out_ms),
                start_ramp_ms: read_u32(db, KEY_EXECUTE_START_RAMP_MS)?
                    .unwrap_or(defaults.execute.start_ramp_ms),
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
            KEY_CONFIRMATION_SECONDS,
            &self.confirmation_seconds.to_string(),
        )?;
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

        db.set_setting(KEY_TTS_VOICE, &self.tts.voice)?;
        db.set_setting(
            KEY_TTS_LENGTH_SCALE,
            &self.tts.tuning.length_scale_milli.to_string(),
        )?;
        db.set_setting(
            KEY_TTS_NOISE_SCALE,
            &self.tts.tuning.noise_scale_milli.to_string(),
        )?;
        db.set_setting(KEY_TTS_NOISE_W, &self.tts.tuning.noise_w_milli.to_string())?;
        db.set_setting(KEY_TTS_VOLUME, &self.tts.tuning.volume_percent.to_string())?;
        db.set_setting(
            KEY_TTS_SENTENCE_SILENCE,
            &self.tts.tuning.sentence_silence_ms.to_string(),
        )?;

        db.set_setting(KEY_AUDIO_OUTPUT_DEVICE, &self.audio.output_device_id)?;
        db.set_setting(KEY_PLAYER_BASE_URL, self.player.base_url.trim())?;

        db.set_setting(
            KEY_EXECUTE_DUCK_PERCENT,
            &self.execute.duck_percent.to_string(),
        )?;
        db.set_setting(
            KEY_EXECUTE_DUCK_RAMP_MS,
            &self.execute.duck_ramp_ms.to_string(),
        )?;
        db.set_setting(KEY_EXECUTE_GAP_MS, &self.execute.gap_ms.to_string())?;
        db.set_setting(
            KEY_EXECUTE_FADE_OUT_MS,
            &self.execute.fade_out_ms.to_string(),
        )?;
        db.set_setting(
            KEY_EXECUTE_START_RAMP_MS,
            &self.execute.start_ramp_ms.to_string(),
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
            KEY_CONFIRMATION_SECONDS,
            CONFIRMATION_SECONDS_RANGE,
            self.confirmation_seconds,
        )?;
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

        validate_tts(&self.tts.voice, &self.tts.tuning)?;

        check_audio_device(&self.audio.output_device_id)?;

        check_player_url(&self.player.base_url)?;

        check_range(
            KEY_EXECUTE_DUCK_PERCENT,
            DUCK_PERCENT_RANGE,
            self.execute.duck_percent,
        )?;
        check_range(
            KEY_EXECUTE_DUCK_RAMP_MS,
            EXECUTE_MS_RANGE,
            self.execute.duck_ramp_ms,
        )?;
        check_range(KEY_EXECUTE_GAP_MS, EXECUTE_MS_RANGE, self.execute.gap_ms)?;
        check_range(
            KEY_EXECUTE_FADE_OUT_MS,
            EXECUTE_MS_RANGE,
            self.execute.fade_out_ms,
        )?;
        check_range(
            KEY_EXECUTE_START_RAMP_MS,
            EXECUTE_MS_RANGE,
            self.execute.start_ramp_ms,
        )?;

        Ok(())
    }
}

/// Sprawdza sam wybór głosu i jego strojenie.
///
/// Wydzielone, bo ekran lektora odsłuchuje próbkę **przed** zapisem: podgląd musi odrzucić
/// głos albo wartość spoza zakresu tym samym kodem, co zapis ustawień — inaczej dałoby się
/// posłuchać czegoś, czego potem nie da się zapisać.
pub fn validate_tts(voice: &str, tuning: &VoiceTuning) -> Result<(), SettingsError> {
    check_voice(voice)?;
    check_range(
        KEY_TTS_LENGTH_SCALE,
        LENGTH_SCALE_MILLI_RANGE,
        tuning.length_scale_milli,
    )?;
    check_range(
        KEY_TTS_NOISE_SCALE,
        NOISE_SCALE_MILLI_RANGE,
        tuning.noise_scale_milli,
    )?;
    check_range(KEY_TTS_NOISE_W, NOISE_W_MILLI_RANGE, tuning.noise_w_milli)?;
    check_range(KEY_TTS_VOLUME, VOLUME_PERCENT_RANGE, tuning.volume_percent)?;
    check_range(
        KEY_TTS_SENTENCE_SILENCE,
        SENTENCE_SILENCE_MS_RANGE,
        tuning.sentence_silence_ms,
    )?;

    Ok(())
}

/// Identyfikator urządzenia wyjściowego trafia prosto do interfejsu audio, więc ograniczamy jego
/// długość. Pusty jest w porządku — to znaczy „domyślne urządzenie systemowe”.
fn check_audio_device(device_id: &str) -> Result<(), SettingsError> {
    if device_id.chars().count() > AUDIO_DEVICE_MAX_CHARS {
        return Err(SettingsError::InvalidAudioDevice);
    }

    Ok(())
}

/// Adres API odtwarzacza trafia prosto do klienta HTTP. Wymagamy `http://`, bo beefweb stoi
/// lokalnie i nie ma tam czego szyfrować — a klient HTTP jest zbudowany bez obsługi TLS,
/// więc `https://` i tak by nie zadziałał. Adres z literówką lepiej odrzucić przy zapisie, niż
/// pokazać DJ-owi „brak połączenia z odtwarzaczem” w środku imprezy.
fn check_player_url(base_url: &str) -> Result<(), SettingsError> {
    let trimmed = base_url.trim();

    let looks_like_url = !trimmed.is_empty()
        && trimmed.chars().count() <= PLAYER_BASE_URL_MAX_CHARS
        && !trimmed
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        && trimmed.starts_with("http://")
        && trimmed.len() > "http://".len();

    if !looks_like_url {
        return Err(SettingsError::InvalidPlayerUrl(trimmed.to_string()));
    }

    Ok(())
}

/// Nazwa głosu musi być identyfikatorem pliku — trafia potem do ścieżek i klucza pamięci
/// podręcznej, więc nie może zawierać spacji ani separatorów.
fn check_voice(voice: &str) -> Result<(), SettingsError> {
    let voice = voice.trim();

    let looks_like_id = !voice.is_empty()
        && voice.chars().count() <= TTS_VOICE_MAX_CHARS
        && voice
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'));

    if !looks_like_id {
        return Err(SettingsError::InvalidVoice(voice.to_string()));
    }

    Ok(())
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
    use crate::tts::VoiceTuning;

    #[test]
    fn defaults_are_valid_and_match_protocol_limits() {
        let settings = AppSettings::default();

        assert!(settings.validate().is_ok());
        assert_eq!(settings.limits, Limits::default());
        assert_eq!(settings.pin, DEFAULT_PIN);
        assert_eq!(
            settings.confirmation_seconds,
            protocol::DEFAULT_CONFIRMATION_SECONDS,
            "domyślny czas potwierdzenia musi zgadzać się z protokołem"
        );
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
            confirmation_seconds: 45,
            limits: Limits {
                dedication_max_chars: 300,
                guest_name_max_chars: 25,
                search_query_max_chars: 60,
            },
            tts: TtsSettings {
                voice: "pl_PL-jarvis_wg_glos-medium".to_string(),
                tuning: VoiceTuning {
                    length_scale_milli: 1_100,
                    noise_scale_milli: 700,
                    noise_w_milli: 750,
                    volume_percent: 90,
                    sentence_silence_ms: 150,
                },
            },
            audio: AudioSettings {
                output_device_id: "wasapi:Głośniki (Realtek Audio)".to_string(),
            },
            player: PlayerSettings {
                base_url: "http://localhost:8880".to_string(),
            },
            execute: ExecutionSettings {
                duck_percent: 25,
                duck_ramp_ms: 700,
                gap_ms: 250,
                fade_out_ms: 500,
                start_ramp_ms: 900,
            },
        };

        settings.save(&db).expect("zapis ustawień");

        assert_eq!(AppSettings::load(&db).expect("odczyt ustawień"), settings);
    }

    #[test]
    fn the_execution_sequence_stays_within_sane_times() {
        let mut settings = AppSettings::default();
        assert!(settings.validate().is_ok());

        settings.execute.duck_percent = 101;
        assert!(settings.validate().is_err(), "duck ponad 100% amplitudy");

        settings.execute = ExecutionSettings::default();
        settings.execute.fade_out_ms = 5_001;
        assert!(settings.validate().is_err(), "wyciszanie dłuższe niż 5 s");

        settings.execute = ExecutionSettings::default();
        settings.execute.duck_percent = 0;
        assert!(
            settings.validate().is_ok(),
            "cisza pod lektorem jest dopuszczalnym wyborem DJ-a"
        );
    }

    #[test]
    fn the_player_address_must_be_a_local_http_url() {
        let mut settings = AppSettings::default();
        assert!(settings.validate().is_ok());

        settings.player.base_url = "localhost:8880".to_string();
        assert!(settings.validate().is_err(), "bez schematu to nie adres");

        settings.player.base_url = "https://localhost:8880".to_string();
        assert!(settings.validate().is_err(), "klient nie obsługuje TLS");

        settings.player.base_url = "http://local host:8880".to_string();
        assert!(settings.validate().is_err(), "spacja w adresie");

        settings.player.base_url = format!("http://{}", "a".repeat(300));
        assert!(settings.validate().is_err(), "adres ponad limit");

        settings.player.base_url = "http://127.0.0.1:8880/".to_string();
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn the_player_url_is_saved_without_surrounding_spaces() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        let mut settings = AppSettings::default();
        settings.player.base_url = "  http://127.0.0.1:8880  ".to_string();
        settings.save(&db).expect("zapis ustawień");

        let loaded = AppSettings::load(&db).expect("odczyt ustawień");

        assert_eq!(loaded.player.base_url, "http://127.0.0.1:8880");
    }

    #[test]
    fn a_fresh_install_plays_the_voice_over_on_the_default_device() {
        let settings = AppSettings::default();

        assert!(settings.validate().is_ok());
        assert_eq!(settings.audio.output_device_id, "");
    }

    #[test]
    fn an_older_interface_without_audio_fields_still_saves() {
        let settings: AppSettings = serde_json::from_str(
            r#"{
                "pin": "123456",
                "port": 8790,
                "confirmation_seconds": 20,
                "limits": {
                    "dedication_max_chars": 400,
                    "guest_name_max_chars": 40,
                    "search_query_max_chars": 80
                }
            }"#,
        )
        .expect("starszy interfejs bez pola audio");

        assert_eq!(settings.audio, AudioSettings::default());
        assert_eq!(settings.player, PlayerSettings::default());
        assert_eq!(settings.execute, ExecutionSettings::default());
    }

    #[test]
    fn an_absurdly_long_device_name_is_rejected() {
        let settings = AppSettings {
            audio: AudioSettings {
                output_device_id: "x".repeat(AUDIO_DEVICE_MAX_CHARS + 1),
            },
            ..AppSettings::default()
        };

        assert!(matches!(
            settings.validate(),
            Err(SettingsError::InvalidAudioDevice)
        ));
    }

    #[test]
    fn tts_defaults_are_valid_and_pick_a_voice() {
        let settings = AppSettings::default();

        assert!(settings.validate().is_ok());
        assert_eq!(settings.tts.voice, DEFAULT_TTS_VOICE);
        assert_eq!(settings.tts.tuning, VoiceTuning::default());
    }

    #[test]
    fn tts_tuning_outside_the_allowed_range_is_rejected() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        let too_slow = AppSettings {
            tts: TtsSettings {
                tuning: VoiceTuning {
                    length_scale_milli: 100,
                    ..VoiceTuning::default()
                },
                ..TtsSettings::default()
            },
            ..AppSettings::default()
        };

        assert!(matches!(
            too_slow.validate(),
            Err(SettingsError::OutOfRange {
                key: KEY_TTS_LENGTH_SCALE,
                ..
            })
        ));
        assert!(matches!(
            too_slow.save(&db),
            Err(SettingsError::OutOfRange {
                key: KEY_TTS_LENGTH_SCALE,
                ..
            })
        ));

        db.set_setting(KEY_TTS_VOLUME, "500").expect("zapis");

        assert!(matches!(
            AppSettings::load(&db),
            Err(SettingsError::OutOfRange {
                key: KEY_TTS_VOLUME,
                ..
            })
        ));
    }

    #[test]
    fn the_preview_validates_the_voice_and_tuning_like_a_save_does() {
        let tuning = VoiceTuning::default();

        assert!(validate_tts("justyna", &tuning).is_ok());
        assert!(matches!(
            validate_tts("justyna/../wzorowy", &tuning),
            Err(SettingsError::InvalidVoice(_))
        ));

        let too_fast = VoiceTuning {
            length_scale_milli: 5_000,
            ..tuning
        };

        assert!(matches!(
            validate_tts("justyna", &too_fast),
            Err(SettingsError::OutOfRange {
                key: KEY_TTS_LENGTH_SCALE,
                ..
            })
        ));
    }

    #[test]
    fn an_invalid_voice_name_is_rejected() {
        let settings = AppSettings {
            tts: TtsSettings {
                voice: "justyna/../wzorowy".to_string(),
                ..TtsSettings::default()
            },
            ..AppSettings::default()
        };

        assert!(matches!(
            settings.validate(),
            Err(SettingsError::InvalidVoice(_))
        ));
    }

    #[test]
    fn an_older_interface_without_tts_fields_still_saves() {
        let settings: AppSettings = serde_json::from_str(
            r#"{
                "pin": "123456",
                "port": 8790,
                "confirmation_seconds": 20,
                "limits": {
                    "dedication_max_chars": 400,
                    "guest_name_max_chars": 40,
                    "search_query_max_chars": 80
                }
            }"#,
        )
        .expect("starszy interfejs bez pól TTS");

        assert_eq!(settings.tts, TtsSettings::default());
    }

    #[test]
    fn invalid_pin_is_rejected_on_load_and_save() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        let settings = AppSettings {
            pin: "12345".to_string(),
            ..AppSettings::default()
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
            limits: Limits {
                dedication_max_chars: 10,
                ..Limits::default()
            },
            ..AppSettings::default()
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
    fn confirmation_seconds_must_be_within_the_allowed_range() {
        let db = Db::open_in_memory().expect("baza w pamięci");

        let too_short = AppSettings {
            confirmation_seconds: 1,
            ..AppSettings::default()
        };

        assert!(matches!(
            too_short.validate(),
            Err(SettingsError::OutOfRange {
                key: KEY_CONFIRMATION_SECONDS,
                ..
            })
        ));
        assert!(matches!(
            too_short.save(&db),
            Err(SettingsError::OutOfRange {
                key: KEY_CONFIRMATION_SECONDS,
                ..
            })
        ));

        db.set_setting(KEY_CONFIRMATION_SECONDS, "9999")
            .expect("zapis do bazy");

        assert!(matches!(
            AppSettings::load(&db),
            Err(SettingsError::OutOfRange {
                key: KEY_CONFIRMATION_SECONDS,
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
