//! Czysta logika ramp głośności (PLAN.md, sekcja 7).
//!
//! Rampy są **osobno od wywołań HTTP** i bez żadnego stanu: dostają parametry, zwracają listę
//! wartości do wysłania. Dzięki temu duck, fade-out i wejście zamówionego utworu da się sprawdzić
//! testem jednostkowym, a nie „na ucho” w środku imprezy (AGENTS.md).
//!
//! **Wszystko liczymy w decybelach**, bo tak działa wolumen beefweba: `0.0` to maksimum,
//! `-100.0` to cisza. Rampa liniowa w dB brzmi dla ucha równomiernie; liniowa w „procentach”
//! zrobiłaby skok na początku i wlokłaby się na końcu.

use std::time::Duration;

/// Maksimum skali wolumenu beefweba (0 dB).
pub const MAX_VOLUME_DB: f64 = 0.0;

/// Cisza na skali beefweba.
pub const SILENCE_DB: f64 = -100.0;

/// Jedna wartość rampy wraz z odstępem od poprzedniej.
///
/// `delay` to **czekanie przed wysłaniem** tej wartości — pierwszy krok ma zerowe opóźnienie,
/// bo jest wartością początkową.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RampStep {
    pub delay: Duration,
    pub volume_db: f64,
}

/// Przycina wolumen do zakresu, który rozumie beefweb.
pub fn clamp_db(volume_db: f64) -> f64 {
    if volume_db.is_nan() {
        return SILENCE_DB;
    }

    volume_db.clamp(SILENCE_DB, MAX_VOLUME_DB)
}

/// Rampa od `start_db` do `end_db`, równomiernie w dB, w krokach po `step_ms`.
///
/// Zwraca **także wartość początkową**, żeby wołający nie musiał pamiętać o jej wysłaniu.
/// Czas trwania `0` zwraca samą wartość końcową — nie ma czego rozkładać w czasie.
pub fn ramp(start_db: f64, end_db: f64, duration_ms: u32, step_ms: u32) -> Vec<RampStep> {
    let start = clamp_db(start_db);
    let end = clamp_db(end_db);

    if duration_ms == 0 || step_ms == 0 {
        return vec![RampStep {
            delay: Duration::ZERO,
            volume_db: end,
        }];
    }

    let step_ms = u64::from(step_ms);
    let duration = u64::from(duration_ms);
    let steps = (duration / step_ms).max(1);

    let mut plan = Vec::with_capacity(steps as usize + 1);

    for index in 0..=steps {
        let position = index as f64 / steps as f64;

        plan.push(RampStep {
            // Pierwszy krok jest już wartością początkową — nie ma na co czekać.
            delay: if index == 0 {
                Duration::ZERO
            } else {
                Duration::from_millis(step_ms)
            },
            volume_db: clamp_db(start + (end - start) * position),
        });
    }

    plan
}

/// Poziom ducku: `duck_percent` procent **amplitudy** względem poziomu normalnego.
///
/// „Ścisz do jednej trzeciej” to potocznie jedna trzecia głośności, czyli amplitudy — dlatego
/// przeliczamy procent na decybele, a nie odejmujemy procent od decybeli. `100%` zostawia poziom
/// bez zmian, `0%` to cisza.
pub fn duck_level_db(normal_db: f64, duck_percent: u32) -> f64 {
    let normal = clamp_db(normal_db);

    if duck_percent == 0 {
        return SILENCE_DB;
    }

    let ratio = f64::from(duck_percent) / 100.0;

    clamp_db(normal + 20.0 * ratio.log10())
}

