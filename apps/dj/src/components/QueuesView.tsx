import { useCallback, useEffect, useState, type MouseEvent, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ArrowDown, ArrowUp, Ban, Check } from "lucide-react";

import { Panel } from "./Panel";
import { RequestCard } from "./RequestCard";
import { ui } from "../text";
import type { AppSettings, QueuedRequest, RequestStatus } from "../types";

/** Zdarzenie o zmianie kolejki — musi zgadzać się ze stałą `EVENT_REQUESTS_CHANGED` w warstwie Rust. */
const EVENT_REQUESTS_CHANGED = "requests://changed";

/** Limit dedykacji z protokołu — używany tylko do czasu wczytania ustawień DJ-a. */
const FALLBACK_DEDICATION_CHARS = 400;

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

    return () => {
      active = false;
      void unlisten.then((stop) => stop());
    };
  }, [refresh]);

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
                    onSaveDedication={(dedication) =>
                      run(
                        () => invoke("update_dedication", { request_id: request.id, dedication }),
                        ui.queue.editError,
                      )
                    }
                    actions={
                      <>
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
