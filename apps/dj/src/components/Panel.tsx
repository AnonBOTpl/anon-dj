import type { ReactNode } from "react";
import { motion } from "framer-motion";

type PanelProps = {
  title: string;
  hint: string;
  empty: string;
  children?: ReactNode;
};

/** Panel kolejki (do przeglądu, gotowe, historia). Animacja jest celowo dyskretna. */
export function Panel({ title, hint, empty, children }: PanelProps) {
  return (
    <motion.section
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.18, ease: "easeOut" }}
      className="flex min-h-0 flex-col rounded-lg border border-scena-800 bg-scena-900"
    >
      <header className="border-b border-scena-800 px-3 py-2">
        <h2 className="text-sm font-semibold text-zinc-200">{title}</h2>
        <p className="text-xs text-zinc-500">{hint}</p>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        {children ?? <p className="text-sm text-zinc-500">{empty}</p>}
      </div>
    </motion.section>
  );
}
