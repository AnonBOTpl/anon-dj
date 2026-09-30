//! Warstwa lektora (TTS): kontrakt na syntezę, gotowy klip i pamięć podręczna klipów.
//!
//! Silnik jest schowany za traitem [`TtsProvider`] — reszta aplikacji nie wie, że pod spodem
//! siedzi Piper, i nie pozna tego nawet wtedy, gdy dojdzie drugi silnik (PLAN.md, sekcja 8).
//!
//! Klipy są **generowane z wyprzedzeniem i trzymane na dysku**: zatwierdzenie dedykacji albo
//! poprawka tekstu liczy klip raz, a „Wykonaj” tylko go odtwarza. Dzięki temu decyzja DJ-a
//! nie czeka na syntezę.
//!
//! Klip jest adresowany treścią: nazwa pliku to skrót z głosu, tekstu i strojenia. Zmiana
//! któregokolwiek z nich daje nowy plik, a powtórzenie tej samej dedykacji nie liczy nic drugi raz.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Gotowy do odtworzenia klip z dedykacją.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Clip {
    /// Ścieżka pliku WAV.
    pub path: PathBuf,
    /// Długość nagrania — potrzebna do zgrania rampy głośności z końcem lektora.
    pub duration_ms: u32,
    pub sample_rate: u32,
}

/// Strojenie głosu. Wartości trzymamy jako liczby całkowite (promile i procenty), bo ustawienia
/// muszą dać się porównywać i zapisywać bez błędów zaokrągleń `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VoiceTuning {
    /// Tempo mowy w promilach: 1000 = normalne (odpowiada `length_scale` w Piper).
    pub length_scale_milli: u32,
    /// Zmienność barwy w promilach (odpowiada `noise_scale`).
    pub noise_scale_milli: u32,
    /// Zmienność rytmu w promilach (odpowiada `noise_w`).
    pub noise_w_milli: u32,
    /// Głośność lektora w procentach (100 = normalna).
    pub volume_percent: u32,
    /// Pauza między zdaniami w milisekundach.
    pub sentence_silence_ms: u32,
}

impl Default for VoiceTuning {
    fn default() -> Self {
        // Domyślne wartości z modeli Piper (`inference` w pliku `.onnx.json`).
        Self {
            length_scale_milli: 1000,
            noise_scale_milli: 667,
            noise_w_milli: 800,
            volume_percent: 100,
            sentence_silence_ms: 200,
        }
    }
}

impl VoiceTuning {
    pub fn length_scale(&self) -> f32 {
        self.length_scale_milli as f32 / 1000.0
    }

    pub fn noise_scale(&self) -> f32 {
        self.noise_scale_milli as f32 / 1000.0
    }

    pub fn noise_w(&self) -> f32 {
        self.noise_w_milli as f32 / 1000.0
    }

    pub fn volume(&self) -> f32 {
        self.volume_percent as f32 / 100.0
    }
}

/// Błąd lektora.
#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    #[error("nie udało się zapisać pliku audio {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("głos „{0}” nie jest dostępny")]
    VoiceUnavailable(String),

    #[error("nie udało się zsyntezować tekstu: {0}")]
    Synthesis(String),
}

impl TtsError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Kontrakt na syntezę mowy: tekst dedykacji wchodzi, gotowy klip wychodzi.
///
/// Implementacja jednego silnika wystarcza, ale nic nie stoi na przeszkodzie, żeby dołożyć
/// drugi — dlatego cały kod aplikacji zna wyłącznie ten trait.
pub trait TtsProvider: Send + Sync {
    /// Identyfikator głosu (np. `justyna`). Wchodzi do klucza pamięci podręcznej, więc musi być
    /// stabilny między uruchomieniami.
    fn voice_id(&self) -> &str;

    /// Zamienia tekst dedykacji na klip audio.
    fn synthesize(&self, text: &str, tuning: &VoiceTuning) -> Result<Clip, TtsError>;
}

