import type { ManagerApi } from "./api";

export type ManagerEvent = {
  type: "event";
  id: string;
  sequence: number;
  topic: string;
  payload: Record<string, unknown>;
};

type EventMessage =
  | ManagerEvent
  | { type: "hello"; version: number; current_event_id: string | null; oldest_event_id: string | null }
  | { type: "gap"; after: string | null; reason: string }
  | { type: "overflow"; after?: string | null; reason: string }
  | { type: "log_tail"; id: string; sequence: number; item_id: string | null; offset: number; text: string }
  | { type: "ping" }
  | { type: "pong" }
  | { type: "error"; reason: string };

type EventClientOptions = {
  api: Pick<ManagerApi, "request">;
  url?: string;
  onEvent?: (event: ManagerEvent) => void;
  onLogTail?: (tail: Extract<EventMessage, { type: "log_tail" }>) => void;
  onResync?: (snapshot: unknown) => void;
};

type Socket = WebSocket;
type SocketConstructor = new (url: string) => Socket;

type Snapshot = {
  data: Array<Record<string, unknown>>;
  meta: { current_cursor: string | null; next_cursor: string | null; current_sequence?: number };
};

export class EventClient {
  private readonly api: Pick<ManagerApi, "request">;
  private readonly url: string;
  private readonly onEvent?: (event: ManagerEvent) => void;
  private readonly onLogTail?: (tail: Extract<EventMessage, { type: "log_tail" }>) => void;
  private readonly onResync?: (snapshot: unknown) => void;
  private socket: Socket | null = null;
  private cursor: string | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private stopped = false;
  private reconnectDelay = 250;
  private resyncGeneration = 0;
  private resyncing = false;
  private pendingDuringResync: EventMessage[] = [];
  private pendingOverflowed = false;
  private resyncAfter: string | null | undefined;
  private pendingResyncAfter: string | null | undefined;
  private preservePendingOnReconnect = false;
  private highestSequence = -1;
  private pendingResyncBytes = 0;
  private resyncAbortController: AbortController | null = null;
  private static readonly maxRawFrameBytes = 64 * 1024;
  private static readonly maxProtocolStringLength = 4096;
  private static readonly maxPendingDuringResyncBytes = 256 * 1024;
  private static readonly maxPendingDuringResync = 1000;
  private static readonly maxResyncAttempts = 1;
  private static readonly maxResyncResponseBytes = 64 * 1024;
  private static readonly maxResyncRows = 1000;
  private static readonly maxResyncPages = 100;
  private static readonly resyncRequestTimeoutMs = 10_000;

  constructor(options: EventClientOptions) {
    this.api = options.api;
    this.url = options.url ?? "/api/ws";
    this.onEvent = options.onEvent;
    this.onLogTail = options.onLogTail;
    this.onResync = options.onResync;
  }

  get lastCursor(): string | null {
    return this.cursor;
  }

  connect(preserveResync = false): void {
    this.stopped = false;
    if (!preserveResync) {
      this.resyncAbortController?.abort();
      this.resyncAbortController = null;
      this.resyncGeneration += 1;
      this.resyncing = false;
    }
    if (!preserveResync && !this.preservePendingOnReconnect) {
      this.pendingDuringResync = [];
      this.pendingOverflowed = false;
      this.resyncAfter = undefined;
      this.pendingResyncAfter = undefined;
    }
    this.preservePendingOnReconnect = false;
    if (this.reconnectTimer !== null) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    const previous = this.socket;
    this.socket = null;
    if (previous) {
      previous.onmessage = null;
      previous.onopen = null;
      previous.onclose = null;
      previous.onerror = null;
      previous.close();
    }
    const WebSocketImpl = globalThis.WebSocket as unknown as SocketConstructor;
    const url = new URL(this.url, globalThis.location?.href ?? "http://localhost/");
    if (url.protocol === "http:") url.protocol = "ws:";
    if (url.protocol === "https:") url.protocol = "wss:";
    if (this.cursor) url.searchParams.set("after", this.cursor);
    const socket = new WebSocketImpl(url.toString());
    this.socket = socket;
    socket.onmessage = (message) => {
      if (this.socket === socket) void this.handle(message.data, socket);
    };
    socket.onopen = () => {
      if (this.socket === socket) {
        this.reconnectDelay = 250;
        if (!this.resyncing && (this.pendingDuringResync.length > 0 || this.pendingOverflowed || this.resyncAfter !== undefined)) void this.resync();
      }
    };
    socket.onclose = () => {
      if (this.socket !== socket) return;
      this.socket = null;
      if (!this.stopped && this.reconnectTimer === null) {
        this.reconnectTimer = setTimeout(() => {
          this.reconnectTimer = null;
          this.reconnectDelay = Math.min(this.reconnectDelay * 2, 5000);
          this.connect(true);
        }, this.reconnectDelay);
      }
    };
    socket.onerror = () => {
      if (this.socket === socket) socket.close();
    };
  }

