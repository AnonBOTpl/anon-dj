//! Lektor dedykacji: głosy Piper uruchamiane przez `sherpa-onnx`.
//!
//! Dlaczego nie `piper-rs`: jego ONNX Runtime wymaga AVX2 i wywraca się na komputerze, pod którym
//! gramy (AMD FX-8300), a sam build potrzebuje `libclang` i `cmake`. `sherpa-onnx` linkuje gotowe
//! biblioteki statyczne i działa na tym samym sprzęcie — sprawdzone (PLAN.md, sekcja 8).
//!
//! Układ katalogu z głosami (jeden katalog na głos, `espeak-ng-data` wspólny):
//!
//! ```text
//! <dane aplikacji>/voices/
//!   espeak-ng-data/            ← wspólny dla wszystkich głosów Piper
//!   justyna/
//!     model.onnx
//!     tokens.txt
//!     voice.json               ← opcjonalny opis (nazwa, licencja, jakość)
//! ```
//!
//! Modele Piper trzeba raz skonwertować do tego układu — dopisać metadane do `.onnx`, wygenerować
//! `tokens.txt` z `phoneme_id_map` i skopiować `espeak-ng-data`. Aplikacja czyta już tylko gotowe.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsModelConfig,
    OfflineTtsVitsModelConfig,
};
use tracing::{info, warn};

use crate::tts::{Clip, ClipCache, TtsError, TtsProvider, VoiceTuning, read_clip, write_wav};

/// Nazwa pliku z modelem akustycznym wewnątrz katalogu głosu.
const MODEL_FILE_NAME: &str = "model.onnx";
/// Nazwa pliku z tablicą tokenów (fonem → identyfikator).
const TOKENS_FILE_NAME: &str = "tokens.txt";
/// Nazwa wspólnego katalogu z danymi espeak-ng.
const DATA_DIR_NAME: &str = "espeak-ng-data";
/// Opcjonalny opis głosu — uzupełnia to, czego nie ma w samych plikach modelu.
const METADATA_FILE_NAME: &str = "voice.json";

/// Ile wątków dostaje synteza. Dwa wystarczają (synteza jest krótsza niż czas rzeczywisty),
/// a nie zabierają całego procesora aplikacji, która w tym czasie obsługuje imprezę.
const SYNTHESIS_THREADS: i32 = 2;

/// Jakość modelu — deklarowana przez głos, a gdy jej nie ma, odczytana z nazwy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceQuality {
    Low,
    Medium,
    High,
    /// Nie wiadomo — DJ i tak zobaczy, jak głos brzmi, na podglądzie.
    Unknown,
}

impl VoiceQuality {
    /// Rozpoznaje jakość z nazwy identyfikatora, np. `pl_PL-justyna_wg_glos-medium`.
    fn from_id(id: &str) -> Self {
        let id = id.to_ascii_lowercase();

        for (needle, quality) in [
            ("-low", Self::Low),
            ("-medium", Self::Medium),
            ("-high", Self::High),
        ] {
            if id.contains(needle) {
                return quality;
            }
        }

        Self::Unknown
    }

    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            _ => Self::Unknown,
        }
    }
}

/// Zainstalowany głos lektora.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Voice {
    /// Identyfikator = nazwa katalogu. Trafia do ustawień i do klucza pamięci podręcznej.
    pub id: String,
    /// Nazwa do pokazania DJ-owi.
    pub name: String,
    pub quality: VoiceQuality,
    /// Rozmiar modelu w bajtach.
    pub size_bytes: u64,
    /// Licencja, jeśli głos ją deklaruje.
    pub license: Option<String>,
    #[serde(skip)]
    model: PathBuf,
    #[serde(skip)]
    tokens: PathBuf,
    #[serde(skip)]
    data_dir: PathBuf,
}

/// Opis głosu z pliku `voice.json`.
#[derive(Debug, Default, Deserialize)]
struct VoiceMetadata {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    quality: Option<String>,
}

/// Domyślny katalog z głosami w danych aplikacji.
pub fn default_voices_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("voices")
}

