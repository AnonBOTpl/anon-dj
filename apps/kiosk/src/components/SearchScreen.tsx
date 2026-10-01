import { motion } from "framer-motion";
import { LoaderCircle, Music4, Search } from "lucide-react";

import { TruncatedText } from "./TruncatedText";
import { ui } from "../text";
import type { TrackInfo } from "../types";

type SearchScreenProps = {
  query: string;
  results: TrackInfo[];
  searching: boolean;
  hasSearched: boolean;
  message: string | null;
  onQueryChange: (value: string) => void;
  onSearch: () => void;
  /** Enter: wybiera pierwszą podpowiedź, a gdy jej nie ma — szuka jak zwykłe wyszukiwanie. */
  onEnter: () => void;
  onSelect: (track: TrackInfo) => void;
  /** Limit frazy ustawiony przez DJ-a — pole nie pozwala wpisać więcej. */
  maxQueryChars: number;
};

/** Ekran wyszukiwania: duże pole, duże przyciski, cała lista jednym kliknięciem. */
export function SearchScreen({
  query,
  results,
  searching,
  hasSearched,
  message,
  onQueryChange,
  onSearch,
  onEnter,
  onSelect,
  maxQueryChars,
}: SearchScreenProps) {
  return (
    <div className="flex h-full flex-col items-center gap-8 px-16 py-12">
      <header className="text-center">
        <h1 className="text-4xl font-semibold text-zinc-100">{ui.search.heading}</h1>
        <p className="mt-2 text-lg text-zinc-400">{ui.search.subheading}</p>
      </header>

      <form
        className="flex w-full max-w-3xl gap-4"
        onSubmit={(event) => {
          // Enter = pierwsza podpowiedź. Przycisk „Szukaj” zostaje na pełne wyszukiwanie,
          // więc musi być osobnego typu — inaczej klikałby to samo co Enter.
          event.preventDefault();
          onEnter();
        }}
      >
        <input
          type="text"
          autoFocus
          value={query}
          maxLength={maxQueryChars}
          placeholder={ui.search.placeholder}
          onChange={(event) => onQueryChange(event.currentTarget.value)}
          className="h-16 flex-1 rounded-2xl border border-scena-700 bg-scena-950 px-6 text-xl text-zinc-200 outline-none placeholder:text-zinc-600 focus:border-zinc-500"
        />

        <button
          type="button"
          onClick={onSearch}
          disabled={searching || query.trim() === ""}
          className="flex h-16 items-center gap-3 rounded-2xl bg-zinc-100 px-8 text-xl font-semibold text-scena-950 transition-colors hover:bg-white disabled:opacity-50"
        >
          {searching ? (
            <LoaderCircle className="h-6 w-6 animate-spin" />
          ) : (
            <Search className="h-6 w-6" />
          )}
          {searching ? ui.search.searching : ui.search.button}
        </button>
      </form>

      {message !== null && <p className="text-lg text-red-400">{message}</p>}

      <div className="w-full max-w-3xl flex-1 overflow-x-hidden overflow-y-auto">
        {results.length === 0 ? (
          <p className="text-center text-lg text-zinc-500">
            {hasSearched ? ui.search.empty : ""}
          </p>
        ) : (
          <ul className="space-y-3 pb-4">
            {results.map((track) => (
              <motion.li
                key={track.id}
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ duration: 0.15, ease: "easeOut" }}
              >
                <button
                  type="button"
                  onClick={() => onSelect(track)}
                  className="flex w-full items-center gap-4 rounded-2xl border border-scena-800 bg-scena-900 px-6 py-4 text-left transition-colors hover:border-zinc-500 hover:bg-scena-800"
                >
                  <Music4 className="h-6 w-6 shrink-0 text-zinc-500" />

                  <span className="min-w-0 flex-1">
                    <TruncatedText
                      text={track.title}
                      className="block truncate text-xl text-zinc-100"
                    />
                    <TruncatedText
                      text={track.artist === "" ? ui.search.noArtist : track.artist}
                      className="block truncate text-base text-zinc-400"
                    />
                  </span>
                </button>
              </motion.li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