  close(): void {
    this.stopped = true;
    this.resyncAbortController?.abort();
    this.resyncAbortController = null;
    if (this.reconnectTimer !== null) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    this.socket?.close();
    this.socket = null;
  }

  private async handle(raw: unknown, socket: Socket): Promise<void> {
    if (this.byteLength(raw) > EventClient.maxRawFrameBytes) {
      if (this.socket === socket) socket.close();
      return;
    }
    let message: EventMessage;
    try {
      const parsed: unknown = JSON.parse(String(raw));
      if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) return;
      const type = (parsed as { type?: unknown }).type;
      if (this.isControlFrame(parsed) || this.isSequencedFrame(parsed) || type === "pong") {
        message = parsed as EventMessage;
      } else {
        if (typeof type === "string" && type !== "event" && type !== "log_tail" && this.socket === socket) socket.close();
        return;
      }
    } catch {
      return;
    }
    if (message.type === "ping") {
      if (this.socket === socket) socket.send(JSON.stringify({ type: "pong" }));
      return;
    }
    if (message.type === "pong") {
      if (!this.hasExactKeys(message, ["type"])) socket.close();
      return;
    }
    if (message.type === "event" || message.type === "log_tail") {
      if (!this.isSequenced(message)) return;
      if (this.resyncing) {
        const serializedBytes = this.serializedByteLength(message);
        if (this.pendingDuringResync.length < EventClient.maxPendingDuringResync
          && this.pendingResyncBytes + serializedBytes <= EventClient.maxPendingDuringResyncBytes) {
          this.pendingDuringResync.push(message);
          this.pendingResyncBytes += serializedBytes;
        } else this.pendingOverflowed = true;
        return;
      }
      this.deliver(message);
      return;
    }
    if (message.type === "gap" || message.type === "overflow") {
      const after = "after" in message ? message.after : undefined;
      if (this.resyncing) {
        this.pendingOverflowed = true;
        if (after !== undefined) this.pendingResyncAfter = after;
      } else void this.resync(after);
    }
  }

  private deliver(message: ManagerEvent | Extract<EventMessage, { type: "log_tail" }>): void {
    if (!this.isSequenced(message) || message.sequence <= this.highestSequence) return;
    this.highestSequence = message.sequence;
    this.cursor = message.id;
    if (message.type === "event") this.onEvent?.(message);
    else this.onLogTail?.(message);
  }

  private async resync(after = this.resyncAfter, attempt = 0, generation = ++this.resyncGeneration): Promise<void> {
    if (this.resyncing) {
      this.pendingOverflowed = true;
      return;
    }
    this.resyncing = true;
    if (attempt === 0) {
      this.pendingDuringResync = [];
      this.pendingResyncBytes = 0;
      this.resyncAfter = after;
    }
    let completed = false;
    let abortController: AbortController | null = null;
    try {
      let snapshot: Snapshot;
      for (let currentAttempt = attempt; ; currentAttempt += 1) {
        abortController = new AbortController();
        this.resyncAbortController = abortController;
        try {
          const endpoint = typeof this.resyncAfter === "string"
            ? `/events?after=${encodeURIComponent(this.resyncAfter)}`
            : "/events";
          snapshot = await this.requestWithTimeout(endpoint, abortController, async (response, signal) => {
            if (!response.ok) throw new Error(`snapshot request failed: ${response.status}`);
            return this.readSnapshot(response, signal);
          });
          let pageCursor = snapshot.meta.next_cursor;
          let pageCount = 1;
          const seenCursors = new Set<string>();
          while (pageCursor !== null && snapshot.data.length < EventClient.maxResyncRows) {
            if (pageCount >= EventClient.maxResyncPages || seenCursors.has(pageCursor)
              || (typeof this.resyncAfter === "string" && pageCursor === this.resyncAfter)) {
              throw new Error("snapshot cursor did not progress");
            }
            seenCursors.add(pageCursor);
            const page = await this.requestWithTimeout(`/events?after=${encodeURIComponent(pageCursor)}`, abortController, async (pageResponse, signal) => {
              if (!pageResponse.ok) throw new Error(`snapshot page request failed: ${pageResponse.status}`);
              return this.readSnapshot(pageResponse, signal);
            });
            for (const row of page.data) {
              if (snapshot.data.length >= EventClient.maxResyncRows) break;
              snapshot.data.push(row);
            }
            if (page.meta.next_cursor !== null && (page.meta.next_cursor === pageCursor || seenCursors.has(page.meta.next_cursor))) {
              throw new Error("snapshot cursor did not progress");
            }
            snapshot.meta = page.meta;
            pageCursor = page.meta.next_cursor;
            pageCount += 1;
          }
          if (pageCursor !== null && snapshot.data.length >= EventClient.maxResyncRows) {
            const lastRow = snapshot.data.at(-1);
            if (!lastRow) throw new Error("snapshot row limit reached without a row");
            snapshot.meta = {
              current_cursor: this.isBoundedString(lastRow.id) ? lastRow.id : null,
              next_cursor: pageCursor,
              current_sequence: this.isValidSequence(lastRow.sequence) ? lastRow.sequence : undefined,
            };
          }
          break;
        } catch (error) {
          if (this.isAbortError(error)) throw error;
          if (currentAttempt >= EventClient.maxResyncAttempts) throw error;
        }
      }
      if (generation !== this.resyncGeneration || this.stopped) return;
      const pending = this.pendingDuringResync;
      this.pendingDuringResync = [];
      this.pendingResyncBytes = 0;
      const snapshotSequence = (snapshot!.data ?? [])
        .map((row) => row.sequence)
        .filter((sequence): sequence is number => this.isValidSequence(sequence))
        .reduce((highest, sequence) => Math.max(highest, sequence), -1);
      const currentSequence = snapshot!.meta?.current_sequence;
      const pendingSequences = pending
        .map((queued) => {
          if (queued.type !== "event" && queued.type !== "log_tail") return undefined;
          return (queued as ManagerEvent | Extract<EventMessage, { type: "log_tail" }>).sequence;
        })
        .filter((sequence): sequence is number => this.isValidSequence(sequence));
      const pendingAtBoundary = this.isValidSequence(currentSequence) && pendingSequences.includes(currentSequence);
      const canonicalCursor = snapshot!.meta?.current_cursor;
      if (this.isBoundedString(canonicalCursor) && this.isValidSequence(currentSequence)
        && currentSequence >= this.highestSequence && !pendingAtBoundary) {
        this.highestSequence = currentSequence;
        this.cursor = canonicalCursor;
      } else if (this.isBoundedString(canonicalCursor) && snapshotSequence > this.highestSequence) {
        this.highestSequence = snapshotSequence;
        this.cursor = canonicalCursor;
      }
      this.onResync?.(snapshot!);
      for (const queued of pending) this.deliver(queued as ManagerEvent | Extract<EventMessage, { type: "log_tail" }>);
      completed = snapshot!.meta.next_cursor === null;
      if (!completed && generation === this.resyncGeneration && !this.stopped && this.socket) {
        this.resyncAfter = snapshot!.meta.current_cursor;
        this.preservePendingOnReconnect = true;
        this.socket.close();
      }
    } catch (error) {
      if (this.isAbortError(error)) return;
      if (generation === this.resyncGeneration && !this.stopped && this.socket) {
        this.preservePendingOnReconnect = true;
        this.socket.close();
      }
    } finally {
      if (generation === this.resyncGeneration) {
        if (this.resyncAbortController === abortController) this.resyncAbortController = null;
        this.resyncing = false;
        if (completed && this.pendingOverflowed) {
          const after = this.pendingResyncAfter;
          this.pendingResyncAfter = undefined;
          this.pendingOverflowed = false;
          this.resyncAfter = undefined;
          if (this.socket && !this.stopped) void this.resync(after);
        } else if (completed) {
          this.resyncAfter = undefined;
        }
      }
    }
  }

  private async readSnapshot(response: Response, signal: AbortSignal): Promise<Snapshot> {
    let text: string;
    if (response.body && typeof response.body.getReader === "function") {
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let totalBytes = 0;
      let chunks = "";
      const cancelReader = () => { void reader.cancel(); };
      signal.addEventListener("abort", cancelReader, { once: true });
      try {
        for (;;) {
          const result = await reader.read();
          if (result.done) {
            chunks += decoder.decode();
            break;
          }
          totalBytes += result.value.byteLength;
          if (totalBytes > EventClient.maxResyncResponseBytes) {
            await reader.cancel();
            throw new Error("snapshot response too large");
          }
          chunks += decoder.decode(result.value, { stream: true });
        }
      } finally {
        signal.removeEventListener("abort", cancelReader);
        reader.releaseLock();
      }
      text = chunks;
    } else {
      throw new Error("snapshot response is not stream-readable");
    }
    let parsed: unknown;
    try { parsed = JSON.parse(text); } catch { throw new Error("malformed snapshot JSON"); }
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("malformed snapshot envelope");
    const candidate = parsed as { data?: unknown; meta?: unknown };
    if (!Array.isArray(candidate.data) || candidate.data.length > EventClient.maxResyncRows || candidate.meta === null || typeof candidate.meta !== "object" || Array.isArray(candidate.meta)) throw new Error("malformed snapshot envelope");
    const meta = candidate.meta as Record<string, unknown>;
    if (!this.isCursor(meta.current_cursor) || (meta.next_cursor !== undefined && !this.isCursor(meta.next_cursor))) throw new Error("malformed snapshot cursor");
    if (meta.current_sequence !== undefined && !this.isValidSequence(meta.current_sequence, true)) throw new Error("malformed snapshot sequence");
    const data: Array<Record<string, unknown>> = [];
    for (const row of candidate.data) {
      if (row === null || typeof row !== "object" || Array.isArray(row)) throw new Error("malformed snapshot row");
      const value = row as Record<string, unknown>;
      if (!this.isBoundedString(value.id) || !this.isValidSequence(value.sequence) || !this.isBoundedString(value.topic) || !this.isObjectPayload(value.payload)) throw new Error("malformed snapshot event");
      if (value.item_id !== undefined && value.item_id !== null && !this.isBoundedString(value.item_id)) throw new Error("malformed snapshot log tail");
      data.push(value);
    }
    const metadataSequence = meta.current_sequence;
    if (metadataSequence === 0 && (data.length !== 0 || meta.current_cursor !== null)) throw new Error("invalid empty snapshot boundary");
    if (this.isValidSequence(metadataSequence)) {
      if (data.some((row) => (row.sequence as number) > metadataSequence)) throw new Error("snapshot sequence boundary mismatch");
      if (this.isBoundedString(meta.current_cursor)) {
        const boundaryRow = data.find((row) => row.id === meta.current_cursor);
        if (boundaryRow && boundaryRow.sequence !== metadataSequence) throw new Error("snapshot boundary sequence mismatch");
      }
    }
    return { data, meta: { current_cursor: meta.current_cursor === undefined ? null : meta.current_cursor, next_cursor: meta.next_cursor === undefined ? null : meta.next_cursor, ...(metadataSequence === undefined ? {} : { current_sequence: metadataSequence }) } };
  }

  private async requestWithTimeout<T = Response>(
    path: string,
    controller: AbortController,
    consume: (response: Response, signal: AbortSignal) => Promise<T> | T = (response) => response as T,
  ): Promise<T> {
    const signal = controller.signal;
    let rejectAbort!: (reason: unknown) => void;
    const aborted = new Promise<never>((_, reject) => { rejectAbort = reject; });
    let timedOut = false;
    const abortRequest = () => {
      if (!timedOut) rejectAbort(new DOMException("aborted", "AbortError"));
    };
    signal.addEventListener("abort", abortRequest, { once: true });
    let timer: ReturnType<typeof setTimeout> | null = null;
    const request = Promise.resolve(this.api.request(path, { signal }));
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        timedOut = true;
        controller.abort();
        reject(new Error("snapshot request timed out"));
      }, EventClient.resyncRequestTimeoutMs);
    });
    try {
      const response = await Promise.race([request, timeout, aborted]);
      return await Promise.race([Promise.resolve(consume(response, signal)), timeout, aborted]);
    } finally {
      if (timer !== null) clearTimeout(timer);
      timer = null;
      signal.removeEventListener("abort", abortRequest);
    }
  }

  private isCursor(value: unknown): value is string | null {
    return value === null || this.isBoundedString(value);
  }

  private isAbortError(error: unknown): boolean {
    return error instanceof DOMException && error.name === "AbortError";
  }

  private isSequenced(message: EventMessage): message is ManagerEvent | Extract<EventMessage, { type: "log_tail" }> {
    if (message.type !== "event" && message.type !== "log_tail") return false;
    const candidate = message as ManagerEvent | Extract<EventMessage, { type: "log_tail" }>;
    if (!this.isBoundedString(candidate.id) || !this.isValidSequence(candidate.sequence)) return false;
    if (candidate.type === "event") return this.isBoundedString(candidate.topic) && this.isObjectPayload(candidate.payload);
    return (candidate.item_id === null || this.isBoundedString(candidate.item_id))
      && this.isBoundedString(candidate.text)
      && typeof candidate.offset === "number" && Number.isSafeInteger(candidate.offset) && candidate.offset >= 0;
  }

  private isSequencedFrame(value: object): boolean {
    const candidate = value as { type?: unknown };
    return candidate.type === "event" || candidate.type === "log_tail"
      ? this.isSequenced(candidate as EventMessage)
      : false;
  }

  private isControlFrame(value: object): boolean {
    const candidate = value as Record<string, unknown>;
    switch (candidate.type) {
      case "ping":
      case "pong": return this.hasExactKeys(candidate, ["type"]);
      case "hello":
        return this.hasExactKeys(candidate, ["type", "version", "current_event_id", "oldest_event_id"])
          && typeof candidate.version === "number" && Number.isSafeInteger(candidate.version) && candidate.version >= 0
          && this.isCursor(candidate.current_event_id) && this.isCursor(candidate.oldest_event_id);
      case "gap":
        return this.hasExactKeys(candidate, ["type", "after", "reason"])
          && this.isCursor(candidate.after) && this.isBoundedString(candidate.reason);
      case "overflow":
        return (this.hasExactKeys(candidate, ["type", "reason"]) || this.hasExactKeys(candidate, ["type", "after", "reason"]))
          && (candidate.after === undefined || this.isCursor(candidate.after))
          && this.isBoundedString(candidate.reason);
      case "error":
        return this.hasExactKeys(candidate, ["type", "reason"]) && this.isBoundedString(candidate.reason);
      default: return false;
    }
  }

  private hasExactKeys(value: object, keys: string[]): boolean {
    const actual = Object.keys(value).sort();
    const expected = [...keys].sort();
    return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
  }

  private isBoundedString(value: unknown): value is string {
    return typeof value === "string" && value.length > 0 && value.length <= EventClient.maxProtocolStringLength;
  }

  private isObjectPayload(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object" && !Array.isArray(value);
  }

  private byteLength(value: unknown): number {
    if (typeof value !== "string") return Number.POSITIVE_INFINITY;
    return new TextEncoder().encode(value).byteLength;
  }

  private serializedByteLength(message: EventMessage): number {
    return this.byteLength(JSON.stringify(message));
  }

  private isValidSequence(sequence: unknown, allowZero = false): sequence is number {
    return typeof sequence === "number" && Number.isSafeInteger(sequence) && (sequence > 0 || (allowZero && sequence === 0));
  }
}
