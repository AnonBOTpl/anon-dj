import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { motion } from "framer-motion";
import { Check, LoaderCircle, Mic, Play, Save, Square } from "lucide-react";

import { ui } from "../text";
import type {
  AppSettings,
  PlaybackStatus,
  TtsStatus,
  Voice,
  VoicePreview,
  VoiceTuning,
} from "../types";

/** Zdarzenie o odtwarzanym voice-overze — `EVENT_PLAYBACK` w `src-tauri/src/audio.rs`. */
const EVENT_PLAYBACK = "audio://playback";

/**
 * Suwaki strojenia. Zakresy muszą zgadzać się ze stałymi w `src-tauri/src/settings.rs` —
 * poza nimi zapis ustawień i podgląd odrzucą wartość.
 */
const tuningFields: {
  key: keyof VoiceTuning;
  label: string;
  hint: string;
  min: number;
  max: number;
  step: number;
  unit: string;
}[] = [
  {
    key: "length_scale_milli",
    label: ui.voice.tempoLabel,
    hint: ui.voice.tempoHint,
    min: 500,
    max: 2000,
    step: 25,
    unit: "‰",
  },
  {
    key: "noise_scale_milli",
    label: ui.voice.noiseLabel,
    hint: ui.voice.noiseHint,
    min: 0,
    max: 1500,
    step: 25,
    unit: "‰",
  },
  {
    key: "noise_w_milli",
    label: ui.voice.noiseWLabel,
    hint: ui.voice.noiseWHint,
    min: 0,
    max: 1500,
    step: 25,
    unit: "‰",
  },
  {
    key: "volume_percent",
    label: ui.voice.volumeLabel,
    hint: ui.voice.volumeHint,
    min: 0,
    max: 200,
    step: 5,
    unit: "%",
  },
  {
    key: "sentence_silence_ms",
    label: ui.voice.silenceLabel,
    hint: ui.voice.silenceHint,
    min: 0,
    max: 2000,
    step: 25,
    unit: " ms",
  },
];