/// Wypisuje głosy znalezione w katalogu, alfabetycznie po identyfikatorze.
///
/// Głos bez modelu albo bez tablicy tokenów jest nieużywalny — pomijamy go z ostrzeżeniem.
/// Brak katalogu to nie błąd: świeża instalacja po prostu nie ma jeszcze żadnego głosu,
/// a aplikacja ma wystartować i pokazać dedykację do przeczytania.
pub fn discover_voices(voices_dir: &Path) -> Result<Vec<Voice>, TtsError> {
    let data_dir = voices_dir.join(DATA_DIR_NAME);

    let entries = match std::fs::read_dir(voices_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(TtsError::io(voices_dir, error)),
    };

    let mut voices = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| TtsError::io(voices_dir, error))?;
        let dir = entry.path();

        if !dir.is_dir() {
            continue;
        }

        let model = dir.join(MODEL_FILE_NAME);
        let tokens = dir.join(TOKENS_FILE_NAME);

        if !model.is_file() || !tokens.is_file() {
            warn!(voice = %dir.display(), "pomijam niekompletny głos lektora");

            continue;
        }

        let Some(id) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        let metadata = read_metadata(&dir);
        let size_bytes = std::fs::metadata(&model)
            .map(|meta| meta.len())
            .unwrap_or(0);

        voices.push(Voice {
            name: metadata.name.unwrap_or_else(|| id.to_string()),
            quality: metadata
                .quality
                .as_deref()
                .map(VoiceQuality::parse)
                .unwrap_or_else(|| VoiceQuality::from_id(id)),
            license: metadata.license,
            id: id.to_string(),
            size_bytes,
            model,
            tokens,
            data_dir: data_dir.clone(),
        });
    }

    voices.sort_by(|left, right| left.id.cmp(&right.id));

    Ok(voices)
}

fn read_metadata(dir: &Path) -> VoiceMetadata {
    let path = dir.join(METADATA_FILE_NAME);

    let Ok(raw) = std::fs::read_to_string(&path) else {
        return VoiceMetadata::default();
    };

    match serde_json::from_str(&raw) {
        Ok(metadata) => metadata,
        Err(error) => {
            warn!(
                file = %path.display(),
                error = %error,
                "nieczytelny opis głosu, korzystam z samych plików modelu"
            );

            VoiceMetadata::default()
        }
    }
}

/// Głos gotowy do syntezy. Silnik powstaje dopiero przy pierwszej dedykacji i jest trzymany
/// w pamięci, bo wczytanie modelu trwa kilka sekund (na sprzęcie docelowym ~4 s).
pub struct PiperTts {
    voice: Voice,
    cache: ClipCache,
    engine: Mutex<Option<Engine>>,
}

/// Silnik wraz z parametrami barwy, przy których powstał.
struct Engine {
    /// Para (`noise_scale`, `noise_w`) w promilach — zmiana wymaga nowego silnika.
    noise: (u32, u32),
    tts: OfflineTts,
}

impl Engine {
    fn create(voice: &Voice, tuning: &VoiceTuning) -> Result<Self, TtsError> {
        let config = OfflineTtsConfig {
            model: OfflineTtsModelConfig {
                vits: OfflineTtsVitsModelConfig {
                    model: Some(path_string(&voice.model)),
                    tokens: Some(path_string(&voice.tokens)),
                    data_dir: Some(path_string(&voice.data_dir)),
                    // Barwę i rytm silnik przyjmuje tylko przy tworzeniu, nie na żądanie.
                    noise_scale: tuning.noise_scale(),
                    noise_scale_w: tuning.noise_w(),
                    ..Default::default()
                },
                num_threads: SYNTHESIS_THREADS,
                ..Default::default()
            },
            // Jedno zdanie naraz: dedykacja to kilka zdań, a mniejszy bufor to mniej pamięci.
            max_num_sentences: 1,
            ..Default::default()
        };

        let tts = OfflineTts::create(&config).ok_or_else(|| {
            warn!(voice = %voice.id, model = %voice.model.display(), "nie udało się wczytać głosu");

            TtsError::VoiceUnavailable(voice.id.clone())
        })?;

        info!(voice = %voice.id, "głos lektora wczytany");

        Ok(Self {
            noise: (tuning.noise_scale_milli, tuning.noise_w_milli),
            tts,
        })
    }
}

impl PiperTts {
    /// Tworzy providera. Silnik powstanie leniwie — dzięki temu start aplikacji nie czeka
    /// na wczytanie modelu.
    pub fn new(voice: Voice, cache: ClipCache) -> Self {
        Self {
            voice,
            cache,
            engine: Mutex::new(None),
        }
    }
}