/// Rampa dochodzenia do poziomu normalnego z ciszy — używana dla zamówionego utworu.
pub fn ramp_up(normal_db: f64, duration_ms: u32, step_ms: u32) -> Vec<RampStep> {
    // Zaczynamy od ciszy: poprzedni utwór został wyciszony do zera, więc nowy nie może
    // wskoczyć na pełną głośność.
    ramp(SILENCE_DB, normal_db, duration_ms, step_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Domyślny krok wysyłania wartości — tyle wynosi przerwa między żądaniami do odtwarzacza.
    const STEP_MS: u32 = 50;

    #[test]
    fn a_ramp_starts_at_the_start_and_ends_at_the_end() {
        let plan = ramp(-20.0, -40.0, 400, STEP_MS);

        assert_eq!(plan[0].delay, Duration::ZERO, "pierwszy krok idzie od razu");
        assert!((plan[0].volume_db - (-20.0)).abs() < 1e-9);
        assert!((plan.last().expect("krok").volume_db - (-40.0)).abs() < 1e-9);
    }

    #[test]
    fn a_ramp_is_evenly_spaced_in_decibels() {
        let plan = ramp(0.0, -10.0, 400, STEP_MS);

        // 400 ms / 50 ms = 8 kroków, czyli 9 wartości.
        assert_eq!(plan.len(), 9);

        for (index, step) in plan.iter().enumerate().skip(1) {
            let expected = -10.0 * index as f64 / 8.0;

            assert!(
                (step.volume_db - expected).abs() < 1e-9,
                "krok {index} ma {step:?}, a powinien mieć {expected}"
            );
            assert_eq!(step.delay, Duration::from_millis(u64::from(STEP_MS)));
        }
    }

    #[test]
    fn a_ramp_never_leaves_the_scale_the_player_understands() {
        let plan = ramp(0.0, -500.0, 100, STEP_MS);

        assert!(
            plan.iter().all(|step| step.volume_db >= SILENCE_DB),
            "wolumen poniżej -100 dB nie ma sensu"
        );
        assert_eq!(plan.last().expect("krok").volume_db, SILENCE_DB);
    }

    #[test]
    fn a_zero_length_ramp_jumps_straight_to_the_target() {
        let plan = ramp(0.0, -35.0, 0, STEP_MS);

        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].delay, Duration::ZERO);
        assert!((plan[0].volume_db - (-35.0)).abs() < 1e-9);
    }

    #[test]
    fn a_duck_to_a_third_is_about_ten_decibels_down() {
        // Trzydzieści trzy procent amplitudy to 20*log10(0,33) ≈ -9,63 dB.
        let level = duck_level_db(0.0, 33);

        assert!(
            (level - (-9.63)).abs() < 0.05,
            "duck do jednej trzeciej powinien być około -9,6 dB, a jest {level}"
        );
    }

    #[test]
    fn a_full_duck_changes_nothing_and_a_zero_duck_is_silence() {
        assert!((duck_level_db(-6.0, 100) - (-6.0)).abs() < 1e-9);
        assert_eq!(duck_level_db(-6.0, 0), SILENCE_DB);
    }

    #[test]
    fn a_duck_level_is_measured_against_the_current_volume_not_the_maximum() {
        // Duck działa względem tego, co DJ ma teraz ustawione, a nie względem zera.
        let quiet = duck_level_db(-30.0, 50);

        assert!(
            (quiet - (-36.02)).abs() < 0.05,
            "połowa amplitudy od -30 dB to około -36 dB, a jest {quiet}"
        );
    }

    #[test]
    fn ramping_up_starts_from_silence() {
        let plan = ramp_up(-8.0, 400, STEP_MS);

        assert_eq!(
            plan[0].volume_db, SILENCE_DB,
            "zamówiony utwór wjeżdża z ciszy"
        );
        assert!((plan.last().expect("krok").volume_db - (-8.0)).abs() < 1e-9);
    }

    #[test]
    fn a_broken_value_is_treated_as_silence() {
        assert_eq!(clamp_db(f64::NAN), SILENCE_DB);
        assert_eq!(clamp_db(12.0), MAX_VOLUME_DB);
        assert_eq!(clamp_db(-1000.0), SILENCE_DB);
    }
}