/** Rozmiar modelu w megabajtach — DJ patrzy, ile miejsca zajmuje głos, a nie ile ma bajtów. */
function formatSize(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(0)} MB`;
}

/** Czas trwania w sekundach, z jednym miejscem po przecinku. */
function formatSeconds(milliseconds: number): string {
  return `${(milliseconds / 1000).toFixed(1)} s`;
}

/**
 * Ekran lektora (PLAN.md, sekcja 8): lista zainstalowanych głosów, strojenie i próbka.
 *
 * Nic nie stosuje się do ustawień, dopóki DJ nie naciśnie „Zapisz” — do tego czasu może bez
 * konsekwencji przeskakiwać między głosami i porównywać próbki.
 */
export function VoiceView() {
  const [snapshot, setSnapshot] = useState<AppSettings | null>(null);
  const [voices, setVoices] = useState<Voice[]>([]);
  const [activeVoice, setActiveVoice] = useState<string | null>(null);
  const [voiceId, setVoiceId] = useState("");
  const [tuning, setTuning] = useState<VoiceTuning | null>(null);
  const [preview, setPreview] = useState<VoicePreview | null>(null);
  const [playing, setPlaying] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refreshStatus = useCallback(async () => {
    const status = await invoke<TtsStatus>("tts_status");

    setVoices(status.voices);
    setActiveVoice(status.selected);
  }, []);

  useEffect(() => {
    let active = true;

    invoke<AppSettings>("settings_snapshot")
      .then((settings) => {
        if (!active) {
          return;
        }

        setSnapshot(settings);
        setVoiceId(settings.tts.voice);
        setTuning(settings.tts.tuning);
      })
      .catch((reason: unknown) => {
        if (active) {
          setError(`${ui.voice.loadError} ${String(reason)}`);
        }
      });

    void refreshStatus().catch((reason: unknown) => {
      if (active) {
        setError(`${ui.voice.voicesError} ${String(reason)}`);
      }
    });

    return () => {
      active = false;
    };
  }, [refreshStatus]);

  // Koniec próbki gasi przycisk sam — warstwa Rust melduje, gdy odtwarzanie się skończy.
  useEffect(() => {
    const unlisten = listen<PlaybackStatus>(EVENT_PLAYBACK, (event) => {
      setPlaying(event.payload.preview);
    });

    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  async function togglePreview() {
    setError(null);

    if (playing) {
      try {
        await invoke("stop_dedication");
      } catch (reason: unknown) {
        setError(`${ui.voice.previewStopError} ${String(reason)}`);
      }

      return;
    }

    if (tuning === null) {
      return;
    }

    setGenerating(true);

    try {
      const result = await invoke<VoicePreview>("preview_voice", {
        voice: voiceId,
        tuning,
        text: ui.voice.previewText,
      });

      setPreview(result);
    } catch (reason: unknown) {
      setError(`${ui.voice.previewError} ${String(reason)}`);
    } finally {
      setGenerating(false);
    }
  }

  async function handleSave() {
    if (snapshot === null || tuning === null) {
      setError(ui.voice.loadError);

      return;
    }

    setSaved(false);
    setError(null);
    setBusy(true);

    try {
      await invoke("save_settings", {
        settings: { ...snapshot, tts: { voice: voiceId, tuning } },
      });

      setSnapshot((current) => (current === null ? current : { ...current, tts: { voice: voiceId, tuning } }));
      await refreshStatus();
      setSaved(true);
    } catch (reason: unknown) {
      setError(`${ui.voice.saveError} ${String(reason)}`);
    } finally {
      setBusy(false);
    }
  }

  function changeTuning(key: keyof VoiceTuning, value: number) {
    setTuning((current) => (current === null ? current : { ...current, [key]: value }));
    setSaved(false);
  }

  return (
    <motion.div
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.18, ease: "easeOut" }}
      className="flex max-w-4xl flex-col gap-5"
    >
      <section className="rounded-lg border border-scena-800 bg-scena-900 p-5">
        <header>
          <h2 className="text-sm font-semibold text-zinc-200">{ui.voice.title}</h2>
          <p className="text-xs text-zinc-500">{ui.voice.hint}</p>
        </header>

        {error !== null && <p className="mt-3 text-xs text-red-400">{error}</p>}

        <h3 className="mt-4 text-xs font-medium text-zinc-300">{ui.voice.voicesTitle}</h3>

        {voices.length === 0 ? (
          <p className="mt-2 text-xs text-zinc-500">{ui.voice.voicesEmpty}</p>
        ) : (
          <ul className="mt-2 grid grid-cols-2 gap-2">
            {voices.map((voice) => {
              const chosen = voice.id === voiceId;

              return (
                <li key={voice.id}>
                  <button
                    type="button"
                    onClick={() => {
                      setVoiceId(voice.id);
                      setSaved(false);
                    }}
                    className={`flex w-full items-start gap-3 rounded border px-3 py-2 text-left transition-colors ${
                      chosen
                        ? "border-zinc-400 bg-scena-800"
                        : "border-scena-800 hover:border-scena-700 hover:bg-scena-800/60"
                    }`}
                  >
                    <Mic
                      className={`mt-0.5 h-4 w-4 shrink-0 ${chosen ? "text-zinc-100" : "text-zinc-500"}`}
                    />

                    <span className="min-w-0 flex-1">
                      <span className="flex items-baseline gap-2">
                        <span className="truncate text-sm font-medium text-zinc-100">
                          {voice.name}
                        </span>

                        {voice.id === activeVoice && (
                          <span className="shrink-0 rounded-full border border-emerald-700/60 px-2 py-0.5 text-[10px] text-emerald-300">
                            {ui.voice.activeTag}
                          </span>
                        )}
                      </span>

                      <span className="mt-0.5 block truncate text-[11px] text-zinc-500">
                        {ui.voice.quality[voice.quality]} · {formatSize(voice.size_bytes)}
                        {voice.license !== null &&
                          ` · ${ui.voice.licenseLabel}: ${voice.license}`}
                      </span>
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}

        <h3 className="mt-5 text-xs font-medium text-zinc-300">{ui.voice.tuningTitle}</h3>

        <div className="mt-2 grid grid-cols-2 gap-x-6 gap-y-4">
          {tuningFields.map((field) => (
            <label key={field.key} className="flex flex-col gap-1">
              <span className="flex items-baseline justify-between">
                <span className="text-xs text-zinc-300">{field.label}</span>
                <span className="text-xs tabular-nums text-zinc-500">
                  {tuning === null ? "—" : `${tuning[field.key]}${field.unit}`}
                </span>
              </span>
              <input
                type="range"
                min={field.min}
                max={field.max}
                step={field.step}
                disabled={tuning === null}
                value={tuning === null ? field.min : tuning[field.key]}
                onChange={(event) => changeTuning(field.key, Number(event.currentTarget.value))}
                className="accent-zinc-300"
              />
              <span className="text-[11px] text-zinc-500">{field.hint}</span>
            </label>
          ))}
        </div>
      </section>

      <section className="rounded-lg border border-scena-800 bg-scena-900 p-5">
        <h3 className="text-xs font-medium text-zinc-300">{ui.voice.previewTitle}</h3>
        <p className="mt-1 text-sm text-zinc-100">{ui.voice.previewText}</p>
        <p className="text-[11px] text-zinc-500">{ui.voice.previewSuffixNotice}</p>

        <div className="mt-3 flex items-center gap-3">
          <button
            type="button"
            onClick={() => void togglePreview()}
            disabled={generating || tuning === null || voiceId === ""}
            className="flex items-center gap-2 rounded bg-zinc-100 px-4 py-2 text-sm font-semibold text-scena-950 transition-colors hover:bg-white disabled:opacity-60"
          >
            {playing ? <Square className="h-4 w-4" /> : <Play className="h-4 w-4" />}
            {generating
              ? ui.voice.previewBusy
              : playing
                ? ui.voice.previewStop
                : ui.voice.previewPlay}
          </button>

          {generating && <LoaderCircle className="h-4 w-4 animate-spin text-zinc-400" />}

          {preview !== null && !generating && (
            <span className="text-xs tabular-nums text-zinc-500">
              {ui.voice.previewTook} {formatSeconds(preview.synthesis_ms)} ·{" "}
              {ui.voice.previewLength} {formatSeconds(preview.duration_ms)}
            </span>
          )}
        </div>

        <div className="mt-4 flex items-center gap-3">
          <button
            type="button"
            onClick={() => void handleSave()}
            disabled={busy || snapshot === null || tuning === null}
            className="flex items-center gap-2 rounded border border-scena-700 px-4 py-2 text-sm font-semibold text-zinc-100 transition-colors hover:bg-scena-800 disabled:opacity-60"
          >
            <Save className="h-4 w-4" />
            {busy ? ui.voice.saving : ui.voice.save}
          </button>

          {saved && (
            <motion.span
              initial={{ opacity: 0, x: -4 }}
              animate={{ opacity: 1, x: 0 }}
              className="flex items-center gap-1 text-sm text-emerald-400"
            >
              <Check className="h-4 w-4" />
              {ui.voice.saved}
            </motion.span>
          )}

          {error !== null && <span className="text-sm text-red-400">{error}</span>}
        </div>
      </section>
    </motion.div>
  );
}
