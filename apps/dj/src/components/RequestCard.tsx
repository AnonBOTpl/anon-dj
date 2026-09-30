import { useState, type MouseEvent, type ReactNode } from "react";
import { motion } from "framer-motion";
import { Check, Pencil, User, X } from "lucide-react";

import { ui } from "../text";
import type { QueuedRequest } from "../types";

/** Godzina zgłoszenia — DJ patrzy na kolejkę chronologicznie. */
function formatTime(timestamp: number): string {
  return new Date(timestamp).toLocaleTimeString("pl-PL", {
    hour: "2-digit",
    minute: "2-digit",
  });
}

type RequestCardProps = {
  request: QueuedRequest;
  /**
   * Zapis poprawionej dedykacji. `false` oznacza, że się nie udało i trzeba zostać w trybie
   * poprawiania. Brak funkcji oznacza kartę tylko do czytania (historia).
   */
  onSaveDedication?: (dedication: string) => Promise<boolean>;
  /** Przyciski decyzji DJ-a; brak oznacza kartę tylko do czytania. */
  actions?: ReactNode;
  /** Dopisek pod treścią — w historii mówi, co DJ zrobił z prośbą. */
  footnote?: string;
  /** Limit znaków dedykacji z ustawień DJ-a. */
  maxChars: number;
  /** Blokuje przyciski na czas operacji na bazie. */
  disabled?: boolean;
};

/**
 * Jedna prośba gościa. Dedykacja jest największym tekstem na ekranie — DJ czyta ją w hałasie
 * i pod presją czasu (AGENTS.md, konwencje frontendu). Poprawa tekstu zapisuje się od razu,
 * ale nic nie leci na antenę, dopóki DJ nie zatwierdzi prośby.
 */
export function RequestCard({
  request,
  onSaveDedication,
  actions,
  footnote,
  maxChars,
  disabled = false,
}: RequestCardProps) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(request.dedication);
  const [saving, setSaving] = useState(false);

  const editable = onSaveDedication !== undefined;
  const draftChars = [...draft].length;

  function startEditing(event: MouseEvent<HTMLButtonElement>) {
    event.stopPropagation();

    setDraft(request.dedication);
    setEditing(true);
  }

  function cancelEditing(event: MouseEvent<HTMLButtonElement>) {
    event.stopPropagation();

    setEditing(false);
  }

  async function saveDedication(event: MouseEvent<HTMLButtonElement>) {
    event.stopPropagation();

    if (onSaveDedication === undefined) {
      return;
    }

    setSaving(true);

    try {
      // Nieudany zapis zostawia tekst w polu — DJ nie traci tego, co napisał.
      if (await onSaveDedication(draft)) {
        setEditing(false);
      }
    } finally {
      setSaving(false);
    }
  }

  return (
    <motion.li
      initial={{ opacity: 0, y: 4 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.15, ease: "easeOut" }}
      className="rounded border border-scena-800 bg-scena-950 p-3"
    >
      {editing ? (
        <div className="space-y-1">
          <textarea
            value={draft}
            maxLength={maxChars}
            rows={3}
            autoFocus
            onChange={(event) => setDraft(event.currentTarget.value)}
            className="w-full resize-none rounded border border-scena-700 bg-scena-900 px-2 py-1.5 text-base leading-snug text-zinc-100 outline-none focus:border-zinc-500"
          />
          <p className="text-[11px] tabular-nums text-zinc-500">
            {draftChars}/{maxChars} {ui.queue.charsLabel}
          </p>
        </div>
      ) : (
        <p className="text-base leading-snug text-zinc-100">{request.dedication}</p>
      )}

      <div className="mt-2 flex items-baseline gap-3 text-xs text-zinc-500">
        <span className="min-w-0 flex-1 truncate text-zinc-400">
          {[request.title, request.artist].filter((part) => part !== "").join(" — ")}
        </span>

        {request.guest_name !== null && (
          <span className="flex shrink-0 items-center gap-1 text-zinc-400">
            <User className="h-3 w-3" />
            {request.guest_name}
          </span>
        )}

        <span className="shrink-0 tabular-nums">{formatTime(request.created_at)}</span>
      </div>

      {(editable || actions !== undefined || footnote !== undefined) && (
        <div className="mt-2 flex items-center gap-2">
          {footnote !== undefined && <span className="text-xs text-zinc-500">{footnote}</span>}

          <div className="ml-auto flex items-center gap-1.5">
            {editable &&
              (editing ? (
                <>
                  <button
                    type="button"
                    onClick={(event) => void saveDedication(event)}
                    disabled={disabled || saving}
                    className="flex items-center gap-1.5 rounded border border-scena-700 px-2 py-1 text-xs text-zinc-200 transition-colors hover:bg-scena-800 disabled:opacity-40"
                  >
                    <Check className="h-3.5 w-3.5" />
                    {ui.queue.saveEdit}
                  </button>
                  <button
                    type="button"
                    onClick={cancelEditing}
                    disabled={disabled || saving}
                    className="flex items-center gap-1.5 rounded border border-scena-700 px-2 py-1 text-xs text-zinc-400 transition-colors hover:bg-scena-800 disabled:opacity-40"
                  >
                    <X className="h-3.5 w-3.5" />
                    {ui.queue.cancelEdit}
                  </button>
                </>
              ) : (
                <button
                  type="button"
                  onClick={startEditing}
                  disabled={disabled}
                  className="flex items-center gap-1.5 rounded border border-scena-700 px-2 py-1 text-xs text-zinc-300 transition-colors hover:bg-scena-800 disabled:opacity-40"
                >
                  <Pencil className="h-3.5 w-3.5" />
                  {ui.queue.edit}
                </button>
              ))}

            {!editing && actions}
          </div>
        </div>
      )}
    </motion.li>
  );
}