/// Pamięć podręczna klipów: katalog na dysku i nazwy plików wyliczane z treści.
pub struct ClipCache {
    dir: PathBuf,
}

impl ClipCache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Katalog, w którym leżą klipy.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Ścieżka klipu dla danego głosu, tekstu i strojenia. Ten sam zestaw zawsze daje tę samą
    /// nazwę, więc klip policzony raz nie jest liczony drugi raz.
    pub fn path_for(&self, voice_id: &str, text: &str, tuning: &VoiceTuning) -> PathBuf {
        self.dir
            .join(format!("{}.wav", cache_key(voice_id, text, tuning)))
    }

    /// Czy klip jest już na dysku.
    pub fn is_cached(&self, voice_id: &str, text: &str, tuning: &VoiceTuning) -> bool {
        self.path_for(voice_id, text, tuning).is_file()
    }

    /// Zabiera klip z pamięci podręcznej (np. gdy tekst okazał się nieaktualny).
    /// Brak pliku nie jest błędem.
    pub fn forget(&self, voice_id: &str, text: &str, tuning: &VoiceTuning) -> Result<(), TtsError> {
        let path = self.path_for(voice_id, text, tuning);

        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(TtsError::io(&path, error)),
        }
    }
}

/// Skrót (FNV-1a 64) z głosu, tekstu i strojenia — stabilny, więc klipy przeżywają restart
/// aplikacji. Własna implementacja zamiast `DefaultHasher`, bo ten nie gwarantuje tej samej
/// wartości między wersjami kompilatora, a wtedy cache zacząłby się „gubić” bez powodu.
fn cache_key(voice_id: &str, text: &str, tuning: &VoiceTuning) -> String {
    // Znacznik \u{1f} rozdziela pola; w dedykacji ani w nazwie głosu się nie pojawi.
    let fingerprint = format!(
        "{voice_id}\u{1f}{text}\u{1f}{},{},{},{},{}",
        tuning.length_scale_milli,
        tuning.noise_scale_milli,
        tuning.noise_w_milli,
        tuning.volume_percent,
        tuning.sentence_silence_ms,
    );

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;

    for byte in fingerprint.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }

    format!("{hash:016x}")
}

