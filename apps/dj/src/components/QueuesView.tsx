import { useCallback, useEffect, useState, type MouseEvent, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ArrowDown, ArrowUp, Ban, Check, LoaderCircle, Play } from "lucide-react";

import { Panel } from "./Panel";
import { RequestCard } from "./RequestCard";
import { ui } from "../text";
import type {
  AppSettings,
  ClipProgress,
  PlaybackStatus,
  QueuedRequest,
  RequestStatus,
  VoiceOverState,
} from "../types";

/** Zdarzenie o zmianie kolejki — musi zgadzać się ze stałą `EVENT_REQUESTS_CHANGED` w warstwie Rust. */
const EVENT_REQUESTS_CHANGED = "requests://changed";

/** Zdarzenie z postępem generowania voice-overu — `EVENT_CLIP_PROGRESS` w warstwie Rust. */
const EVENT_CLIP_PROGRESS = "clips://progress";

/** Zdarzenie o odtwarzanym voice-overze — `EVENT_PLAYBACK` w warstwie Rust. */
const EVENT_PLAYBACK = "audio://playback";

/** Limit dedykacji z protokołu — używany tylko do czasu wczytania ustawień DJ-a. */
const FALLBACK_DEDICATION_CHARS = 400;

/**
 * Co ile odświeżamy kolejki, gdy nic się nie dzieje.
 *
 * Zdarzenie `requests://changed` daje natychmiastową reakcję, ale potrafi przepaść — dokładnie
 * tak zastygał chip kiosku, zanim doszło do niego odpytanie. Bez tej siatki DJ widzi nową
 * dedykację dopiero po przełączeniu widoku i powrocie (wtedy widok montuje się od nowa).
 * Odpytanie w tle jest tanie: to trzy zapytania do lokalnej bazy.
 */
const QUEUE_POLL_MS = 5_000;

/** Trzy kolejki DJ-a: do przeglądu, gotowe do wykonania i historia. */
type Queues = {
  review: QueuedRequest[];
  ready: QueuedRequest[];
  history: QueuedRequest[];
};

const actionButton =
  "flex items-center gap-1.5 rounded border border-scena-700 px-2 py-1 text-xs transition-colors hover:bg-scena-800 disabled:opacity-40";

/** Status prośby w historii — pokazujemy go słowem, nie kodem. */
function statusLabel(status: RequestStatus): string {
  return ui.queue.statuses[status];
}

function ActionButton({
  icon,
  label,
  disabled,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  disabled: boolean;
  onClick: (event: MouseEvent<HTMLButtonElement>) => void;
}) {
  return (
    <button type="button" onClick={onClick} disabled={disabled} className={actionButton}>
      {icon}
      {label}
    </button>
  );
}

/**
 * Kolejki DJ-a (PLAN.md, Faza 4): przegląd dedykacji, gotowe do wykonania z własną kolejnością
 * i historia. Nic nie leci na antenę bez decyzji DJ-a — tutaj tylko zatwierdzamy, poprawiamy
 * i odrzucamy.
 */
