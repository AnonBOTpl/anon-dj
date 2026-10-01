//! Odtwarzanie klipu lektora na wybranym urządzeniu wyjściowym (PLAN.md, sekcja 8).
//!
//! DJ musi usłyszeć dedykację, zanim puści ją na antenę, i musi ona wyjść **tym samym wyjściem,
//! co muzyka** — inaczej gość usłyszy lektora z innych głośników niż utwór. Dlatego urządzenie
//! wyjściowe jest ustawieniem, a nie czymś, co wybiera system.
//!
//! Sam dźwięk to cienka warstwa nad `rodio`: otwarcie strumienia i podanie pliku WAV. Logikę,
//! którą da się sprawdzić bez sprzętu (głośność, nazwa urządzenia), trzymamy osobno i testujemy.
//!
//! Zapamiętujemy **identyfikator** urządzenia, a nie jego nazwę: identyfikator przeżywa restart
//! systemu i ponowne podłączenie karty, więc wybór DJ-a nie przepada po restarcie imprezy.
//!
//! Odtwarzamy zawsze jedną rzecz naraz — kolejna prośba milknie poprzednią. Kiedy klip się kończy,
//! watcher zwalnia strumień i wysyła zdarzenie, po którym interfejs gasi przycisk.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::cpal::{self};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};

/// Zdarzenie o tym, czyj voice-over leci teraz (`None` = nic nie leci).
pub const EVENT_PLAYBACK: &str = "audio://playback";

/// Co ile sprawdzamy, czy klip dobiegł końca. Krócej niż 100 ms to marnowanie procesora,
/// dłużej niż 300 ms to przycisk gaśnie zauważalnie po dźwięku.
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Błąd odtwarzania voice-overu.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("nie udało się wypisać urządzeń wyjściowych: {0}")]
    Devices(String),

    #[error("wybrane urządzenie wyjściowe jest niedostępne — wybierz inne w ustawieniach")]
    DeviceNotFound,

    #[error("nie udało się otworzyć urządzenia wyjściowego: {0}")]
    Device(String),

    #[error("nie udało się odczytać klipu {path}: {source}")]
    Clip {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("nie udało się zdekodować klipu {path}: {message}")]
    Decode { path: PathBuf, message: String },

    #[error("odtwarzacz jest chwilowo niedostępny")]
    Unavailable,
}

/// Jedno urządzenie wyjściowe widziane przez DJ-a.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OutputDevice {
    /// Identyfikator urządzenia — to zapisujemy w ustawieniach. Nie pokazujemy go DJ-owi.
    pub id: String,
    /// Czytelna nazwa urządzenia, np. `Głośniki (Realtek Audio)`.
    pub name: String,
    /// Czy to domyślne urządzenie systemowe.
    pub is_default: bool,
}

/// Co odtwarzamy — voice-over prośby z kolejki albo próbkę głosu z ekranu lektora.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackTarget {
    /// Odsłuch dedykacji prośby.
    Request(i64),
    /// Odsłuch próbki głosu — DJ porównuje tak głosy i strojenie, zanim je zapisze.
    Preview,
}

/// Stan odtwarzania wysyłany do interfejsu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct PlaybackStatus {
    /// Prośba, której voice-overu słuchamy; `None`, gdy nic nie leci albo leci próbka głosu.
    pub request_id: Option<i64>,
    /// Czy leci próbka głosu z ekranu lektora.
    pub preview: bool,
}

impl PlaybackStatus {
    /// Nic nie leci.
    fn stopped() -> Self {
        Self {
            request_id: None,
            preview: false,
        }
    }

    fn playing(target: PlaybackTarget) -> Self {
        match target {
            PlaybackTarget::Request(request_id) => Self {
                request_id: Some(request_id),
                preview: false,
            },
            PlaybackTarget::Preview => Self {
                request_id: None,
                preview: true,
            },
        }
    }
}

