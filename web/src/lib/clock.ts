/**
 * A clock anchored to the server's.
 *
 * The whole product is deadlines, and phones have wrong clocks. Every API
 * response carries `server_time`; we record the offset from local time and run
 * countdowns against the corrected value. The client still never *decides* that
 * a window closed — it asks the server — but what it displays is right.
 */

import { useEffect, useRef, useState } from "react";

let offsetMs = 0;
let anchored = false;

export function anchor(serverTime: string) {
  const server = Date.parse(serverTime);
  if (!Number.isNaN(server)) {
    offsetMs = server - Date.now();
    anchored = true;
  }
}

export function serverNow(): number {
  return Date.now() + offsetMs;
}

export function isAnchored(): boolean {
  return anchored;
}

export interface Remaining {
  totalMs: number;
  expired: boolean;
  label: string;
}

export function remainingUntil(iso: string | null): Remaining | null {
  if (!iso) return null;
  const due = Date.parse(iso);
  if (Number.isNaN(due)) return null;

  const totalMs = due - serverNow();
  const expired = totalMs <= 0;
  const s = Math.max(0, Math.floor(totalMs / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;

  const label = expired
    ? "any moment now"
    : h > 0
      ? `${h}h ${String(m).padStart(2, "0")}m`
      : m > 0
        ? `${m}m ${String(sec).padStart(2, "0")}s`
        : `${sec}s`;

  return { totalMs, expired, label };
}

/** Re-renders once a second while a deadline is pending. */
export function useCountdown(iso: string | null): Remaining | null {
  const [, tick] = useState(0);
  const raf = useRef<number | undefined>(undefined);

  useEffect(() => {
    if (!iso) return;
    const id = window.setInterval(() => tick((n) => n + 1), 1000);
    return () => {
      window.clearInterval(id);
      if (raf.current) cancelAnimationFrame(raf.current);
    };
  }, [iso]);

  return remainingUntil(iso);
}

/** Polls a loader on an interval, pausing while the tab is hidden. */
export function usePoll(fn: () => void, intervalMs: number, active = true) {
  const saved = useRef(fn);
  saved.current = fn;

  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => {
      if (document.visibilityState === "visible") saved.current();
    }, intervalMs);
    const onVisible = () => {
      if (document.visibilityState === "visible") saved.current();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      window.clearInterval(id);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [intervalMs, active]);
}