export function QueuesView() {
  const [queues, setQueues] = useState<Queues>({ review: [], ready: [], history: [] });
  const [maxChars, setMaxChars] = useState(FALLBACK_DEDICATION_CHARS);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [clipStates, setClipStates] = useState<Record<number, ClipProgress>>({});
  const [ttsAvailable, setTtsAvailable] = useState(false);
  // Który voice-over leci teraz. Trzymamy to po zdarzeniach z warstwy Rust, żeby przyciski
  // we wszystkich kolejkach zgadzały się z tym, co naprawdę słychać.
  const [playingRequestId, setPlayingRequestId] = useState<number | null>(null);
  // Która prośba jest właśnie wykonywana. Sekwencja trwa kilka sekund (ściszenie, dedykacja,
  // wyciszenie starego utworu), więc DJ musi widzieć, że coś się dzieje.
  const [executingId, setExecutingId] = useState<number | null>(null);

  const refresh = useCallback(async () => {
    const [review, ready, history] = await Promise.all([
      invoke<QueuedRequest[]>("pending_requests"),
      invoke<QueuedRequest[]>("ready_requests"),
      invoke<QueuedRequest[]>("request_history"),
    ]);

    setQueues({ review, ready, history });
  }, []);

  // Wczytanie kolejek przy wejściu w widok oraz po każdym zgłoszeniu gościa.
  useEffect(() => {
    let active = true;

    void refresh().catch((reason: unknown) => {
      if (active) {
        setError(`${ui.requests.loadError} ${String(reason)}`);
      }
    });

    invoke<AppSettings>("settings_snapshot")
      .then((settings) => {
        if (active) {
          setMaxChars(settings.limits.dedication_max_chars);
        }
      })
      .catch(() => {
        // Limit z protokołu wystarczy — brak ustawień nie może zablokować kolejek.
      });

    const unlisten = listen(EVENT_REQUESTS_CHANGED, () => {
      void refresh().catch((reason: unknown) => {
        setError(`${ui.requests.loadError} ${String(reason)}`);
      });
    });

    // Siatka bezpieczeństwa wobec zdarzenia, które mogło przepaść. Cicha: chwilowy brak danych
    // nie może podmienić tego, co DJ już widzi na ekranie, ani zamrugać komunikatem błędu.
    const timer = window.setInterval(() => {
      void refresh().catch(() => {
        // Nic nie robimy — następny cykl spróbuje znowu.
      });
    }, QUEUE_POLL_MS);

    return () => {
      active = false;
      window.clearInterval(timer);
      void unlisten.then((stop) => stop());
    };
  }, [refresh]);

  // Stan voice-overów: raz na wejściu — dzięki temu po przeładowaniu okna widać generowanie,
  // które już trwa — a potem zdarzeniami z warstwy Rust (status i pasek postępu).
  useEffect(() => {
    let active = true;

    invoke<ClipProgress[]>("clip_states")
      .then((states) => {
        if (!active) {
          return;
        }

        const known: Record<number, ClipProgress> = {};

        for (const state of states) {
          known[state.request_id] = state;
        }

        setClipStates(known);
      })
      .catch(() => {
        // Brak stanów to nie błąd — karty pokażą „Przygotuj voice-over”.
      });

    invoke<{ available: boolean }>("tts_status")
      .then((status) => {
        if (active) {
          setTtsAvailable(status.available);
        }
      })
      .catch(() => {
        // Bez informacji o lektorze nie pokazujemy przycisku, żeby nie obiecywać dźwięku.
      });

    const unlisten = listen<ClipProgress>(EVENT_CLIP_PROGRESS, (event) => {
      const progress = event.payload;

      setClipStates((current) => ({ ...current, [progress.request_id]: progress }));
    });

    return () => {
      active = false;
      void unlisten.then((stop) => stop());
    };
  }, []);

  // Odsłuch voice-overu. Zdarzenie przychodzi też wtedy, gdy klip sam się skończył — wtedy
  // przycisk wraca do stanu „Odsłuchaj” bez pytania DJ-a o cokolwiek.
  useEffect(() => {
    const unlisten = listen<PlaybackStatus>(EVENT_PLAYBACK, (event) => {
      setPlayingRequestId(event.payload.request_id);
    });

    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  /**
   * Odsłuch dedykacji. Nie odświeżamy przy tym kolejek — odtwarzanie niczego w nich nie zmienia,
   * więc nie ma po co przerysowywać całego ekranu w trakcie słuchania.
   */
  async function togglePlayback(request: QueuedRequest) {
    setError(null);

    const playing = playingRequestId === request.id;

    try {
      if (playing) {
        await invoke("stop_dedication");
      } else {
        await invoke("play_dedication", { request_id: request.id });
      }
    } catch (reason: unknown) {
      const prefix = playing
        ? ui.queue.voiceOver.stopError
        : ui.queue.voiceOver.playError;

      setError(`${prefix} ${String(reason)}`);
    }
  }

  /**
   * Stan voice-overu karty. Bieżący przebieg generowania wygrywa ze ścieżką klipu, bo po poprawce
   * tekstu klip z bazy jest już nieaktualny, a nowy jeszcze się liczy.
   */
  function voiceOverFor(request: QueuedRequest): VoiceOverState {
    const clip = clipStates[request.id];

    if (clip !== undefined) {
      if (clip.status === "generating") {
        return { status: "generating", progress: clip.progress };
      }

      if (clip.status === "failed") {
        return { status: "failed", message: clip.message };
      }

      if (clip.status === "ready") {
        return { status: "ready" };
      }
    }

    return request.tts_clip_path === null ? { status: "missing" } : { status: "ready" };
  }

  /**
   * Jedna droga dla wszystkich decyzji DJ-a: wywołanie komendy, komunikat błędu i odświeżenie.
   * Zwraca `true`, gdy operacja się udała — karta edycji wie dzięki temu, czy zamknąć pole tekstu.
   */
  async function run(action: () => Promise<unknown>, failure: string): Promise<boolean> {
    setError(null);
    setBusy(true);

    try {
      await action();
      await refresh();

      return true;
    } catch (reason: unknown) {
      setError(`${failure} ${String(reason)}`);

      return false;
    } finally {
      setBusy(false);
    }
  }

  /**
   * Wykonanie prośby: sekwencja po stronie Rusta ścisza muzykę, czyta dedykację i wpuszcza
   * zamówiony utwór. Nie idziemy tu przez `run`, bo tamto ustawia `busy` i blokuje cały ekran —
   * a DJ ma w tym czasie widzieć, które wykonanie trwa.
   */
  async function handleExecute(requestId: number) {
    setError(null);
    setExecutingId(requestId);

    try {
      await invoke("execute_request", { request_id: requestId });
      await refresh();
    } catch (reason: unknown) {
      setError(`${ui.queue.executeError} ${String(reason)}`);
    } finally {
      setExecutingId(null);
    }
  }

  const reviewEmpty = queues.review.length === 0;
  const readyEmpty = queues.ready.length === 0;
  const historyEmpty = queues.history.length === 0;

  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      {error !== null && <p className="shrink-0 text-xs text-red-400">{error}</p>}

      <div className="grid min-h-0 flex-1 grid-cols-[1.5fr_1fr] gap-3">
        <Panel title={ui.queue.reviewTitle} hint={ui.queue.reviewHint} empty={ui.queue.reviewEmpty}>
          {reviewEmpty ? undefined : (
            <ul className="space-y-2">
              {queues.review.map((request) => (
                <RequestCard
                  key={request.id}
                  request={request}
                  maxChars={maxChars}
                  disabled={busy}
                  voiceOver={voiceOverFor(request)}
                  ttsAvailable={ttsAvailable}
                  playing={playingRequestId === request.id}
                  onTogglePlayback={() => void togglePlayback(request)}
                  onGenerateVoiceOver={() =>
                    void run(
                      () => invoke("generate_clip", { request_id: request.id }),
                      ui.queue.voiceOver.generateError,
                    )
                  }
                  onSaveDedication={(dedication) =>
                    run(
                      () => invoke("update_dedication", { request_id: request.id, dedication }),
                      ui.queue.editError,
                    )
                  }
                  actions={
                    <>
                      <ActionButton
                        icon={<Check className="h-3.5 w-3.5" />}
                        label={ui.queue.approve}
                        disabled={busy}
                        onClick={(event) => {
                          event.stopPropagation();
                          void run(
                            () => invoke("approve_request", { request_id: request.id }),
                            ui.queue.approveError,
                          );
                        }}
                      />
                      <ActionButton
                        icon={<Ban className="h-3.5 w-3.5" />}
                        label={ui.queue.reject}
                        disabled={busy}
                        onClick={(event) => {
                          event.stopPropagation();
                          void run(
                            () => invoke("reject_request", { request_id: request.id }),
                            ui.queue.rejectError,
                          );
                        }}
                      />
                    </>
                  }
                />
              ))}
            </ul>
          )}
        </Panel>

        <div className="grid min-h-0 grid-rows-2 gap-3">
          <Panel title={ui.queue.readyTitle} hint={ui.queue.readyHint} empty={ui.queue.readyEmpty}>
            {readyEmpty ? undefined : (
              <ul className="space-y-2">
                {queues.ready.map((request, index) => (
                  <RequestCard
                    key={request.id}
                    request={request}
                    maxChars={maxChars}
                    disabled={busy}
                    voiceOver={voiceOverFor(request)}
                    ttsAvailable={ttsAvailable}
                    playing={playingRequestId === request.id}
                    onTogglePlayback={() => void togglePlayback(request)}
                    onGenerateVoiceOver={() =>
                      void run(
                        () => invoke("generate_clip", { request_id: request.id }),
                        ui.queue.voiceOver.generateError,
                      )
                    }
                    onSaveDedication={(dedication) =>
                      run(
                        () => invoke("update_dedication", { request_id: request.id, dedication }),
                        ui.queue.editError,
                      )
                    }
                    actions={
                      <>
                        <ActionButton
                          icon={
                            executingId === request.id ? (
                              <LoaderCircle className="h-3.5 w-3.5 animate-spin" />
                            ) : (
                              <Play className="h-3.5 w-3.5" />
                            )
                          }
                          label={executingId === request.id ? ui.queue.executing : ui.queue.execute}
                          // Bez gotowego voice-overu nie ma czego czytać, a bez odtwarzacza nie ma
                          // gdzie zagrać — w obu przypadkach przycisk musi być nieaktywny.
                          disabled={
                            busy ||
                            executingId !== null ||
                            voiceOverFor(request).status !== "ready"
                          }
                          onClick={(event) => {
                            event.stopPropagation();
                            void handleExecute(request.id);
                          }}
                        />
                        <ActionButton
                          icon={<ArrowUp className="h-3.5 w-3.5" />}
                          label={ui.queue.moveUp}
                          disabled={busy || index === 0}
                          onClick={(event) => {
                            event.stopPropagation();
                            void run(
                              () =>
                                invoke("move_request", {
                                  request_id: request.id,
                                  direction: "up",
                                }),
                              ui.queue.moveError,
                            );
                          }}
                        />
                        <ActionButton
                          icon={<ArrowDown className="h-3.5 w-3.5" />}
                          label={ui.queue.moveDown}
                          disabled={busy || index === queues.ready.length - 1}
                          onClick={(event) => {
                            event.stopPropagation();
                            void run(
                              () =>
                                invoke("move_request", {
                                  request_id: request.id,
                                  direction: "down",
                                }),
                              ui.queue.moveError,
                            );
                          }}
                        />
                        <ActionButton
                          icon={<Ban className="h-3.5 w-3.5" />}
                          label={ui.queue.reject}
                          disabled={busy}
                          onClick={(event) => {
                            event.stopPropagation();
                            void run(
                              () => invoke("reject_request", { request_id: request.id }),
                              ui.queue.rejectError,
                            );
                          }}
                        />
                      </>
                    }
                  />
                ))}
              </ul>
            )}
          </Panel>

          <Panel
            title={ui.queue.historyTitle}
            hint={ui.queue.historyHint}
            empty={ui.queue.historyEmpty}
          >
            {historyEmpty ? undefined : (
              <ul className="space-y-2">
                {queues.history.map((request) => (
                  <RequestCard
                    key={request.id}
                    request={request}
                    maxChars={maxChars}
                    footnote={statusLabel(request.status)}
                  />
                ))}
              </ul>
            )}
          </Panel>
        </div>
      </div>
    </div>
  );
}
