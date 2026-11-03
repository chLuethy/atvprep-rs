import { createSignal } from "solid-js";
import type { ManagerEvent } from "./events";

type LogTail = {
  type: "log_tail";
  id: string;
  sequence: number;
  item_id: string | null;
  offset: number;
  text: string;
};

type Snapshot = {
  data?: Array<Record<string, unknown>>;
  meta?: { current_cursor?: string | null; next_cursor?: string | null; current_sequence?: number };
};

const [events, setEvents] = createSignal<ManagerEvent[]>([]);
const [logTails, setLogTails] = createSignal<LogTail[]>([]);
const [snapshot, setSnapshot] = createSignal<Snapshot | null>(null);
let highestSequence = -1;

export const realtimeState = {
  events,
  logTails,
  snapshot,
  applyEvent(event: ManagerEvent): void {
    if (!Number.isSafeInteger(event.sequence) || event.sequence <= 0 || event.sequence <= highestSequence) return;
    highestSequence = event.sequence;
    setEvents((current) => [...current, event].slice(-1000));
  },
  applyLogTail(tail: LogTail): void {
    if (!Number.isSafeInteger(tail.sequence) || tail.sequence <= 0 || tail.sequence <= highestSequence) return;
    highestSequence = tail.sequence;
    setLogTails((current) => [...current, tail].slice(-500));
  },
  applySnapshot(value: unknown): void {
    const next = validateSnapshot(value);
    if (!next) return;
    setSnapshot(next);
    const eventRows = (next.data ?? []).filter((row) => row.topic !== "job.log");
    const tails = (next.data ?? []).filter((row) => row.topic === "job.log").map((row) => {
      const payload = row.payload as Record<string, unknown> | undefined;
      return {
        type: "log_tail" as const,
        id: String(row.id ?? ""),
        sequence: typeof row.sequence === "number" ? row.sequence : -1,
        item_id: typeof row.item_id === "string" ? row.item_id : null,
        offset: typeof payload?.offset === "number" ? payload.offset : 0,
        text: String(payload?.text ?? payload?.log_tail ?? "").slice(-8192),
      };
    });
    const nextEvents = eventRows.map((row) => ({
        type: "event" as const,
        id: String(row.id ?? ""),
        sequence: typeof row.sequence === "number" ? row.sequence : -1,
        topic: String(row.topic ?? ""),
        payload: (row.payload as Record<string, unknown> | undefined) ?? {},
      }));
    const unique = [...nextEvents, ...tails]
      .filter((entry) => Number.isSafeInteger(entry.sequence) && entry.sequence > 0 && entry.sequence > highestSequence)
      .sort((left, right) => left.sequence - right.sequence)
      .filter((entry, index, entries) => index === 0 || entry.sequence !== entries[index - 1]!.sequence);
    unique.forEach((entry) => { highestSequence = Math.max(highestSequence, entry.sequence); });
    const boundarySequence = next.meta?.current_sequence;
    if (typeof boundarySequence === "number" && boundarySequence > 0 && isBoundedString(next.meta?.current_cursor)
      && boundarySequence > highestSequence) {
      highestSequence = boundarySequence;
    }
    const uniqueEvents = unique.filter((entry): entry is (typeof nextEvents)[number] => entry.type === "event");
    const uniqueTails = unique.filter((entry): entry is (typeof tails)[number] => entry.type === "log_tail");
    setEvents((current) => [...current, ...uniqueEvents].slice(-1000));
    setLogTails((current) => [...current, ...uniqueTails].slice(-500));
  },
};

const MAX_STRING_LENGTH = 4096;

function isBoundedString(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= MAX_STRING_LENGTH;
}

function isCursor(value: unknown): value is string | null {
  return value === null || isBoundedString(value);
}

function isPayload(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function validateSnapshot(value: unknown): Snapshot | null {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return null;
  const candidate = value as { data?: unknown; meta?: unknown };
  if (!Array.isArray(candidate.data) || candidate.meta === null || typeof candidate.meta !== "object" || Array.isArray(candidate.meta)) return null;
  const meta = candidate.meta as Record<string, unknown>;
  if (!isCursor(meta.current_cursor) || !isCursor(meta.next_cursor)) return null;
  if (meta.current_sequence !== undefined
    && (typeof meta.current_sequence !== "number" || !Number.isSafeInteger(meta.current_sequence) || meta.current_sequence < 0)) return null;
  if (meta.current_sequence === 0 && (candidate.data.length !== 0 || meta.current_cursor !== null)) return null;
  const currentSequence = meta.current_sequence;
  if (typeof currentSequence === "number" && currentSequence > 0) {
    if (candidate.data.some((row) => {
      if (row === null || typeof row !== "object" || Array.isArray(row)) return false;
      const item = row as Record<string, unknown>;
      return typeof item.sequence === "number" && item.sequence > currentSequence;
    })) return null;
    if (isBoundedString(meta.current_cursor)) {
      const boundaryRow = candidate.data.find((row) => row !== null && typeof row === "object" && !Array.isArray(row)
        && (row as Record<string, unknown>).id === meta.current_cursor) as Record<string, unknown> | undefined;
      if (boundaryRow && boundaryRow.sequence !== currentSequence) return null;
    }
  }
  const data: Array<Record<string, unknown>> = [];
  for (const row of candidate.data) {
    if (row === null || typeof row !== "object" || Array.isArray(row)) return null;
    const item = row as Record<string, unknown>;
    if (!isBoundedString(item.id) || typeof item.sequence !== "number" || !Number.isSafeInteger(item.sequence) || item.sequence <= 0
      || !isBoundedString(item.topic) || !isPayload(item.payload)) return null;
    data.push(item);
  }
  return { data, meta: {
    current_cursor: meta.current_cursor,
    next_cursor: meta.next_cursor,
    ...(meta.current_sequence === undefined ? {} : { current_sequence: meta.current_sequence }),
  } };
}