/// Lista urządzeń wyjściowych, alfabetycznie po nazwie.
///
/// Urządzenie, którego nazwy lub identyfikatora nie da się odczytać, pomijamy z ostrzeżeniem —
/// jedno zepsute urządzenie nie może ukryć przed DJ-em pozostałych.
pub fn output_devices() -> Result<Vec<OutputDevice>, AudioError> {
    let host = cpal::default_host();
    let default_id = host
        .default_output_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());

    let devices = host
        .output_devices()
        .map_err(|error| AudioError::Devices(error.to_string()))?;

    let mut listed = Vec::new();

    for device in devices {
        let Ok(id) = device.id() else {
            warn!("pomijam urządzenie wyjściowe bez identyfikatora");

            continue;
        };

        let Ok(description) = device.description() else {
            warn!("pomijam urządzenie wyjściowe bez opisu");

            continue;
        };

        let id = id.to_string();

        listed.push(OutputDevice {
            is_default: default_id.as_deref() == Some(id.as_str()),
            name: description.name().to_string(),
            id,
        });
    }

    listed.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(listed)
}

/// Głośność lektora jako mnożnik dla odtwarzacza (100% to 1.0, czyli dźwięk bez zmian).
pub fn playback_volume(volume_percent: u32) -> f32 {
    volume_percent as f32 / 100.0
}

/// Identyfikator urządzenia bez białych znaków; pusty oznacza „domyślne systemowe”.
pub fn normalize_device_id(device_id: &str) -> &str {
    device_id.trim()
}

/// Aktywne odtwarzanie.
struct Active {
    /// Numer odtwarzania — watcher starszego nie może wygasić nowszego.
    serial: u64,
    /// Trzyma strumień przy życiu; po zwolnieniu dźwięk milknie.
    _sink: MixerDeviceSink,
    /// Kolejka odtwarzania. Musi żyć razem ze strumieniem: zwolnienie odtwarzacza ucina dźwięk,
    /// a jawnie opróżniona kolejka pozwala watcherowi od razu poznać, że nic już nie gra.
    player: Arc<Player>,
}

/// Odtwarzacz voice-overu. Trzyma najwyżej jedno odtwarzanie naraz.
pub struct VoiceOverPlayer {
    active: Mutex<Option<Active>>,
    serial: AtomicU64,
}

