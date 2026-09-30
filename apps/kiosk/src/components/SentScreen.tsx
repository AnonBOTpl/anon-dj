import { motion } from "framer-motion";
import { CircleCheck, Clock } from "lucide-react";

import { ui } from "../text";
import type { RequestStatus, TrackInfo } from "../types";

type SentScreenProps = {
  track: TrackInfo;
  status: RequestStatus;
  /** Ile sekund zostało do samoczynnego powrotu na ekran wyszukiwania. */
  secondsLeft: number;
  onAgain: () => void;
};

/**
 * Ekran po wysłaniu: gość widzi, że zgłoszenie dotarło, i ma czym zacząć od nowa.
 * Kiosk sam wraca do wyszukiwania po `secondsLeft`, żeby kolejny gość mógł od razu pisać.
 */
export function SentScreen({ track, status, secondsLeft, onAgain }: SentScreenProps) {
  const positive = status !== "rejected";

  return (
    <div className="flex h-full items-center justify-center px-16">
      <motion.section
        initial={{ opacity: 0, y: 12 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.25, ease: "easeOut" }}
        className="w-full max-w-2xl rounded-3xl border border-scena-800 bg-scena-900 p-10 text-center shadow-2xl"
      >
        {positive ? (
          <CircleCheck className="mx-auto h-14 w-14 text-emerald-400" />
        ) : (
          <Clock className="mx-auto h-14 w-14 text-amber-400" />
        )}

        <h1 className="mt-6 text-3xl font-semibold text-zinc-100">{ui.sent.heading}</h1>
        <p className="mt-2 text-lg text-zinc-400">{ui.sent.subheading}</p>

        <p className="mt-6 truncate text-xl text-zinc-100">{track.title}</p>
        <p className="mt-1 text-lg text-zinc-400">{ui.status[status]}</p>
        <p className="mt-4 text-base text-zinc-500">{ui.sent.countdown(secondsLeft)}</p>

        <button
          type="button"
          onClick={onAgain}
          className="mt-10 h-14 rounded-2xl bg-zinc-100 px-8 text-lg font-semibold text-scena-950 transition-colors hover:bg-white"
        >
          {ui.sent.again}
        </button>
      </motion.section>
    </div>
  );
}
