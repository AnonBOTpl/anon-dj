import { useEffect, useRef, useState, type ReactNode } from "react";
import { AnimatePresence, motion } from "framer-motion";

/** Ile trzeba przytrzymać kursor, żeby tooltip się pojawił. */
const HOVER_DELAY_MS = 450;

/** Szerokość tooltipa — stała, żeby dało się go wcisnąć w ekran bez mierzenia zawartości. */
const TOOLTIP_WIDTH_PX = 420;

/** Odstęp tooltipa od krawędzi ekranu i od tekstu. */
const TOOLTIP_MARGIN_PX = 12;

/**
 * Tekst, który przycina się do szerokości rodzica i pokazuje w całości po najechaniu.
 *
 * W kiosku teksty są duże, więc długi tytuł łatwo wychodzi za ekran. Tooltip pojawia się
 * **tylko wtedy, gdy tekst naprawdę się nie mieści** — dla krótkich tytułów byłby zbędnym
 * mrugnięciem. Pozycjonujemy go względem okna (`position: fixed`), bo lista przewija się
 * w kontenerze z `overflow`, który uciąłby tooltip w środku wiersza.
 */
export function TruncatedText({
  text,
  className,
}: {
  /** Pełny tekst — ten sam, który trafia do ciała komponentu. */
  text: string;
  /** Klasy przycinające (`truncate`, `min-w-0`, szerokości) — dobiera je miejsce użycia. */
  className?: string;
}): ReactNode {
  const ref = useRef<HTMLSpanElement>(null);
  const timer = useRef<number | null>(null);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);

  // Sprzątanie timera przy odmontowaniu — bez tego zdarzenie znikniętego wiersza ustawiłoby stan.
  useEffect(() => {
    return () => {
      if (timer.current !== null) {
        window.clearTimeout(timer.current);
      }
    };
  }, []);

  function handleEnter() {
    timer.current = window.setTimeout(() => {
      const node = ref.current;

      // Tekst mieści się w całości — nie ma czego pokazywać.
      if (node === null || node.scrollWidth <= node.clientWidth) {
        return;
      }

      const rect = node.getBoundingClientRect();
      const maxLeft = window.innerWidth - TOOLTIP_WIDTH_PX - TOOLTIP_MARGIN_PX;

      setPosition({
        left: Math.max(TOOLTIP_MARGIN_PX, Math.min(rect.left, maxLeft)),
        top: rect.bottom + TOOLTIP_MARGIN_PX,
      });
    }, HOVER_DELAY_MS);
  }

  function handleLeave() {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }

    setPosition(null);
  }

  return (
    <>
      <span ref={ref} className={className} onMouseEnter={handleEnter} onMouseLeave={handleLeave}>
        {text}
      </span>

      <AnimatePresence>
        {position !== null && (
          <motion.span
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.12 }}
            style={{
              position: "fixed",
              left: position.left,
              top: position.top,
              width: TOOLTIP_WIDTH_PX,
            }}
            className="pointer-events-none z-50 rounded-xl border border-scena-700 bg-scena-800 px-4 py-2 text-base text-zinc-100 shadow-lg"
          >
            {text}
          </motion.span>
        )}
      </AnimatePresence>
    </>
  );
}
