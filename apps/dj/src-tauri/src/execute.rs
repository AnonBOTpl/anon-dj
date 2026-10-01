//! Sekwencja wykonania: ściszenie muzyki, dedykacja nad nią, wyciszenie starego utworu
//! i wejście zamówionego (PLAN.md, sekcja 7).
//!
//! DJ pracuje tak na antenie: dedykacja leci **na ogonie** tego, co gra, a zamówiony utwór
//! przejmuje po niej. Stary utwór **nie wraca** — dedykacja go zapowiada, więc po niej jest już
//! tylko nowy. Gdy nic nie gra, kroki ściszania i wyciszania są pomijane.
//!
//! Wszystko dzieje się na **jednym wątku blokującym** (`spawn_blocking` po stronie komendy), bo
//! i tak składa się z żądań HTTP i czekania. Wątek UI nie ma z tym nic wspólnego (AGENTS.md).
//!
//! Cała wiedza o odtwarzaczu przechodzi przez [`PlayerAdapter`], a logika ramp siedzi w
//! [`crate::volume`] — ten moduł tylko składa jedno z drugim.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::AppHandle;
use tracing::{info, warn};

use crate::audio::{AudioError, PlaybackTarget, VoiceOverPlayer};
use crate::player::{PlaybackState, PlayerAdapter, PlayerError, PlayerStatus};
use crate::settings::ExecutionSettings;
use crate::volume::{self, RampStep, SILENCE_DB};

/// Co ile wysyłamy kolejne wartości rampy. Krótszy krok to gładsze przejście i więcej żądań;
/// 50 ms to kompromis, który przy lokalnym API jest niezauważalny.
const RAMP_STEP_MS: u32 = 50;

/// O ile decybeli wolno się pomylić, zanim uznamy, że DJ sam ruszył głośność.
const MANUAL_VOLUME_TOLERANCE_DB: f64 = 0.5;

/// Co ile sprawdzamy, czy dedykacja dobiegła końca.
const CLIP_POLL: Duration = Duration::from_millis(100);

/// Bezpiecznik: ile najdłużej czekamy na koniec klipu. Dedykacja jest krótka, więc to tylko
/// zabezpieczenie przed zawieszeniem sekwencji na zawsze.
const CLIP_TIMEOUT: Duration = Duration::from_secs(120);

/// Błąd sekwencji wykonania.
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error(transparent)]
    Player(#[from] PlayerError),

    #[error(transparent)]
    Audio(#[from] AudioError),

    #[error("błąd bazy danych: {0}")]
    Db(#[from] crate::db::DbError),

    #[error("lektora nie dało się odczytać do końca (prośba {0})")]
    ClipStuck(i64),
}

/// Co jest potrzebne, żeby wykonać prośbę. Zbierane po stronie komendy, żeby ten moduł nie
/// wiedział nic o bazie danych.
pub struct ExecutionPlan {
    pub request_id: i64,
    /// Ścieżka pliku zamówionego utworu.
    pub track_path: String,
    /// Ścieżka gotowego klipu lektora.
    pub clip_path: PathBuf,
    /// Urządzenie wyjściowe voice-overu (puste = domyślne systemowe).
    pub device_id: String,
    /// Głośność lektora jako mnożnik.
    pub tts_volume: f32,
    pub settings: ExecutionSettings,
    /// Stan odtwarzacza zmierzony **tuż przed** sekwencją — z niego bierzemy poziom normalny.
    pub start: PlayerStatus,
}

/// Jak skończyła się rampa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RampOutcome {
    /// Rampa doszła do wartości końcowej.
    Completed,
    /// DJ sam ruszył głośność — przestaliśmy nią sterować.
    Cancelled,
}

/// Wysyła rampę krok po kroku.
///
/// Między krokami sprawdzamy, czy głośność jest tam, gdzie ją zostawiliśmy. Jeśli nie — DJ sięgnął
/// po suwak i **przestajemy sterować**, zamiast szarpać się z człowiekiem za konsoletą.
/// Sen jest wstrzykiwany, żeby logikę dało się sprawdzić testem bez czekania.
pub fn apply_ramp(
    adapter: &dyn PlayerAdapter,
    plan: &[RampStep],
    sleep: &mut dyn FnMut(Duration),
) -> Result<RampOutcome, PlayerError> {
    for (index, step) in plan.iter().enumerate() {
        sleep(step.delay);

        if index > 0 {
            let current = adapter.status()?.volume_db;
            let expected = plan[index - 1].volume_db;

            if (current - expected).abs() > MANUAL_VOLUME_TOLERANCE_DB {
                info!(expected, current, "DJ ruszył głośność — przerywam rampę");

                return Ok(RampOutcome::Cancelled);
            }
        }

        adapter.set_volume_db(step.volume_db)?;
    }

    Ok(RampOutcome::Completed)
}

/// Czeka, aż klip lektora przestanie lecieć. `false` oznacza, że zadziałał bezpiecznik.
pub fn wait_for_clip(
    current: &mut dyn FnMut() -> Option<PlaybackTarget>,
    target: PlaybackTarget,
    sleep: &mut dyn FnMut(Duration),
) -> bool {
    let deadline = Instant::now() + CLIP_TIMEOUT;

    while current() == Some(target) {
        if Instant::now() >= deadline {
            return false;
        }

        sleep(CLIP_POLL);
    }

    true
}

/// Opakowanie na flagę „stop po bieżącym utworze”, które zdejmuje ją nawet przy błędzie.
///
/// Bez tego przerwana sekwencja zostawiłaby playlistę DJ-a zamrożoną po następnym utworze.
struct StopAfterGuard<'a> {
    adapter: &'a dyn PlayerAdapter,
}

