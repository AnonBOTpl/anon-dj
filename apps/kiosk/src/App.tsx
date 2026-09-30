import { motion } from "framer-motion";
import { Search } from "lucide-react";

import { ui } from "./text";

/**
 * Ekran startowy kiosku. Wyszukiwanie i wysyłanie dedykacji dochodzą w kolejnym etapie,
 * gdy będzie połączenie z aplikacją DJ-a.
 */
export default function App() {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-10 bg-scena-950 px-16">
      <span className="text-sm font-semibold tracking-[0.5em] text-zinc-500">
        {ui.appName}
      </span>

      <motion.section
        initial={{ opacity: 0, y: 12 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.25, ease: "easeOut" }}
        className="w-full max-w-2xl rounded-3xl border border-scena-800 bg-scena-900 p-10 shadow-2xl"
      >
        <h1 className="text-4xl font-semibold text-zinc-100">{ui.heading}</h1>
        <p className="mt-3 text-lg text-zinc-400">{ui.subheading}</p>

        <div className="mt-8 flex gap-4">
          <input
            type="text"
            disabled
            placeholder={ui.searchPlaceholder}
            className="h-16 flex-1 rounded-2xl border border-scena-700 bg-scena-950 px-6 text-xl text-zinc-200 outline-none placeholder:text-zinc-600 disabled:opacity-70"
          />

          <button
            type="button"
            disabled
            className="flex h-16 items-center gap-3 rounded-2xl bg-zinc-700 px-8 text-xl font-semibold text-zinc-300 disabled:cursor-not-allowed"
          >
            <Search className="h-6 w-6" />
            {ui.searchButton}
          </button>
        </div>

        <p className="mt-6 text-sm text-zinc-500">{ui.notConnected}</p>
      </motion.section>
    </div>
  );
}
