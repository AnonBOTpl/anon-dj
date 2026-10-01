import { useEffect, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { motion } from "framer-motion";
import { Save, X } from "lucide-react";

import { ui } from "../text";
import type { AppSettings, ExecutionSettings, OutputDevice } from "../types";

/** Zakres czasu powrotu ekranu potwierdzenia — musi zgadzać się ze stałą w `settings.rs`. */
const CONFIRMATION_SECONDS_RANGE = { min: 5, max: 300 } as const;

/**
 * Pola strojenia sekwencji wykonania. Zakresy muszą zgadzać się ze stałymi w `settings.rs`
 * (`DUCK_PERCENT_RANGE` i `EXECUTE_MS_RANGE`) — poza nimi Rust odrzuci zapis.
 */
const sequenceFields: {
  key: keyof ExecutionSettings;
  label: string;
  hint: string;
  min: number;
  max: number;
}[] = [
  {
    key: "duck_percent",
    label: ui.settings.duckLevelLabel,
    hint: ui.settings.duckLevelHint,
    min: 0,
    max: 100,
  },
  {
    key: "duck_ramp_ms",
    label: ui.settings.duckRampLabel,
    hint: ui.settings.duckRampHint,
    min: 0,
    max: 5000,
  },
  {
    key: "gap_ms",
    label: ui.settings.gapLabel,
    hint: ui.settings.gapHint,
    min: 0,
    max: 5000,
  },
  {
    key: "fade_out_ms",
    label: ui.settings.fadeOutLabel,
    hint: ui.settings.fadeOutHint,
    min: 0,
    max: 5000,
  },
  {
    key: "start_ramp_ms",
    label: ui.settings.startRampLabel,
    hint: ui.settings.startRampHint,
    min: 0,
    max: 5000,
  },
];

const numberFields = [
  {
    key: "dedication_max_chars",
    label: ui.settings.dedicationLimitLabel,
    hint: ui.settings.dedicationLimitHint,
  },
  {
    key: "guest_name_max_chars",
    label: ui.settings.guestNameLimitLabel,
    hint: ui.settings.guestNameLimitHint,
  },
  {
    key: "search_query_max_chars",
    label: ui.settings.searchQueryLimitLabel,
    hint: null,
  },
] as const;

const input =
  "rounded border border-scena-700 bg-scena-950 px-3 py-2 text-zinc-100 outline-none focus:border-zinc-500";

/**
 * Ustawienia jako okno nad bieżącym widokiem.
 *
 * Modal, a nie osobna sekcja: ustawienia zmienia się raz na imprezę, a kolejki trzeba mieć pod
 * ręką cały czas. Otwarcie z dowolnego ekranu oznacza też, że DJ nie traci tego, co akurat robił —
 * wróciłby do widoku, który i tak zostałby przerysowany.
 *
 * „Zapisz” utrwala ustawienia i zamyka okno; zamknięcie bez zapisu nie rusza niczego.
 */
export function SettingsDialog({ onClose }: { onClose: () => void }) {
  const [pin, setPin] = useState("");
  const [port, setPort] = useState("");
  const [confirmationSeconds, setConfirmationSeconds] = useState("");
  const [values, setValues] = useState<Record<string, string>>({});
  const [audioDeviceId, setAudioDeviceId] = useState("");
  const [previewDeviceId, setPreviewDeviceId] = useState("");
  const [executeValues, setExecuteValues] = useState<Record<string, string>>({});
  const [devices, setDevices] = useState<OutputDevice[]>([]);
  // Czy lista urządzeń zdążyła się wczytać — dopiero wtedy umiemy powiedzieć, że zapisane
  // urządzenie zniknęło, a nie tylko że lista jest jeszcze pusta.
  const [devicesLoaded, setDevicesLoaded] = useState(false);
  const [devicesError, setDevicesError] = useState<string | null>(null);
  // Cały wczytany zestaw trzymamy obok pól formularza: zapisywanie wysyła komplet ustawień,
  // a ten formularz nie edytuje lektora — bez tego zapis kasowałby głos i jego strojenie.
  const [snapshot, setSnapshot] = useState<AppSettings | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;

    invoke<AppSettings>("settings_snapshot")
      .then((settings) => {
        if (!active) {
          return;
        }

        setSnapshot(settings);
        setPin(settings.pin);
        setPort(String(settings.port));
        setConfirmationSeconds(String(settings.confirmation_seconds));
        setAudioDeviceId(settings.audio.output_device_id);
        setPreviewDeviceId(settings.audio.preview_device_id);
        setValues({
          dedication_max_chars: String(settings.limits.dedication_max_chars),
          guest_name_max_chars: String(settings.limits.guest_name_max_chars),
          search_query_max_chars: String(settings.limits.search_query_max_chars),
        });
        setExecuteValues(
          Object.fromEntries(
            sequenceFields.map((field) => [field.key, String(settings.execute[field.key])]),
          ),
        );
      })
      .catch((reason: unknown) => {
        if (active) {
          setError(`${ui.settings.loadError} ${String(reason)}`);
        }
      });

    return () => {
      active = false;
    };
  }, []);

  // Lista urządzeń wyjściowych. Brak listy nie może zablokować ustawień — wtedy zostaje wybór
  // domyślnego urządzenia systemowego.
  useEffect(() => {
    let active = true;

    invoke<OutputDevice[]>("audio_devices")
      .then((listed) => {
        if (!active) {
          return;
        }

        setDevices(listed);
        setDevicesLoaded(true);
      })
      .catch((reason: unknown) => {
        if (active) {
          setDevicesError(`${ui.settings.audioDeviceError} ${String(reason)}`);
          setDevicesLoaded(true);
        }
      });

    return () => {
      active = false;
    };
  }, []);

  // Escape zamyka okno — DJ nie powinien szukać krzyżyka pod presją czasu.
  useEffect(() => {
    function handleKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }

    window.addEventListener("keydown", handleKey);

    return () => {
      window.removeEventListener("keydown", handleKey);
    };
  }, [onClose]);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);

    const limits: Record<string, number> = {};

    for (const field of numberFields) {
      const parsed = Number(values[field.key]);

      if (!Number.isFinite(parsed) || parsed < 0) {
        setError(ui.settings.numberError);

        return;
      }

      limits[field.key] = parsed;
    }

    const parsedPort = Number(port);

    if (!Number.isInteger(parsedPort) || parsedPort < 1024 || parsedPort > 65535) {
      setError(ui.settings.portError);

      return;
    }

    const parsedConfirmationSeconds = Number(confirmationSeconds);

    if (
      !Number.isInteger(parsedConfirmationSeconds) ||
      parsedConfirmationSeconds < CONFIRMATION_SECONDS_RANGE.min ||
      parsedConfirmationSeconds > CONFIRMATION_SECONDS_RANGE.max
    ) {
      setError(ui.settings.confirmationSecondsError);

      return;
    }

    if (snapshot === null) {
      setError(ui.settings.loadError);

      return;
    }

    // Strojenie sekwencji walidujemy tymi samymi zakresami, co Rust — komunikat jest wtedy
    // konkretny („podaj liczbę z zakresu przy polu X”), a nie ogólny błąd z warstwy Rust.
    const parsedExecute: ExecutionSettings = {
      duck_percent: 0,
      duck_ramp_ms: 0,
      gap_ms: 0,
      fade_out_ms: 0,
      start_ramp_ms: 0,
    };

    for (const field of sequenceFields) {
      const value = Number(executeValues[field.key]);

      if (!Number.isInteger(value) || value < field.min || value > field.max) {
        setError(`${ui.settings.sequenceNumberError} (${field.label})`);

        return;
      }

      parsedExecute[field.key] = value;
    }

    setBusy(true);

    try {
      await invoke("save_settings", {
        settings: {
          ...snapshot,
          pin,
          port: parsedPort,
          confirmation_seconds: parsedConfirmationSeconds,
          limits,
          audio: { output_device_id: audioDeviceId, preview_device_id: previewDeviceId },
          execute: parsedExecute,
        },
      });

      // Zapis zamyka okno: potwierdzeniem jest sam powrót do kolejek z nowym portem czy PIN-em
      // w pasku statusu.
      onClose();
    } catch (reason: unknown) {
      // Nieudany zapis zostawia okno otwarte — DJ nie traci tego, co wpisał.
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  // Zapisane urządzenie mogło zostać odłączone. Pokazujemy to wprost, zamiast po cichu
  // przestawiać DJ-a na domyślne wyjście — na imprezie to różnica między dźwiękiem w sali
  // a dźwiękiem w słuchawce.
  const deviceMissing =
    devicesLoaded && audioDeviceId !== "" && !devices.some((device) => device.id === audioDeviceId);
  const auditionDeviceMissing =
    devicesLoaded &&
    previewDeviceId !== "" &&
    !devices.some((device) => device.id === previewDeviceId);

  return (
    <div
      // Klik w tło zamyka, klik w kartę nie — i tylko wtedy, gdy wyszły z karty, a nie na niej.
      onClick={(event) => {
        if (event.target === event.currentTarget) {
          onClose();
        }
      }}
      className="fixed inset-0 z-50 flex items-center justify-center bg-scena-950/80 p-6"
    >
      <motion.form
        initial={{ opacity: 0, y: -8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.18, ease: "easeOut" }}
        onSubmit={handleSubmit}
        // Wysokość bierze się z treści: okno rośnie razem z liczbą ustawień, a nie przewija się
        // w środku. `max-h-full` z przewijaniem to tylko zabezpieczenie na bardzo niskie okno —
        // bez niego przyciski zapisu uciekłyby poza ekran.
        className="max-h-full w-full max-w-3xl overflow-y-auto rounded-lg border border-scena-800 bg-scena-900"
      >
        <header className="flex items-start gap-3 border-b border-scena-800 px-5 py-3">
          <div>
            <h2 className="text-sm font-semibold text-zinc-200">{ui.settings.title}</h2>
            <p className="text-xs text-zinc-500">{ui.settings.hint}</p>
          </div>

          <button
            type="button"
            onClick={onClose}
            title={ui.settings.close}
            aria-label={ui.settings.close}
            className="ml-auto flex h-7 w-7 shrink-0 items-center justify-center rounded text-zinc-400 transition-colors hover:bg-scena-700 hover:text-zinc-100"
          >
            <X className="h-4 w-4" />
          </button>
        </header>

        <div className="grid grid-cols-2 gap-x-6 gap-y-5 px-5 py-4">
          <section className="space-y-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-zinc-500">
              {ui.settings.connectionTitle}
            </h3>

            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-zinc-300">{ui.settings.pinLabel}</span>
              <input
                type="text"
                inputMode="numeric"
                maxLength={6}
                value={pin}
                onChange={(event) => setPin(event.currentTarget.value)}
                className={`w-40 font-mono text-lg tracking-[0.3em] ${input}`}
              />
              <span className="text-xs text-zinc-500">{ui.settings.pinHint}</span>
            </label>

            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-zinc-300">{ui.settings.portLabel}</span>
              <input
                type="number"
                min={1024}
                max={65535}
                value={port}
                onChange={(event) => setPort(event.currentTarget.value)}
                className={`w-40 ${input}`}
              />
              <span className="text-xs text-zinc-500">{ui.settings.portHint}</span>
            </label>

            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-zinc-300">
                {ui.settings.confirmationSecondsLabel}
              </span>
              <input
                type="number"
                min={CONFIRMATION_SECONDS_RANGE.min}
                max={CONFIRMATION_SECONDS_RANGE.max}
                value={confirmationSeconds}
                onChange={(event) => setConfirmationSeconds(event.currentTarget.value)}
                className={`w-40 ${input}`}
              />
              <span className="text-xs text-zinc-500">
                {ui.settings.confirmationSecondsHint}
              </span>
            </label>
          </section>

          <section className="space-y-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-zinc-500">
              {ui.settings.limitsTitle}
            </h3>

            {numberFields.map((field) => (
              <label key={field.key} className="flex flex-col gap-1">
                <span className="text-xs font-medium text-zinc-300">{field.label}</span>
                <input
                  type="number"
                  min={0}
                  value={values[field.key] ?? ""}
                  onChange={(event) => {
                    // Wartość czytamy **przed** updaterem: React woła funkcję aktualizującą
                    // po zakończeniu obsługi zdarzenia, kiedy `currentTarget` jest już `null`.
                    const next = event.currentTarget.value;

                    setValues((current) => ({ ...current, [field.key]: next }));
                  }}
                  className={`w-40 ${input}`}
                />
                {field.hint !== null && <span className="text-xs text-zinc-500">{field.hint}</span>}
              </label>
            ))}
          </section>

          <section className="col-span-2 space-y-4 border-t border-scena-800 pt-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-zinc-500">
              {ui.settings.audioTitle}
            </h3>

            <div className="grid grid-cols-2 gap-x-6 gap-y-4">
              <label className="flex flex-col gap-1">
                <span className="text-xs font-medium text-zinc-300">
                  {ui.settings.audioDeviceLabel}
                </span>
                <select
                  value={audioDeviceId}
                  onChange={(event) => setAudioDeviceId(event.currentTarget.value)}
                  className={`w-full ${input}`}
                >
                  <option value="">{ui.settings.audioDeviceDefault}</option>

                  {deviceMissing && (
                    <option value={audioDeviceId}>{ui.settings.audioDeviceMissing}</option>
                  )}

                  {devices.map((device) => (
                    <option key={device.id} value={device.id}>
                      {device.is_default
                        ? `${device.name} — ${ui.settings.audioDeviceDefaultTag}`
                        : device.name}
                    </option>
                  ))}
                </select>
                <span className="text-xs text-zinc-500">{ui.settings.audioDeviceHint}</span>
                {devicesError !== null && (
                  <span className="text-xs text-red-400">{devicesError}</span>
                )}
              </label>

              <label className="flex flex-col gap-1">
                <span className="text-xs font-medium text-zinc-300">
                  {ui.settings.auditionDeviceLabel}
                </span>
                <select
                  value={previewDeviceId}
                  onChange={(event) => setPreviewDeviceId(event.currentTarget.value)}
                  className={`w-full ${input}`}
                >
                  <option value="">{ui.settings.auditionDeviceSameAsOutput}</option>

                  {auditionDeviceMissing && (
                    <option value={previewDeviceId}>{ui.settings.audioDeviceMissing}</option>
                  )}

                  {devices.map((device) => (
                    <option key={device.id} value={device.id}>
                      {device.is_default
                        ? `${device.name} — ${ui.settings.audioDeviceDefaultTag}`
                        : device.name}
                    </option>
                  ))}
                </select>
                <span className="text-xs text-zinc-500">{ui.settings.auditionDeviceHint}</span>
              </label>
            </div>
          </section>

          <section className="col-span-2 space-y-4 border-t border-scena-800 pt-4">
            <div>
              <h3 className="text-xs font-semibold uppercase tracking-wider text-zinc-500">
                {ui.settings.sequenceTitle}
              </h3>
              <p className="text-xs text-zinc-500">{ui.settings.sequenceHint}</p>
            </div>

            <div className="grid grid-cols-2 gap-x-6 gap-y-4">
              {sequenceFields.map((field) => (
                <label key={field.key} className="flex flex-col gap-1">
                  <span className="text-xs font-medium text-zinc-300">{field.label}</span>
                  <input
                    type="number"
                    min={field.min}
                    max={field.max}
                    value={executeValues[field.key] ?? ""}
                    onChange={(event) => {
                      // Jak wyżej: `currentTarget` jest ważne tylko do końca obsługi zdarzenia.
                      const next = event.currentTarget.value;

                      setExecuteValues((current) => ({ ...current, [field.key]: next }));
                    }}
                    className={`w-40 ${input}`}
                  />
                  <span className="text-xs text-zinc-500">{field.hint}</span>
                </label>
              ))}
            </div>
          </section>
        </div>

        <footer className="flex items-center gap-3 border-t border-scena-800 px-5 py-3">
          <button
            type="submit"
            disabled={busy}
            className="flex items-center gap-2 rounded bg-zinc-100 px-4 py-2 text-sm font-semibold text-scena-950 transition-colors hover:bg-white disabled:opacity-60"
          >
            <Save className="h-4 w-4" />
            {busy ? ui.settings.saving : ui.settings.save}
          </button>

          <button
            type="button"
            onClick={onClose}
            className="rounded border border-scena-700 px-4 py-2 text-sm text-zinc-300 transition-colors hover:bg-scena-800"
          >
            {ui.settings.close}
          </button>

          {error !== null && <span className="text-sm text-red-400">{error}</span>}
        </footer>
      </motion.form>
    </div>
  );
}
