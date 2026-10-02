/**
 * Typy wymieniane między interfejsem DJ-a a warstwą Rust. Muszą zgadzać się z `QueuedRequest`
 * w `src-tauri/src/db.rs` oraz ze statusami w crate `protocol`.
 */

/** Status prośby gościa (PLAN.md, sekcja 6). */
export type RequestStatus = "submitted" | "approved" | "playing" | "done" | "rejected";

/** Prośba gościa widziana przez DJ-a. */
export type QueuedRequest = {
  id: number;
  track_id: number;
  title: string;
  artist: string;
  /** Tekst po edycji DJ-a; dopóki jej nie ma — tekst wpisany przez gościa. */
  dedication: string;
  guest_name: string | null;
  status: RequestStatus;
  created_at: number;
  /** Czas ostatniej zmiany — po nim sortowana jest historia. */
  updated_at: number;
  /** Ścieżka gotowego klipu lektora; `null`, dopóki voice-over nie jest policzony. */
  tts_clip_path: string | null;
};

/** Stan klipu lektora raportowany przez warstwę Rust — `ClipStatus` w `src-tauri/src/clips.rs`. */
export type ClipStatus = "generating" | "ready" | "failed";

/** Postęp generowania klipu lektora — `ClipProgress` w `src-tauri/src/clips.rs`. */
export type ClipProgress = {
  request_id: number;
  status: ClipStatus;
  /** Postęp syntezy w procentach (0–100). */
  progress: number;
  /** Powód niepowodzenia; `null`, gdy wszystko poszło dobrze. */
  message: string | null;
};

/** Stan voice-overu widziany przez kartę prośby — policzony, w toku, nieudany albo brak. */
export type VoiceOverState =
  | { status: "missing" }
  | { status: "generating"; progress: number }
  | { status: "ready" }
  | { status: "failed"; message: string | null };

/** Limity tekstów ustawione przez DJ-a — zgadzają się z typem `Limits` w `crate protocol`. */
export type Limits = {
  dedication_max_chars: number;
  guest_name_max_chars: number;
  search_query_max_chars: number;
};

/** Strojenie głosu lektora — zgadza się z typem `VoiceTuning` w `src-tauri/src/tts.rs`. */
export type VoiceTuning = {
  /** Tempo mowy w promilach: 1000 = normalne. */
  length_scale_milli: number;
  noise_scale_milli: number;
  noise_w_milli: number;
  /** Głośność w procentach: 100 = normalna. */
  volume_percent: number;
  /** Pauza między zdaniami w milisekundach. */
  sentence_silence_ms: number;
};

/** Ustawienia lektora — zgadzają się z typem `TtsSettings` w `src-tauri/src/settings.rs`. */
export type TtsSettings = {
  voice: string;
  tuning: VoiceTuning;
};

/** Ustawienia dźwięku — zgadzają się z typem `AudioSettings` w `src-tauri/src/settings.rs`. */
export type AudioSettings = {
  /** Identyfikator urządzenia wyjściowego na antenie; pusty oznacza domyślne urządzenie systemowe. */
  output_device_id: string;
  /** Identyfikator urządzenia odsłuchu DJ-a; pusty oznacza „to samo co na antenie”. */
  preview_device_id: string;
};

/** Ustawienia odtwarzacza — zgadzają się z typem `PlayerSettings` w `src-tauri/src/settings.rs`. */
export type PlayerSettings = {
  /** Bazowy adres API beefweb, np. `http://localhost:8880`. */
  base_url: string;
};

/**
 * Sposób, w jaki stary utwór ustępuje zamówionemu — zgadza się z `Handover`
 * w `src-tauri/src/settings.rs`.
 */
export type Handover = "fade" | "swap";

/**
 * Strojenie sekwencji wykonania — zgadza się z typem `ExecutionSettings`
 * w `src-tauri/src/settings.rs`. Wszystkie czasy w milisekundach.
 */
export type ExecutionSettings = {
  /** Poziom ducku w procentach amplitudy (0 = cisza, 100 = brak ściszenia). */
  duck_percent: number;
  /** Jak długo ściszamy do poziomu ducku. */
  duck_ramp_ms: number;
  /** Pauza po dedykacji (tylko przy przejściu „fade”). */
  gap_ms: number;
  /** Jak długo wyciszamy stary utwór do zera. */
  fade_out_ms: number;
  /** Jak długo zamówiony utwór wjeżdża do normalnej głośności. */
  start_ramp_ms: number;
  /** Kiedy muzyka zmienia się pod lektorem. */
  handover: Handover;
};

/** Ustawienia DJ-a — zgadzają się z typem `AppSettings` w `src-tauri/src/settings.rs`. */
export type AppSettings = {
  pin: string;
  port: number;
  confirmation_seconds: number;
  limits: Limits;
  tts: TtsSettings;
  audio: AudioSettings;
  player: PlayerSettings;
  execute: ExecutionSettings;
};

/** Urządzenie wyjściowe dźwięku — zgadza się z `OutputDevice` w `src-tauri/src/audio.rs`. */
export type OutputDevice = {
  /** Identyfikator urządzenia; to zapisujemy w ustawieniach. */
  id: string;
  /** Czytelna nazwa pokazywana DJ-owi. */
  name: string;
  /** Czy to domyślne urządzenie systemowe. */
  is_default: boolean;
};

/** Stan odtwarzania voice-overu — zgadza się z `PlaybackStatus` w `src-tauri/src/audio.rs`. */
export type PlaybackStatus = {
  /** Prośba, której voice-overu słuchamy; `null`, gdy nic nie leci albo leci próbka głosu. */
  request_id: number | null;
  /** Czy leci próbka głosu z ekranu lektora. */
  preview: boolean;
};

/** Jakość modelu głosu — zgadza się z `VoiceQuality` w `src-tauri/src/piper.rs`. */
export type VoiceQuality = "low" | "medium" | "high" | "unknown";

/** Zainstalowany głos lektora — zgadza się z `Voice` w `src-tauri/src/piper.rs`. */
export type Voice = {
  /** Identyfikator = nazwa katalogu; to zapisujemy w ustawieniach. */
  id: string;
  /** Nazwa do pokazania DJ-owi. */
  name: string;
  quality: VoiceQuality;
  /** Rozmiar modelu w bajtach. */
  size_bytes: number;
  /** Licencja, jeśli głos ją deklaruje. */
  license: string | null;
};

/** Stan lektora — zgadza się z `TtsStatus` w `src-tauri/src/lib.rs`. */
export type TtsStatus = {
  /** Czy da się przeczytać dedykację. */
  available: boolean;
  /** Głos, którym aplikacja czyta teraz; `null`, gdy lektora nie ma. */
  selected: string | null;
  /** Dlaczego lektora nie ma — DJ ma wiedzieć, co poprawić. */
  reason: string | null;
  /** Głosy znalezione na dysku. */
  voices: Voice[];
};

/**
 * Raport awarii z poprzedniego uruchomienia — zgadza się z `CrashReport` w `src-tauri/src/lib.rs`.
 * `null` (brak wartości) oznacza, że poprzedni start skończył się czysto.
 */
export type CrashReport = {
  /** Cała treść raportu — do pokazania i skopiowania. */
  text: string;
  /** Ścieżka pliku z raportem, gdyby DJ wolał wysłać plik zamiast wklejać tekst. */
  path: string;
};

/** Wynik podglądu głosu — zgadza się z `VoicePreview` w `src-tauri/src/lib.rs`. */
export type VoicePreview = {
  /** Ile trwała synteza próbki. */
  synthesis_ms: number;
  /** Długość nagrania. */
  duration_ms: number;
};