/// Zapisuje próbki jako 16-bitowy WAV mono i zwraca gotowy klip.
///
/// Leży tutaj, a nie w implementacji silnika, bo format klipu jest umową całej warstwy TTS —
/// dzięki temu każdy silnik zapisuje dokładnie to samo.
pub fn write_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<Clip, TtsError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| TtsError::io(parent, error))?;
    }

    // 16 bitów wystarcza na lektora, a mniejszy plik szybciej się wczytuje w trakcie imprezy.
    let pcm: Vec<i16> = samples
        .iter()
        .map(|sample| (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16)
        .collect();

    let data_len = u32::try_from(pcm.len() * 2).unwrap_or(u32::MAX);
    let byte_rate = sample_rate.saturating_mul(2);

    let file = std::fs::File::create(path).map_err(|error| TtsError::io(path, error))?;
    let mut writer = std::io::BufWriter::new(file);

    let write = |writer: &mut std::io::BufWriter<std::fs::File>| -> std::io::Result<()> {
        writer.write_all(b"RIFF")?;
        writer.write_all(&(36 + data_len).to_le_bytes())?;
        writer.write_all(b"WAVEfmt ")?;
        writer.write_all(&16u32.to_le_bytes())?;
        writer.write_all(&1u16.to_le_bytes())?;
        writer.write_all(&1u16.to_le_bytes())?;
        writer.write_all(&sample_rate.to_le_bytes())?;
        writer.write_all(&byte_rate.to_le_bytes())?;
        writer.write_all(&2u16.to_le_bytes())?;
        writer.write_all(&16u16.to_le_bytes())?;
        writer.write_all(b"data")?;
        writer.write_all(&data_len.to_le_bytes())?;

        for sample in &pcm {
            writer.write_all(&sample.to_le_bytes())?;
        }

        writer.flush()
    };

    write(&mut writer).map_err(|error| TtsError::io(path, error))?;

    let duration_ms = if sample_rate == 0 {
        0
    } else {
        // Milisekundy z liczby próbek; przy typowym nagraniu błąd to ułamek milisekundy.
        u32::try_from((samples.len() as u64 * 1_000) / u64::from(sample_rate)).unwrap_or(u32::MAX)
    };

    Ok(Clip {
        path: path.to_path_buf(),
        duration_ms,
        sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Świeży katalog testowy — czyścimy go, żeby wynik nie zależał od poprzedniego przebiegu.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anon-dj-tts-{name}"));

        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("katalog testowy");

        dir
    }

    #[test]
    fn the_same_text_voice_and_tuning_share_one_clip() {
        let cache = ClipCache::new(temp_dir("same-key"));
        let tuning = VoiceTuning::default();

        assert_eq!(
            cache.path_for("justyna", "Sto lat!", &tuning),
            cache.path_for("justyna", "Sto lat!", &tuning)
        );
        assert!(!cache.is_cached("justyna", "Sto lat!", &tuning));
    }

    #[test]
    fn changing_the_text_voice_or_tuning_gives_a_new_clip() {
        let cache = ClipCache::new(temp_dir("different-keys"));
        let base = VoiceTuning::default();

        let reference = cache.path_for("justyna", "Sto lat!", &base);

        assert_ne!(
            reference,
            cache.path_for("justyna", "Sto lat!!", &base),
            "inny tekst to inny klip"
        );
        assert_ne!(
            reference,
            cache.path_for("jarvis", "Sto lat!", &base),
            "inny głos to inny klip"
        );

        let faster = VoiceTuning {
            length_scale_milli: 900,
            ..base
        };

        assert_ne!(
            reference,
            cache.path_for("justyna", "Sto lat!", &faster),
            "inne tempo to inny klip"
        );
    }

    #[test]
    fn a_written_clip_reports_its_duration_and_can_be_forgotten() {
        let dir = temp_dir("write-and-forget");
        let cache = ClipCache::new(dir.clone());
        let tuning = VoiceTuning::default();

        // Sekunda ciszy przy 22 050 Hz.
        let samples = vec![0.0_f32; 22_050];
        let path = cache.path_for("justyna", "Sto lat!", &tuning);

        let clip = write_wav(&path, &samples, 22_050).expect("zapis klipu");

        assert_eq!(clip.duration_ms, 1_000);
        assert_eq!(clip.sample_rate, 22_050);
        assert_eq!(clip.path, path);
        assert!(path.is_file(), "plik WAV powinien powstać");
        assert!(cache.is_cached("justyna", "Sto lat!", &tuning));

        cache
            .forget("justyna", "Sto lat!", &tuning)
            .expect("usunięcie klipu");

        assert!(!cache.is_cached("justyna", "Sto lat!", &tuning));
        // Powtórne usunięcie nie może wywrócić aplikacji.
        cache
            .forget("justyna", "Sto lat!", &tuning)
            .expect("usunięcie nieistniejącego klipu");
    }

    #[test]
    fn tuning_milli_values_convert_to_engine_units() {
        let tuning = VoiceTuning {
            length_scale_milli: 1_250,
            noise_scale_milli: 500,
            noise_w_milli: 750,
            volume_percent: 80,
            sentence_silence_ms: 300,
        };

        assert!((tuning.length_scale() - 1.25).abs() < f32::EPSILON);
        assert!((tuning.noise_scale() - 0.5).abs() < f32::EPSILON);
        assert!((tuning.noise_w() - 0.75).abs() < f32::EPSILON);
        assert!((tuning.volume() - 0.8).abs() < f32::EPSILON);
    }
}