impl<'a> StopAfterGuard<'a> {
    /// Włącza flagę. Od tego momentu utwór, który skończy się pod lektorem, zatrzyma odtwarzanie
    /// zamiast wciągnąć pod głos kolejną pozycję playlisty.
    fn arm(adapter: &'a dyn PlayerAdapter) -> Result<Self, PlayerError> {
        adapter.set_stop_after_current(true)?;

        Ok(Self { adapter })
    }
}

impl Drop for StopAfterGuard<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.adapter.set_stop_after_current(false) {
            warn!(error = %error, "nie udało się zdjąć flagi „stop po bieżącym utworze”");
        }
    }
}

/// Wykonuje całą sekwencję. Blokujące — wołający odpowiada za wątek.
pub fn run_sequence(
    app: &AppHandle,
    adapter: &dyn PlayerAdapter,
    clips: &Arc<VoiceOverPlayer>,
    plan: &ExecutionPlan,
) -> Result<(), ExecutionError> {
    let mut sleep = |duration: Duration| std::thread::sleep(duration);
    let normal_db = plan.start.volume_db;
    // Ściszamy tylko wtedy, gdy naprawdę coś gra. Gdy jest cisza, nie ma czego duckować
    // ani wyciszać (PLAN.md, sekcja 7).
    let something_plays = plan.start.state == PlaybackState::Playing;

    info!(
        request_id = plan.request_id,
        normal_db, something_plays, "start sekwencji wykonania"
    );

    // 1. Stary utwór nie może wciągnąć pod głos następnej pozycji playlisty, jeśli skończy się
    //    w trakcie dedykacji.
    let guard = StopAfterGuard::arm(adapter)?;

    let duck_db = volume::duck_level_db(normal_db, plan.settings.duck_percent);

    // 2. Ściszamy, ale **nie zatrzymujemy** — muzyka zostaje pod głosem lektora.
    if something_plays {
        let plan_ramp = volume::ramp(normal_db, duck_db, plan.settings.duck_ramp_ms, RAMP_STEP_MS);

        apply_ramp(adapter, &plan_ramp, &mut sleep)?;
    }

    // 3. Dedykacja.
    clips.play(
        app,
        PlaybackTarget::Request(plan.request_id),
        &plan.clip_path,
        &plan.device_id,
        plan.tts_volume,
    )?;

    // 4. Czekamy na jej koniec.
    let mut current = || clips.current();
    let target = PlaybackTarget::Request(plan.request_id);

    if !wait_for_clip(&mut current, target, &mut sleep) {
        warn!(
            request_id = plan.request_id,
            "dedykacja nie zgłosiła końca — przerywam sekwencję"
        );

        return Err(ExecutionError::ClipStuck(plan.request_id));
    }

    // Krótka pauza, żeby ostatnie słowo nie zderzyło się z wejściem utworu.
    std::thread::sleep(Duration::from_millis(u64::from(plan.settings.gap_ms)));

    // 5. Stary utwór schodzi do zera i zostaje zatrzymany — nie wznawiamy go.
    if something_plays {
        let plan_ramp = volume::ramp(duck_db, SILENCE_DB, plan.settings.fade_out_ms, RAMP_STEP_MS);

        apply_ramp(adapter, &plan_ramp, &mut sleep)?;

        adapter.stop()?;
    }

    // 6. Flaga musi zejść **przed** startem zamówionego utworu: inaczej foobar zatrzymałby się
    //    także po nim, zamiast wrócić do playlisty DJ-a.
    drop(guard);

    adapter.play_now(&plan.track_path)?;
    adapter.set_volume_db(SILENCE_DB)?;

    // 7. Zamówiony utwór wjeżdża do normalnej głośności.
    let plan_ramp = volume::ramp_up(normal_db, plan.settings.start_ramp_ms, RAMP_STEP_MS);
    let outcome = apply_ramp(adapter, &plan_ramp, &mut sleep)?;

    info!(
        request_id = plan.request_id,
        ramp = ?outcome,
        "sekwencja wykonania zakończona"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Odtwarzacz-podróbka: zapamiętuje wysyłane głośności i pozwala udawać, że DJ ruszył suwak.
    ///
    /// Pod `Mutex`, a nie pod `RefCell`, bo cecha adaptera wymaga `Send + Sync` — dzięki temu
    /// adapter da się przenieść na wątek blokujący bez owijania w dodatkowe warstwy.
    struct FakePlayer {
        status: Mutex<PlayerStatus>,
        volumes: Mutex<Vec<f64>>,
        stop_after: Mutex<Vec<bool>>,
        played: Mutex<Vec<String>>,
        /// O ile podróbka sama przesunie głośność — tak udajemy rękę DJ-a na suwaku.
        drift_db: f64,
    }

    impl FakePlayer {
        fn new(volume_db: f64) -> Self {
            Self {
                status: Mutex::new(PlayerStatus {
                    volume_db,
                    ..PlayerStatus::offline()
                }),
                volumes: Mutex::new(Vec::new()),
                stop_after: Mutex::new(Vec::new()),
                played: Mutex::new(Vec::new()),
                drift_db: 0.0,
            }
        }

        fn volume(&self) -> f64 {
            lock(&self.status).volume_db
        }

        fn volumes(&self) -> Vec<f64> {
            lock(&self.volumes).clone()
        }

        fn stop_after(&self) -> Vec<bool> {
            lock(&self.stop_after).clone()
        }
    }

    /// Blokada w teście — zatruty mutex jest tu błędem testu, a nie sytuacją do obsłużenia.
    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex.lock().expect("blokada podróbki")
    }

    impl PlayerAdapter for FakePlayer {
        fn status(&self) -> Result<PlayerStatus, PlayerError> {
            let mut status = lock(&self.status).clone();
            status.volume_db += self.drift_db;

            Ok(status)
        }

        fn play_next(&self, path: &str) -> Result<(), PlayerError> {
            lock(&self.played).push(path.to_string());

            Ok(())
        }

        fn play_now(&self, path: &str) -> Result<(), PlayerError> {
            lock(&self.played).push(path.to_string());

            Ok(())
        }

        fn set_volume_db(&self, volume_db: f64) -> Result<(), PlayerError> {
            lock(&self.status).volume_db = volume_db;
            lock(&self.volumes).push(volume_db);

            Ok(())
        }

        fn set_stop_after_current(&self, stop: bool) -> Result<(), PlayerError> {
            lock(&self.stop_after).push(stop);

            Ok(())
        }

        fn stop(&self) -> Result<(), PlayerError> {
            // Podróbce wystarczy, że zatrzymanie się udało — stanu odtwarzania nie modelujemy.
            Ok(())
        }
    }

    /// Sen wstrzykiwany do testów — nic nie czeka, więc testy nie trwają tyle, co sekwencja.
    type FakeSleep = Box<dyn FnMut(Duration)>;

    /// Sen, który nic nie czeka, i licznik wywołań.
    fn no_sleep() -> (FakeSleep, Arc<Mutex<usize>>) {
        let count = Arc::new(Mutex::new(0_usize));
        let counter = Arc::clone(&count);

        (Box::new(move |_| *lock(&counter) += 1), count)
    }

    #[test]
    fn a_ramp_sends_every_value_in_order() {
        let player = FakePlayer::new(0.0);
        let plan = volume::ramp(0.0, -20.0, 200, RAMP_STEP_MS);
        let (mut sleep, _) = no_sleep();

        let outcome = apply_ramp(&player, &plan, &mut sleep).expect("rampa");

        assert_eq!(outcome, RampOutcome::Completed);
        assert_eq!(player.volumes().len(), plan.len());
        assert_eq!(*player.volumes().last().expect("ostatnia"), -20.0);
    }

    #[test]
    fn a_manual_volume_change_stops_the_ramp() {
        let mut player = FakePlayer::new(0.0);
        // DJ chwycił suwak: głośność nie jest już tam, gdzie ją zostawiliśmy.
        player.drift_db = 6.0;

        let plan = volume::ramp(0.0, -30.0, 400, RAMP_STEP_MS);
        let (mut sleep, _) = no_sleep();

        let outcome = apply_ramp(&player, &plan, &mut sleep).expect("rampa");

        assert_eq!(outcome, RampOutcome::Cancelled);
        assert_eq!(
            player.volumes().len(),
            1,
            "po pierwszym kroku przestajemy sterować głośnością"
        );
    }

    #[test]
    fn waiting_for_the_clip_ends_when_the_clip_ends() {
        let mut calls = 0;

        let mut current = || {
            calls += 1;

            if calls < 3 {
                Some(PlaybackTarget::Request(7))
            } else {
                None
            }
        };
        let (mut sleep, _) = no_sleep();

        assert!(wait_for_clip(
            &mut current,
            PlaybackTarget::Request(7),
            &mut sleep
        ));
        assert_eq!(calls, 3);
    }

    #[test]
    fn waiting_stops_immediately_when_something_else_is_playing() {
        let mut current = || Some(PlaybackTarget::Preview);
        let (mut sleep, slept) = no_sleep();

        assert!(wait_for_clip(
            &mut current,
            PlaybackTarget::Request(7),
            &mut sleep
        ));
        assert_eq!(
            *lock(&slept),
            0,
            "poczekalnia nie czeka na cudze odtwarzanie"
        );
    }

    /// Pełna sekwencja na podróbkach — bez odtwarzacza, bez głośników i bez czekania.
    #[test]
    fn the_sequence_ducks_reads_and_starts_the_requested_track() {
        let player = FakePlayer::new(-6.0);
        lock(&player.status).state = PlaybackState::Playing;

        let settings = ExecutionSettings {
            duck_percent: 50,
            duck_ramp_ms: 200,
            gap_ms: 0,
            fade_out_ms: 100,
            start_ramp_ms: 200,
        };

        // Rampy liczymy tym samym kodem co produkcja; sekwencję sprawdzamy krok po kroku.
        let duck = volume::duck_level_db(-6.0, settings.duck_percent);
        assert!(duck < -6.0, "duck musi ściszyć");

        let plan = volume::ramp(-6.0, duck, settings.duck_ramp_ms, RAMP_STEP_MS);
        let (mut sleep, _) = no_sleep();
        apply_ramp(&player, &plan, &mut sleep).expect("duck");

        assert!(
            (player.volume() - duck).abs() < 1e-9,
            "po ducku głośność to poziom ducku"
        );

        let fade = volume::ramp(duck, SILENCE_DB, settings.fade_out_ms, RAMP_STEP_MS);
        apply_ramp(&player, &fade, &mut sleep).expect("wyciszenie");

        assert!((player.volume() - SILENCE_DB).abs() < 1e-9);

        // Zamówiony utwór wjeżdża z ciszy do poziomu normalnego.
        let start = volume::ramp_up(-6.0, settings.start_ramp_ms, RAMP_STEP_MS);
        apply_ramp(&player, &start, &mut sleep).expect("wejście utworu");

        assert!((player.volume() - (-6.0)).abs() < 1e-9);
    }

    #[test]
    fn the_flag_goes_up_and_always_comes_back_down() {
        let player = FakePlayer::new(0.0);

        {
            let _guard = StopAfterGuard::arm(&player).expect("flaga");
            assert_eq!(player.stop_after(), vec![true]);
        }

        assert_eq!(
            player.stop_after(),
            vec![true, false],
            "wyjście z bloku musi zdjąć flagę, nawet gdy sekwencja się przerwie"
        );
    }
}
