import { motion } from "framer-motion";
import { ChevronLeft, LoaderCircle, Send } from "lucide-react";

import { ui } from "../text";
import type { Limits, TrackInfo } from "../types";

type DedicationScreenProps = {
  track: TrackInfo;
  dedication: string;
  guestName: string;
  limits: Limits;
  sending: boolean;
  message: string | null;
  onDedicationChange: (value: string) => void;
  onGuestNameChange: (value: string) => void;
  onSubmit: () => void;
  onBack: () => void;
};

/** Liczba znaków liczona jak po stronie DJ-a — po znakach, nie po bajtach. */
function countChars(value: string): number {
  return [...value].length;
}

/** Ekran dedykacji: licznik znaków pokazuje limit ustawiony przez DJ-a. */
export function DedicationScreen({
  track,
  dedication,
  guestName,
  limits,
  sending,
  message,
  onDedicationChange,
  onGuestNameChange,
  onSubmit,
  onBack,
}: DedicationScreenProps) {
  const used = countChars(dedication);
  const overLimit = used > limits.dedication_max_chars;
  const canSend = dedication.trim() !== "" && !overLimit && !sending;

  return (
    <motion.div
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, ease: "easeOut" }}
      className="flex h-full items-center justify-center px-16 py-12"
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit();
        }}
        className="w-full max-w-3xl rounded-3xl border border-scena-800 bg-scena-900 p-10 shadow-2xl"
      >
        <div className="flex items-start gap-4">
          <button
            type="button"
            onClick={onBack}
            className="mt-1 flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-scena-700 text-zinc-400 transition-colors hover:bg-scena-800 hover:text-zinc-100"
            title={ui.dedication.back}
            aria-label={ui.dedication.back}
          >
            <ChevronLeft className="h-5 w-5" />
          </button>

          <div className="min-w-0">
            <p className="truncate text-xl text-zinc-100">{track.title}</p>
            <p className="truncate text-base text-zinc-400">
              {track.artist === "" ? ui.search.noArtist : track.artist}
            </p>
          </div>
        </div>

        <h1 className="mt-8 text-3xl font-semibold text-zinc-100">{ui.dedication.heading}</h1>
        <p className="mt-2 text-lg text-zinc-400">{ui.dedication.subheading}</p>

        <textarea
          autoFocus
          rows={4}
          value={dedication}
          placeholder={ui.dedication.placeholder}
          onChange={(event) => onDedicationChange(event.currentTarget.value)}
          className="mt-6 w-full resize-none rounded-2xl border border-scena-700 bg-scena-950 p-5 text-xl leading-snug text-zinc-100 outline-none placeholder:text-zinc-600 focus:border-zinc-500"
        />

        <p className={`mt-2 text-right text-sm ${overLimit ? "text-red-400" : "text-zinc-500"}`}>
          {used} / {limits.dedication_max_chars} {ui.dedication.counterLabel}
        </p>

        {limits.guest_name_max_chars > 0 && (
          <label className="mt-4 flex flex-col gap-2">
            <span className="text-sm font-medium text-zinc-300">
              {ui.dedication.guestNameLabel}
            </span>
            <input
              type="text"
              maxLength={limits.guest_name_max_chars}
              value={guestName}
              placeholder={ui.dedication.guestNamePlaceholder}
              onChange={(event) => onGuestNameChange(event.currentTarget.value)}
              className="h-14 w-full rounded-2xl border border-scena-700 bg-scena-950 px-5 text-lg text-zinc-100 outline-none placeholder:text-zinc-600 focus:border-zinc-500"
            />
          </label>
        )}

        <div className="mt-8 flex items-center gap-4">
          <button
            type="submit"
            disabled={!canSend}
            className="flex h-14 items-center gap-3 rounded-2xl bg-zinc-100 px-8 text-lg font-semibold text-scena-950 transition-colors hover:bg-white disabled:opacity-50"
          >
            {sending ? (
              <LoaderCircle className="h-6 w-6 animate-spin" />
            ) : (
              <Send className="h-6 w-6" />
            )}
            {sending ? ui.dedication.sending : ui.dedication.submit}
          </button>

          {message !== null && <p className="text-lg text-red-400">{message}</p>}
        </div>
      </form>
    </motion.div>
  );
}