impl Default for VoiceOverPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceOverPlayer {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(None),
            serial: AtomicU64::new(0),
        }
    }

    /// Odtwarza klip na wskazanym urządzeniu. Poprzednie odtwarzanie milknie.
    pub fn play(
        self: &Arc<Self>,
        app: &AppHandle,
        target: PlaybackTarget,
        path: &Path,
        device_id: &str,
        volume: f32,
    ) -> Result<(), AudioError> {
        let sink = open_sink(device_id)?;

        let file = std::fs::File::open(path).map_err(|source| AudioError::Clip {
            path: path.to_path_buf(),
            source,
        })?;
        let source = Decoder::try_from(file).map_err(|error| AudioError::Decode {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

        let player = Arc::new(Player::connect_new(sink.mixer()));
        player.set_volume(volume);
        player.append(source);

        let serial = self.serial.fetch_add(1, Ordering::SeqCst) + 1;

        // Podmiana odtwarzania: stare strumienie (razem ze swoim klipem) milkną tutaj.
        match self.active.lock() {
            Ok(mut active) => {
                *active = Some(Active {
                    serial,
                    _sink: sink,
                    player: Arc::clone(&player),
                });
            }
            Err(_) => return Err(AudioError::Unavailable),
        }

        info!(
            request_id = target.request_id(),
            preview = matches!(target, PlaybackTarget::Preview),
            "odtwarzam voice-over"
        );
        emit_playback(app, PlaybackStatus::playing(target));
        self.watch(app.clone(), serial, player);

        Ok(())
    }

    /// Zatrzymuje odtwarzanie (jeśli coś leci).
    pub fn stop(&self, app: &AppHandle) {
        let stopped = match self.active.lock() {
            Ok(mut active) => active.take(),
            Err(_) => None,
        };

        if let Some(active) = stopped {
            // Opróżniamy kolejkę jawnie, żeby watcher nie czekał na klatki po wyciszeniu.
            active.player.stop();

            info!("voice-over zatrzymany");
            emit_playback(app, PlaybackStatus::stopped());
        }
    }

    /// Czeka, aż klip dobiegnie końca, i wtedy zwalnia strumień.
    fn watch(self: &Arc<Self>, app: AppHandle, serial: u64, player: Arc<Player>) {
        // Referencja do odtwarzacza nie może wyjść poza metodę, więc watcher dostaje własny `Arc`.
        let jobs = Arc::clone(self);

        std::thread::spawn(move || {
            // Pierwsze sprawdzenie od razu po starcie mogłoby trafić w moment, gdy źródło nie
            // jest jeszcze w kolejce — dlatego dajemy odtwarzaczowi chwilę na rozruch.
            std::thread::sleep(POLL_INTERVAL);

            while !player.empty() {
                std::thread::sleep(POLL_INTERVAL);
            }

            // Ktoś mógł w międzyczasie puścić coś nowszego — wtedy nie wygaszamy interfejsu.
            if jobs.finish(serial) {
                emit_playback(&app, PlaybackStatus::stopped());
            }
        });
    }

    /// Zwalnia odtwarzanie o podanym numerze. `false` oznacza, że to już nieaktualne odtwarzanie.
    fn finish(&self, serial: u64) -> bool {
        match self.active.lock() {
            Ok(mut active) => {
                let is_current = active.as_ref().map(|current| current.serial) == Some(serial);

                if is_current {
                    *active = None;
                }

                is_current
            }
            Err(_) => false,
        }
    }
}

/// Otwiera strumień wyjściowy. Pusty identyfikator oznacza domyślne urządzenie systemowe.
fn open_sink(device_id: &str) -> Result<MixerDeviceSink, AudioError> {
    let requested = normalize_device_id(device_id);

    if requested.is_empty() {
        return DeviceSinkBuilder::default()
            .with_error_callback(log_stream_error)
            .open_sink_or_fallback()
            .map_err(|error| AudioError::Device(error.to_string()));
    }

    let host = cpal::default_host();
    let device = host
        .output_devices()
        .map_err(|error| AudioError::Devices(error.to_string()))?
        .find(|device| device.id().ok().map(|id| id.to_string()).as_deref() == Some(requested))
        .ok_or(AudioError::DeviceNotFound)?;

    DeviceSinkBuilder::from_device(device)
        .map_err(|error| AudioError::Device(error.to_string()))?
        .with_error_callback(log_stream_error)
        .open_sink_or_fallback()
        .map_err(|error| AudioError::Device(error.to_string()))
}

/// Błąd strumienia audio trafia do logu. Aplikacja nie może zginąć w środku imprezy.
fn log_stream_error(error: cpal::StreamError) {
    warn!(error = %error, "błąd strumienia audio");
}

impl PlaybackTarget {
    /// Prośba, której dotyczy odtwarzanie; próbka głosu nie należy do żadnej prośby.
    fn request_id(self) -> Option<i64> {
        match self {
            Self::Request(request_id) => Some(request_id),
            Self::Preview => None,
        }
    }
}

fn emit_playback(app: &AppHandle, status: PlaybackStatus) {
    if let Err(error) = app.emit(EVENT_PLAYBACK, status) {
        warn!(error = %error, "nie udało się wysłać stanu odtwarzania");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_percent_becomes_a_multiplier() {
        assert!((playback_volume(100) - 1.0).abs() < f32::EPSILON);
        assert!((playback_volume(0) - 0.0).abs() < f32::EPSILON);
        assert!((playback_volume(150) - 1.5).abs() < f32::EPSILON);
        assert!((playback_volume(80) - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn a_blank_device_id_means_the_system_default() {
        assert_eq!(normalize_device_id(""), "");
        assert_eq!(normalize_device_id("   "), "");
        assert_eq!(normalize_device_id(" WASAPI:abc "), "WASAPI:abc");
    }

    #[test]
    fn a_preview_is_reported_without_a_request_id() {
        assert_eq!(
            PlaybackStatus::playing(PlaybackTarget::Request(7)),
            PlaybackStatus {
                request_id: Some(7),
                preview: false
            }
        );
        assert_eq!(
            PlaybackStatus::playing(PlaybackTarget::Preview),
            PlaybackStatus {
                request_id: None,
                preview: true
            },
            "próbka głosu nie należy do żadnej prośby, a interfejs musi wiedzieć, że coś leci"
        );
        assert_eq!(
            PlaybackStatus::stopped(),
            PlaybackStatus {
                request_id: None,
                preview: false
            }
        );
    }

    #[test]
    fn nothing_finishes_when_no_playback_was_started() {
        let player = VoiceOverPlayer::new();

        assert!(
            !player.finish(1),
            "bez uruchomionego odtwarzania nie ma czego zwalniać"
        );
        assert!(
            !player.finish(2),
            "nieaktualny numer odtwarzania nie może nic zwolnić"
        );
    }
}
