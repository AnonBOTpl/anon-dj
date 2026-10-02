import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { motion } from "framer-motion";
import { ClipboardCopy, TriangleAlert, X } from "lucide-react";

import { ui } from "../text";
import type { CrashReport } from "../types";

/** Pole z treścią raportu: czytamy je i kopiujemy, ale nie zmieniamy — to zapis awarii, nie formularz. */
const reportField =
  "h-72 w-full resize-none rounded border border-scena-700 bg-scena-950 p-3 font-mono text-xs leading-relaxed text-zinc-200 outline-none";

const primaryButton =
  "flex items-center gap-2 rounded bg-zinc-100 px-4 py-2 text-sm font-semibold text-scena-950 transition-colors hover:bg-white";
const secondaryButton =
  "rounded border border-scena-700 px-4 py-2 text-sm text-zinc-300 transition-colors hover:bg-scena-800";

/**
 * Raport awarii z poprzedniego uruchomienia.
 *
 * Aplikacja startuje z `panic = "abort"` i bez okna konsoli, więc awaria nie zostawia śladu,
 * którego DJ mógłby nie zauważyć. Raport czeka na dysku, a tutaj DJ go widzi i może kopiować —
 * dlatego to okno nie zamyka się samo i nie chowa treści za skrótem.
 */
export function CrashReportDialog({
  report,
  onClose,
}: {
  report: CrashReport;
  onClose: () => void;
}) {
  const field = useRef<HTMLTextAreaElement>(null);
  const [copied, setCopied] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  async function copy() {
    try {
      await navigator.clipboard.writeText(report.text);

      setProblem(null);
      setCopied(true);
    } catch {
      // Schowek potrafi być zablokowany. Wtedy zaznaczamy tekst, żeby wystarczyło Ctrl+C —
      // DJ ma dostać ten raport, a nie komunikat, że się nie udało.
      field.current?.focus();
      field.current?.select();

      setCopied(false);
      setProblem(ui.crash.copyFallback);
    }
  }

  async function close() {
    try {
      await invoke("dismiss_crash_report");
    } catch {
      // Raport zostanie na dysku i pokaże się znowu — to nie powód, żeby nie zamknąć okna.
    }

    onClose();
  }

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      transition={{ duration: 0.12 }}
      className="fixed inset-0 z-50 flex items-center justify-center bg-scena-950/80 p-6"
    >
      <motion.div
        initial={{ opacity: 0, scale: 0.98 }}
        animate={{ opacity: 1, scale: 1 }}
        transition={{ duration: 0.12 }}
        className="flex max-h-full w-full max-w-3xl flex-col rounded-lg border border-scena-800 bg-scena-900"
      >
        <header className="flex items-start gap-3 border-b border-scena-800 px-4 py-3">
          <TriangleAlert className="mt-0.5 h-5 w-5 shrink-0 text-red-400" />

          <div className="min-w-0">
            <h2 className="text-sm font-semibold text-zinc-100">{ui.crash.title}</h2>
            <p className="mt-0.5 text-xs text-zinc-400">{ui.crash.hint}</p>
          </div>

          <button
            type="button"
            onClick={() => void close()}
            title={ui.crash.close}
            aria-label={ui.crash.close}
            className="ml-auto flex h-7 w-7 shrink-0 items-center justify-center rounded text-zinc-400 transition-colors hover:bg-scena-700 hover:text-zinc-100"
          >
            <X className="h-4 w-4" />
          </button>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
          <label className="block text-xs text-zinc-400" htmlFor="crash-report-text">
            {ui.crash.reportLabel}
          </label>

          <textarea
            id="crash-report-text"
            ref={field}
            readOnly
            value={report.text}
            spellCheck={false}
            className={`${reportField} mt-1`}
          />

          <p className="mt-2 truncate text-[11px] text-zinc-500" title={report.path}>
            {ui.crash.fileLabel}: {report.path}
          </p>

          {problem !== null && <p className="mt-1 text-xs text-amber-300">{problem}</p>}
        </div>

        <footer className="flex items-center justify-end gap-2 border-t border-scena-800 px-4 py-3">
          <button type="button" onClick={() => void close()} className={secondaryButton}>
            {ui.crash.close}
          </button>

          <button type="button" onClick={() => void copy()} className={primaryButton}>
            <ClipboardCopy className="h-4 w-4" />
            {copied ? ui.crash.copied : ui.crash.copy}
          </button>
        </footer>
      </motion.div>
    </motion.div>
  );
}
