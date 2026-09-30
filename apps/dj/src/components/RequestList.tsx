import { motion } from "framer-motion";
import { User } from "lucide-react";

/** Prośba gościa czekająca na decyzję DJ-a — zgadza się z `QueuedRequest` w warstwie Rust. */
export type QueuedRequest = {
  id: number;
  track_id: number;
  title: string;
  artist: string;
  dedication: string;
  guest_name: string | null;
  status: string;
  created_at: number;
};

/** Godzina zgłoszenia — DJ patrzy na kolejkę chronologicznie. */
function formatTime(createdAt: number): string {
  return new Date(createdAt).toLocaleTimeString("pl-PL", {
    hour: "2-digit",
    minute: "2-digit",
  });
}

/**
 * Lista prośb. Dedykacja jest największym tekstem na ekranie — DJ czyta ją w hałasie
 * i pod presją czasu (AGENTS.md, konwencje frontendu).
 */
export function RequestList({ requests }: { requests: QueuedRequest[] }) {
  return (
    <ul className="space-y-2">
      {requests.map((request) => (
        <motion.li
          key={request.id}
          initial={{ opacity: 0, y: 4 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.15, ease: "easeOut" }}
          className="rounded border border-scena-800 bg-scena-950 p-3"
        >
          <p className="text-base leading-snug text-zinc-100">{request.dedication}</p>

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
        </motion.li>
      ))}
    </ul>
  );
}
