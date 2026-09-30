//! Generowanie klipów lektora w tle (PLAN.md, sekcja 8).
//!
//! Synteza trwa sekundy i nigdy nie idzie przez wątek interfejsu (AGENTS.md, zasady architektury):
//! zlecenie jest przyjmowane natychmiast, liczy się w wątku roboczym, a do interfejsu lecą
//! zdarzenia z postępem. DJ widzi status („generuję” / „gotowe” / „nie udało się”) i pasek
//! postępu, więc nigdy nie musi zgadywać, czy voice-over jest przygotowany.
//!
//! Zlecenia są numerowane. Jeśli prośba zostanie poprawiona, gdy starsze zlecenie jeszcze liczy,
//! wynik starszego jest odrzucany — do bazy i do interfejsu trafia tylko klip z najnowszym tekstem.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter};
use tracing::{info, warn};

use crate::db::Db;
use crate::tts::{Clip, TtsError, TtsProvider, VoiceTuning};

/// Zdarzenie z postępem generowania klipu; nasłuchuje widok kolejek.
pub const EVENT_CLIP_PROGRESS: &str = "clips://progress";

/// Stan klipu lektora dla jednej prośby.
///
/// „Klipu nie ma i nic się nie dzieje” poznajemy po tym, że prośba nie ma ścieżki klipu
/// i nie ma dla niej żadnego stanu — nie trzeba osobnego wariantu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipStatus {
    /// Właśnie liczymy klip.
    Generating,
    /// Klip gotowy.
    Ready,
    /// Nie udało się policzyć klipu.
    Failed,
}

/// Postęp generowania klipu wysyłany do interfejsu i zwracany przez `clip_states`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClipProgress {
    pub request_id: i64,
    pub status: ClipStatus,
    /// Postęp syntezy w procentach (0–100). Sensowny tylko przy `generating`.
    pub progress: u8,
    /// Dlaczego się nie udało — pokazujemy DJ-owi, żeby wiedział, co poprawić.
    pub message: Option<String>,
}

impl ClipProgress {
    /// Postęp syntezy. Postęp bez błędu; jedno miejsce prawdy dla kształtu zdarzenia.
    fn generating(request_id: i64, percent: u8) -> Self {
        Self {
            request_id,
            status: ClipStatus::Generating,
            progress: percent,
            message: None,
        }
    }
}

/// Rejestr zleceń generowania klipów.
///
/// Trzyma ostatni znany stan każdej prośby (dzięki temu interfejs odtworzy „generuję” po
/// przeładowaniu okna) oraz numer bieżącego zlecenia (dzięki temu wynik wolniejszego zlecenia
/// nie nadpisze świeższego klipu).
pub struct ClipJobs {
    /// Numer ostatniego zlecenia dla każdej prośby.
    revisions: Mutex<HashMap<i64, u64>>,
    counter: AtomicU64,
    /// Ostatni znany stan klipu każdej prośby.
    states: Mutex<HashMap<i64, ClipProgress>>,
}

impl Default for ClipJobs {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipJobs {
    pub fn new() -> Self {
        Self {
            revisions: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
            states: Mutex::new(HashMap::new()),
        }
    }