/// Stan lektora w aplikacji: gotowy provider albo powód, dla którego go nie ma.
#[derive(Clone)]
pub struct TtsRuntime {
    provider: Option<Arc<dyn TtsProvider>>,
    voices: Vec<Voice>,
    selected: Option<String>,
    unavailable: Option<String>,
}

impl TtsRuntime {
    /// Przygotowuje lektora na podstawie znalezionych głosów.
    ///
    /// Bierzemy głos wskazany w ustawieniach, a gdy go nie ma — pierwszy z listy. Brak wybranego
    /// głosu nie może wywracać aplikacji (PLAN.md, sekcja 8).
    pub fn prepare(voices: Vec<Voice>, preferred: &str, cache: ClipCache) -> Self {
        if voices.is_empty() {
            return Self::unavailable("nie znaleziono żadnego głosu lektora".to_string());
        }

        let chosen = voices
            .iter()
            .position(|voice| voice.id == preferred)
            .unwrap_or(0);
        let voice = voices[chosen].clone();

        if voice.id != preferred {
            warn!(
                preferred,
                used = %voice.id,
                "wskazany głos lektora jest niedostępny, biorę pierwszy z listy"
            );
        }

        Self {
            provider: Some(Arc::new(PiperTts::new(voice.clone(), cache))),
            voices,
            selected: Some(voice.id),
            unavailable: None,
        }
    }

    /// Lektor wyłączony. Aplikacja działa dalej — DJ przeczyta dedykację z ekranu.
    pub fn unavailable(reason: String) -> Self {
        Self {
            provider: None,
            voices: Vec::new(),
            selected: None,
            unavailable: Some(reason),
        }
    }

    /// Czy lektor jest gotowy do czytania dedykacji.
    pub fn is_available(&self) -> bool {
        self.provider.is_some()
    }

    pub fn voices(&self) -> &[Voice] {
        &self.voices
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn unavailable_reason(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }
}

impl TtsProvider for PiperTts {
    fn voice_id(&self) -> &str {
        &self.voice.id
    }

    fn synthesize(&self, text: &str, tuning: &VoiceTuning) -> Result<Clip, TtsError> {
        let path = self.cache.path_for(&self.voice.id, text, tuning);

        // Klip policzony wcześniej — nie ma po co uruchamiać silnika.
        if path.is_file() {
            return read_clip(&path);
        }

        let mut guard = self
            .engine
            .lock()
            .map_err(|_| TtsError::Synthesis("silnik lektora jest niedostępny".to_string()))?;

        let noise = (tuning.noise_scale_milli, tuning.noise_w_milli);

        if guard.as_ref().map(|engine| engine.noise) != Some(noise) {
            *guard = Some(Engine::create(&self.voice, tuning)?);
        }

        let engine = guard
            .as_ref()
            .ok_or_else(|| TtsError::Synthesis("silnik lektora nie jest gotowy".to_string()))?;

        let generation = GenerationConfig {
            // W Piperze `length_scale` jest odwrotnością tempa: większe znaczy wolniej.
            speed: 1.0 / tuning.length_scale().max(0.1),
            // `silence_scale` to mnożnik pauzy, więc nasze milisekundy przeliczamy na krotność.
            silence_scale: tuning.sentence_silence_ms as f32
                / crate::tts::DEFAULT_SENTENCE_SILENCE_MS as f32,
            sid: 0,
            ..Default::default()
        };

        let audio = engine
            .tts
            .generate_with_config(text, &generation, None::<fn(&[f32], f32) -> bool>)
            .ok_or_else(|| TtsError::Synthesis("silnik nie zwrócił dźwięku".to_string()))?;

        let sample_rate = u32::try_from(audio.sample_rate()).unwrap_or(0);

        write_wav(&path, audio.samples(), sample_rate)
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::TtsProvider;

    /// Świeży katalog testowy — czyścimy go, żeby wynik nie zależał od poprzedniego przebiegu.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anon-dj-piper-{name}"));

        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("katalog testowy");

        dir
    }

    /// Zakłada katalog głosu. `complete` decyduje, czy dołożyć komplet plików modelu.
    fn voice_dir(root: &Path, id: &str, complete: bool, metadata: Option<&str>) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).expect("katalog głosu");

        if complete {
            std::fs::write(dir.join(MODEL_FILE_NAME), vec![0_u8; 1_024]).expect("model");
            std::fs::write(dir.join(TOKENS_FILE_NAME), "a 0\n").expect("tokeny");
        }

