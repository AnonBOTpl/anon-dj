import { useEffect, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { motion } from "framer-motion";
import { Check, Save } from "lucide-react";

import { ui } from "../text";

/** Limity długości tekstów — muszą zgadzać się z typem `Limits` w crate `protocol`. */
type Limits = {
  dedication_max_chars: number;
  guest_name_max_chars: number;
  search_query_max_chars: number;
};

type AppSettings = {
  pin: string;
  limits: Limits;
};

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

export function SettingsView() {
  const [pin, setPin] = useState("");
  const [values, setValues] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;

    invoke<AppSettings>("settings_snapshot")
      .then((settings) => {
        if (!active) {
          return;
        }

        setPin(settings.pin);
        setValues({
          dedication_max_chars: String(settings.limits.dedication_max_chars),
          guest_name_max_chars: String(settings.limits.guest_name_max_chars),
          search_query_max_chars: String(settings.limits.search_query_max_chars),
        });
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

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSaved(false);
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

    setBusy(true);

    try {
      await invoke("save_settings", {
        settings: {
          pin,
          limits,
        },
      });
      setSaved(true);
    } catch (reason: unknown) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <motion.form
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.18, ease: "easeOut" }}
      onSubmit={handleSubmit}
      className="flex max-w-3xl flex-col gap-5 rounded-lg border border-scena-800 bg-scena-900 p-5"
    >
      <header>
        <h2 className="text-sm font-semibold text-zinc-200">{ui.settings.title}</h2>
        <p className="text-xs text-zinc-500">{ui.settings.hint}</p>
      </header>

      <label className="flex flex-col gap-1">
        <span className="text-xs font-medium text-zinc-300">{ui.settings.pinLabel}</span>
        <input
          type="text"
          inputMode="numeric"
          maxLength={6}
          value={pin}
          onChange={(event) => {
            setPin(event.currentTarget.value);
            setSaved(false);
          }}
          className="w-40 rounded border border-scena-700 bg-scena-950 px-3 py-2 font-mono text-lg tracking-[0.3em] text-zinc-100 outline-none focus:border-zinc-500"
        />
        <span className="text-xs text-zinc-500">{ui.settings.pinHint}</span>
      </label>

      <div className="grid grid-cols-3 gap-4">
        {numberFields.map((field) => (
          <label key={field.key} className="flex flex-col gap-1">
            <span className="text-xs font-medium text-zinc-300">{field.label}</span>
            <input
              type="number"
              min={0}
              value={values[field.key] ?? ""}
              onChange={(event) => {
                setValues((current) => ({
                  ...current,
                  [field.key]: event.currentTarget.value,
                }));
                setSaved(false);
              }}
              className="rounded border border-scena-700 bg-scena-950 px-3 py-2 text-zinc-100 outline-none focus:border-zinc-500"
            />
            {field.hint !== null && (
              <span className="text-xs text-zinc-500">{field.hint}</span>
            )}
          </label>
        ))}
      </div>

      <div className="flex items-center gap-3">
        <button
          type="submit"
          disabled={busy}
          className="flex items-center gap-2 rounded bg-zinc-100 px-4 py-2 text-sm font-semibold text-scena-950 transition-colors hover:bg-white disabled:opacity-60"
        >
          <Save className="h-4 w-4" />
          {busy ? ui.settings.saving : ui.settings.save}
        </button>

        {saved && (
          <motion.span
            initial={{ opacity: 0, x: -4 }}
            animate={{ opacity: 1, x: 0 }}
            className="flex items-center gap-1 text-sm text-emerald-400"
          >
            <Check className="h-4 w-4" />
            {ui.settings.saved}
          </motion.span>
        )}

        {error !== null && <span className="text-sm text-red-400">{error}</span>}
      </div>
    </motion.form>
  );
}