    /// Stan klipu wszystkich prośb, o których coś wiemy.
    pub fn states(&self) -> Vec<ClipProgress> {
        match self.states.lock() {
            Ok(states) => states.values().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Rozpoczyna liczenie klipu dla prośby. Zwraca od razu — postęp leci zdarzeniami.
    pub fn start(
        self: &Arc<Self>,
        app: AppHandle,
        db: Arc<Db>,
        provider: Arc<dyn TtsProvider>,
        tuning: VoiceTuning,
        request_id: i64,
        text: String,
    ) {
        let revision = self.begin(request_id);

        self.publish(&app, ClipProgress::generating(request_id, 0));

        let jobs = Arc::clone(self);
        let report_jobs = Arc::clone(self);
        let progress_app = app.clone();

        tauri::async_runtime::spawn_blocking(move || {
            let result = provider.synthesize_with_progress(
                &text,
                &tuning,
                Box::new(move |progress| {
                    report_jobs.report(&progress_app, request_id, revision, progress);
                }),
            );

            jobs.finish(&app, &db, request_id, revision, result);
        });
    }

    /// Zapomina stan klipu — np. gdy prośba została odrzucona. Zlecenie w locie, jeśli jakieś
    /// trwa, straci aktualność i nie zapisze nic do bazy.
    pub fn forget(&self, request_id: i64) {
        match self.revisions.lock() {
            Ok(mut revisions) => {
                revisions.remove(&request_id);
            }
            Err(_) => warn!(request_id, "nie udało się wyczyścić numeru zlecenia klipu"),
        }

        match self.states.lock() {
            Ok(mut states) => {
                states.remove(&request_id);
            }
            Err(_) => warn!(request_id, "nie udało się wyczyścić stanu klipu"),
        }
    }

    /// Nowy numer zlecenia — od tej chwili tylko ono ma prawo zapisać wynik.
    fn begin(&self, request_id: i64) -> u64 {
        let revision = self.counter.fetch_add(1, Ordering::SeqCst) + 1;

        match self.revisions.lock() {
            Ok(mut revisions) => {
                revisions.insert(request_id, revision);
            }
            Err(_) => warn!(request_id, "nie udało się zapisać numeru zlecenia klipu"),
        }

        revision
    }

    /// Czy to zlecenie jest wciąż najnowsze dla tej prośby.
    fn is_current(&self, request_id: i64, revision: u64) -> bool {
        self.revisions
            .lock()
            .map(|revisions| revisions.get(&request_id) == Some(&revision))
            .unwrap_or(false)
    }

    /// Melduje postęp zlecenia — ale tylko wtedy, gdy nikt nie zamówił już nowszego klipu.
    fn report(&self, app: &AppHandle, request_id: i64, revision: u64, progress: f32) {
        if !self.is_current(request_id, revision) {
            return;
        }

        let percent = (progress.clamp(0.0, 1.0) * 100.0).round() as u8;

        self.publish(app, ClipProgress::generating(request_id, percent));
    }

    /// Kończy zlecenie: zapisuje ścieżkę klipu w bazie albo melduje porażkę.
    fn finish(
        &self,
        app: &AppHandle,
        db: &Db,
        request_id: i64,
        revision: u64,
        result: Result<Clip, TtsError>,
    ) {
        if !self.is_current(request_id, revision) {
            info!(
                request_id,
                "wynik nieaktualnego zlecenia klipu odrzucony — dedykacja zdążyła się zmienić"
            );

            return;
        }

        match result {
            Ok(clip) => {
                if let Err(error) = db.set_tts_clip_path(request_id, Some(&clip.path)) {
                    warn!(request_id, error = %error, "nie udało się zapisać ścieżki klipu");

                    self.fail(app, request_id, error.to_string());

                    return;
                }

                info!(
                    request_id,
                    path = %clip.path.display(),
                    duration_ms = clip.duration_ms,
                    "klip lektora gotowy"
                );

                self.publish(
                    app,
                    ClipProgress {
                        request_id,
                        status: ClipStatus::Ready,
                        progress: 100,
                        message: None,
                    },
                );
            }
            Err(error) => {
                warn!(request_id, error = %error, "nie udało się policzyć klipu lektora");

                self.fail(app, request_id, error.to_string());
            }
        }
    }

    fn fail(&self, app: &AppHandle, request_id: i64, message: String) {
        self.publish(
            app,
            ClipProgress {
                request_id,
                status: ClipStatus::Failed,
                progress: 0,
                message: Some(message),
            },
        );
    }

    /// Zapisuje stan i wysyła go do interfejsu.
    fn publish(&self, app: &AppHandle, progress: ClipProgress) {
        self.set_state(&progress);

        if let Err(error) = app.emit(EVENT_CLIP_PROGRESS, &progress) {
            warn!(error = %error, "nie udało się wysłać postępu generowania klipu");
        }
    }

    /// Zapisuje stan bez wysyłania zdarzenia (testy i miejsca bez uchwytu aplikacji).
    fn set_state(&self, progress: &ClipProgress) {
        match self.states.lock() {
            Ok(mut states) => {
                states.insert(progress.request_id, progress.clone());
            }
            Err(_) => warn!(
                request_id = progress.request_id,
                "nie udało się zapisać stanu klipu"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generating(request_id: i64, percent: u8) -> ClipProgress {
        ClipProgress {
            request_id,
            status: ClipStatus::Generating,
            progress: percent,
            message: None,
        }
    }

    #[test]
    fn a_newer_revision_replaces_an_older_one() {
        let jobs = ClipJobs::new();

        let first = jobs.begin(7);

        assert!(jobs.is_current(7, first), "świeże zlecenie jest bieżące");
        assert!(!jobs.is_current(8, first), "inna prośba to inna sprawa");

        let second = jobs.begin(7);

        assert!(jobs.is_current(7, second));
        assert!(
            !jobs.is_current(7, first),
            "starsze zlecenie tej samej prośby traci aktualność"
        );
    }

    #[test]
    fn forgetting_a_request_drops_its_state_and_invalidates_work_in_flight() {
        let jobs = ClipJobs::new();
        let revision = jobs.begin(7);

        jobs.set_state(&generating(7, 40));

        assert_eq!(jobs.states().len(), 1);

        jobs.forget(7);

        assert!(
            jobs.states().is_empty(),
            "odrzucona prośba nie trzyma stanu"
        );
        assert!(
            !jobs.is_current(7, revision),
            "zlecenie w locie nie może już nic zapisać"
        );
    }

    #[test]
    fn each_request_keeps_its_own_state() {
        let jobs = ClipJobs::new();

        jobs.set_state(&generating(1, 30));
        jobs.set_state(&ClipProgress {
            request_id: 2,
            status: ClipStatus::Ready,
            progress: 100,
            message: None,
        });

        let mut ids: Vec<i64> = jobs.states().iter().map(|state| state.request_id).collect();
        ids.sort_unstable();

        assert_eq!(ids, vec![1, 2]);
    }
}