        if let Some(metadata) = metadata {
            std::fs::write(dir.join(METADATA_FILE_NAME), metadata).expect("opis głosu");
        }
    }

    #[test]
    fn voices_are_discovered_sorted_and_described() {
        let root = temp_dir("discover");

        voice_dir(&root, "pl_PL-justyna_wg_glos-medium", true, None);
        voice_dir(
            &root,
            "jarvis",
            true,
            Some(r#"{ "name": "Jarvis", "license": "MIT", "quality": "high" }"#),
        );
        // Niekompletny głos nie może trafić na listę.
        voice_dir(&root, "niedokonczony", false, None);

        let voices = discover_voices(&root).expect("wykrywanie głosów");

        assert_eq!(voices.len(), 2, "niekompletny głos pomijamy");
        assert_eq!(voices[0].id, "jarvis", "lista jest alfabetyczna");
        assert_eq!(voices[1].id, "pl_PL-justyna_wg_glos-medium");

        let jarvis = &voices[0];

        assert_eq!(jarvis.name, "Jarvis");
        assert_eq!(jarvis.license.as_deref(), Some("MIT"));
        assert_eq!(jarvis.quality, VoiceQuality::High);
        assert_eq!(jarvis.size_bytes, 1_024);

        // Bez opisu nazwa to identyfikator, a jakość czytamy z nazwy pliku.
        let justyna = &voices[1];

        assert_eq!(justyna.name, "pl_PL-justyna_wg_glos-medium");
        assert_eq!(justyna.license, None);
        assert_eq!(justyna.quality, VoiceQuality::Medium);
    }

    #[test]
    fn a_missing_voices_directory_is_not_an_error() {
        let root = temp_dir("missing");
        let missing = root.join("nie-ma-takiego");

        assert!(
            discover_voices(&missing).expect("brak katalogu").is_empty(),
            "świeża instalacja nie ma głosów i aplikacja ma mimo to wystartować"
        );
    }

    #[test]
    fn a_broken_metadata_file_falls_back_to_the_files() {
        let root = temp_dir("broken-metadata");

        voice_dir(
            &root,
            "pl_PL-meski_wg_glos-low",
            true,
            Some("{ to nie json }"),
        );

        let voices = discover_voices(&root).expect("wykrywanie głosów");

        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].name, "pl_PL-meski_wg_glos-low");
        assert_eq!(voices[0].quality, VoiceQuality::Low);
    }

    #[test]
    fn a_cached_clip_is_returned_without_touching_the_engine() {
        let root = temp_dir("cached");
        let voices = {
            voice_dir(&root, "justyna", true, None);
            discover_voices(&root).expect("wykrywanie głosów")
        };

        let cache = ClipCache::new(root.join("klipy"));
        let provider = PiperTts::new(voices[0].clone(), cache);
        let tuning = VoiceTuning::default();
        let text = "Dla Kasi i Marka, sto lat!";

        // Klip czeka już na dysku — silnik nie ma prawa się uruchomić, bo model jest atrapą.
        write_wav(
            &provider.cache.path_for(provider.voice_id(), text, &tuning),
            &vec![0.0_f32; 4_000],
            8_000,
        )
        .expect("zapis klipu");

        let clip = provider
            .synthesize(text, &tuning)
            .expect("klip z pamięci podręcznej");

        assert_eq!(clip.duration_ms, 500);
        assert_eq!(clip.sample_rate, 8_000);
        assert!(
            provider.engine.lock().expect("blokada silnika").is_none(),
            "pamięć podręczna nie może uruchamiać silnika"
        );
    }

    #[test]
    fn the_runtime_falls_back_to_an_available_voice() {
        let root = temp_dir("runtime");

        voice_dir(&root, "alpha", true, None);
        voice_dir(&root, "beta", true, None);

        let voices = discover_voices(&root).expect("wykrywanie głosów");
        let cache = ClipCache::new(root.join("klipy"));

        // Wskazanego głosu nie ma na liście — bierzemy pierwszy dostępny.
        let runtime = TtsRuntime::prepare(voices.clone(), "nie-ma-takiego", cache.clone());

        assert!(runtime.is_available());
        assert_eq!(runtime.selected(), Some("alpha"));
        assert_eq!(runtime.unavailable_reason(), None);
        assert_eq!(runtime.voices().len(), 2);

        // Wskazany głos istnieje — jego właśnie używamy.
        assert_eq!(
            TtsRuntime::prepare(voices, "beta", cache).selected(),
            Some("beta")
        );
    }

    #[test]
    fn the_runtime_without_voices_is_unavailable_but_alive() {
        let runtime = TtsRuntime::prepare(Vec::new(), "justyna", ClipCache::new(PathBuf::new()));

        assert!(!runtime.is_available());
        assert_eq!(runtime.selected(), None);
        assert!(runtime.unavailable_reason().is_some());
    }
    /// Synteza prawdziwym modelem — jedyny test, który dotyka `sherpa-onnx`.
    ///
    /// Modele nie leżą w repozytorium, więc test jest domyślnie pomijany. Uruchomienie:
    ///
    /// ```text
    /// ANON_DJ_VOICES_DIR=<katalog z głosami> cargo test -p dj the_real_engine -- --ignored --nocapture
    /// ```
    ///
    /// `ANON_DJ_VOICE` wybiera głos (domyślnie `justyna`), a `ANON_DJ_SAMPLES_DIR` każe zapisać
    /// policzony klip pod czytelną nazwą — dzięki temu można odsłuchać głosy jeden po drugim.
    #[test]
    #[ignore = "wymaga katalogu z prawdziwym głosem (ANON_DJ_VOICES_DIR)"]
    fn the_real_engine_synthesizes_a_dedication_and_reuses_the_clip() {
        let dir = std::env::var("ANON_DJ_VOICES_DIR")
            .expect("ustaw ANON_DJ_VOICES_DIR na katalog z głosami");
        let dir = PathBuf::from(dir);
        let preferred = std::env::var("ANON_DJ_VOICE").unwrap_or_else(|_| "justyna".to_string());

        let voices = discover_voices(&dir).expect("wykrywanie głosów");
        assert!(!voices.is_empty(), "w katalogu nie ma żadnego głosu");

        let cache = ClipCache::new(dir.join("klipy"));
        let runtime = TtsRuntime::prepare(voices, &preferred, cache.clone());

        assert_eq!(
            runtime.selected(),
            Some(preferred.as_str()),
            "głos {preferred} nie został wybrany — czy na pewno jest w katalogu?"
        );

        let provider = runtime.provider.clone().expect("provider lektora");

        // Pierwsza dedykacja: silnik startuje i liczy klip.
        let tuning = VoiceTuning::default();
        let text = "Kochani, mamy dla was dedykację. Kasia i Marek, z okazji waszego wesela \
                    życzymy wam wszystkiego najlepszego. Niech ta muzyka gra dla was do samego rana. \
                    Sto lat!";

        let first = provider.synthesize(text, &tuning).expect("synteza");

        assert!(first.path.is_file(), "klip musi trafić na dysk");
        assert!(first.duration_ms > 1_000, "nagranie nie może być puste");
        assert!(first.sample_rate > 8_000);

        // Druga próba ma wrócić z pamięci podręcznej — ten sam plik, bez syntezy.
        let second = provider.synthesize(text, &tuning).expect("odczyt klipu");

        assert_eq!(first.path, second.path);
        assert_eq!(first.duration_ms, second.duration_ms);
        assert_eq!(
            cache.path_for(provider.voice_id(), text, &tuning),
            first.path
        );

        // Na życzenie zapisujemy klip pod czytelną nazwą, żeby można było go odsłuchać.
        if let Ok(samples_dir) = std::env::var("ANON_DJ_SAMPLES_DIR") {
            let samples_dir = PathBuf::from(samples_dir);
            std::fs::create_dir_all(&samples_dir).expect("katalog na próbki");

            let sample = samples_dir.join(format!("{}.wav", provider.voice_id()));
            std::fs::copy(&first.path, &sample).expect("kopia próbki");

            println!(
                "próbka: {} ({:.1} s)",
                sample.display(),
                first.duration_ms as f32 / 1_000.0
            );
        }
    }

    #[test]
    fn voice_quality_is_read_from_the_identifier() {
        assert_eq!(
            VoiceQuality::from_id("pl_PL-justyna_wg_glos-medium"),
            VoiceQuality::Medium
        );
        assert_eq!(VoiceQuality::from_id("pl_PL-bass-high"), VoiceQuality::High);
        assert_eq!(
            VoiceQuality::from_id("pl_PL-mls_6892-low"),
            VoiceQuality::Low
        );
        assert_eq!(VoiceQuality::from_id("cokolwiek"), VoiceQuality::Unknown);
    }
}
