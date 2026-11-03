import { describe, expect, it, vi } from "vitest";
import { EventClient } from "../src/lib/events";
import { realtimeState } from "../src/lib/realtime";

type SocketHandler = ((event: MessageEvent) => void) | null;

class FakeSocket {
  static instances: FakeSocket[] = [];
  onopen: (() => void) | null = null;
  onmessage: SocketHandler = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  sent: string[] = [];
  readyState = 0;

  constructor(public readonly url: string) {
    FakeSocket.instances.push(this);
  }

  send(value: string) { this.sent.push(value); }
  close() { this.readyState = 3; this.onclose?.(); }
  open() { this.readyState = 1; this.onopen?.(); }
  message(value: unknown) { this.onmessage?.(new MessageEvent("message", { data: JSON.stringify(value) })); }
}

describe("durable event client", () => {
  it("keeps the last event cursor in memory and sends it on reconnect", () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const onLogTail = vi.fn();
    const client = new EventClient({ api: { request: vi.fn() }, onLogTail });

    client.connect();
    const first = FakeSocket.instances[0]!;
    first.open();
    expect(first.url).toContain("ws://");
    expect(first.url).toContain("/api/ws");
    first.message({ type: "hello", version: 1, current_event_id: null, oldest_event_id: null });
    first.message({ type: "ping" });
    expect(first.sent).toContain(JSON.stringify({ type: "pong" }));
    first.message({ type: "event", id: "cursor-1", sequence: 1, topic: "job.updated", payload: {} });
    first.message({ type: "log_tail", id: "cursor-2", sequence: 2, item_id: "i_1", offset: 0, text: "tail" });
    expect(onLogTail).toHaveBeenCalledWith(expect.objectContaining({ id: "cursor-2", text: "tail" }));
    first.close();

    client.connect();
    const second = FakeSocket.instances[1]!;
    expect(second.url).toContain("after=cursor-2");
    client.close();
    vi.unstubAllGlobals();
  });

  it("requests a REST snapshot after a server gap", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const request = vi.fn(async () => new Response(JSON.stringify({ data: [], meta: { current_cursor: null, next_cursor: null } }), { status: 200 }));
    const onResync = vi.fn();
    const client = new EventClient({ api: { request }, onResync });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: "cursor-4", reason: "retention" });

    await vi.waitFor(() => expect(request).toHaveBeenCalledWith("/events?after=cursor-4", expect.objectContaining({ signal: expect.any(AbortSignal) })));
    await vi.waitFor(() => expect(onResync).toHaveBeenCalledWith(expect.objectContaining({ data: [] })));
    expect(client.lastCursor).toBeNull();
    client.close();
    vi.unstubAllGlobals();
  });

  it("does not let malformed snapshot metadata replace the last valid realtime snapshot", () => {
    const valid = { data: [], meta: { current_cursor: null, next_cursor: null, current_sequence: 0 } };
    realtimeState.applySnapshot(valid);
    expect(realtimeState.snapshot()).toEqual(valid);
    realtimeState.applySnapshot({ data: [], meta: { current_cursor: 1, next_cursor: null, current_sequence: 0 } });
    expect(realtimeState.snapshot()).toEqual(valid);
  });

  it("merges live events arriving during resync and keeps the cursor monotone", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    let resolveRequest!: (response: Response) => void;
    const request = vi.fn(() => new Promise<Response>((resolve) => { resolveRequest = resolve; }));
    const client = new EventClient({
      api: { request },
      onEvent: realtimeState.applyEvent,
      onResync: realtimeState.applySnapshot,
    });
    realtimeState.applySnapshot({ data: [] });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledWith("/events", expect.objectContaining({ signal: expect.any(AbortSignal) })));

    socket.message({ type: "event", id: "cursor-live", sequence: 2, topic: "job.updated", payload: { status: "running" } });
    resolveRequest(new Response(JSON.stringify({
      data: [{ id: "cursor-snapshot", sequence: 1, topic: "job.updated", payload: { status: "queued" } }],
      meta: { current_cursor: "cursor-snapshot" },
    }), { status: 200 }));

    await vi.waitFor(() => expect(realtimeState.events().map((event) => event.id)).toEqual([
      "cursor-snapshot",
      "cursor-live",
    ]));
    expect(client.lastCursor).toBe("cursor-live");
    client.close();
    realtimeState.applySnapshot({ data: [] });
    vi.unstubAllGlobals();
  });

  it("invalidates an in-flight REST resync when the socket reconnects", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    let resolveRequest!: (response: Response) => void;
    const request = vi.fn(() => new Promise<Response>((resolve) => { resolveRequest = resolve; }));
    const onResync = vi.fn();
    const client = new EventClient({ api: { request }, onResync });
    client.connect();
    const first = FakeSocket.instances.at(-1)!;
    first.open();
    first.message({ type: "gap", after: null, reason: "overflow" });
    await vi.waitFor(() => expect(request).toHaveBeenCalled());
    first.close();
    client.connect();
    const second = FakeSocket.instances.at(-1)!;
    second.open();
    second.message({ type: "event", id: "cursor-new", sequence: 2, topic: "job.updated", payload: {} });
    resolveRequest(new Response(JSON.stringify({ data: [], meta: { current_cursor: "cursor-old" } }), { status: 200 }));
    await Promise.resolve();
    expect(onResync).not.toHaveBeenCalled();
    expect(client.lastCursor).toBe("cursor-new");
    client.close();
    vi.unstubAllGlobals();
  });

  it("keeps an in-flight gap resync across an automatic reconnect and applies its snapshot once", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    vi.useFakeTimers();
    let resolveRequest!: (response: Response) => void;
    const request = vi.fn(() => new Promise<Response>((resolve) => { resolveRequest = resolve; }));
    const onResync = vi.fn();
    const client = new EventClient({ api: { request }, onResync });
    client.connect();
    const first = FakeSocket.instances.at(-1)!;
    first.open();
    first.message({ type: "gap", after: "cursor-before-gap", reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledWith("/events?after=cursor-before-gap", expect.objectContaining({ signal: expect.any(AbortSignal) })));

    first.close();
    await vi.advanceTimersByTimeAsync(250);
    const second = FakeSocket.instances.at(-1)!;
    second.open();
    expect(request).toHaveBeenCalledTimes(1);

    resolveRequest(new Response(JSON.stringify({
      data: [{ id: "cursor-dropped", sequence: 5, topic: "job.updated", payload: {} }],
      meta: { current_cursor: "cursor-snapshot", current_sequence: 5 },
    }), { status: 200 }));
    await vi.waitFor(() => expect(onResync).toHaveBeenCalledTimes(1));
    expect(onResync).toHaveBeenCalledWith(expect.objectContaining({
      data: [expect.objectContaining({ id: "cursor-dropped" })],
    }));
    expect(client.lastCursor).toBe("cursor-snapshot");
    client.close();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("does not accept pong payloads", () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const client = new EventClient({ api: { request: vi.fn() } });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "pong", payload: "unexpected" });
    expect(socket.readyState).toBe(3);
    client.close();
    vi.unstubAllGlobals();
  });

  it("closes the current socket for an unknown object control type", () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const client = new EventClient({ api: { request: vi.fn() } });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "future-control", value: "must not be ignored" });
    expect(socket.readyState).toBe(3);
    client.close();
    vi.unstubAllGlobals();
  });

  it.each([
    ["hello", { type: "hello", version: 1, current_event_id: null }],
    ["gap", { type: "gap", after: 42, reason: "retention" }],
    ["overflow", { type: "overflow", after: null, reason: "" }],
    ["error", { type: "error", reason: "server", extra: true }],
    ["ping", { type: "ping", payload: "unexpected" }],
  ])("closes the socket deterministically for a malformed %s control frame", (_type, frame) => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const client = new EventClient({ api: { request: vi.fn() } });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message(frame);
    expect(socket.readyState).toBe(3);
    client.close();
    vi.unstubAllGlobals();
  });

  it("ignores malformed JSON values that are not protocol objects", () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const onEvent = vi.fn();
    const client = new EventClient({ api: { request: vi.fn() }, onEvent });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    expect(() => socket.message(null)).not.toThrow();
    expect(() => socket.message([])).not.toThrow();
    expect(onEvent).not.toHaveBeenCalled();
    client.close();
    vi.unstubAllGlobals();
  });

  it("deduplicates different opaque cursors by stable sequence and keeps cursor monotone", () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const onEvent = vi.fn();
    const client = new EventClient({ api: { request: vi.fn() }, onEvent });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "event", id: "opaque-high", sequence: 10, topic: "job.updated", payload: {} });
    socket.message({ type: "event", id: "opaque-duplicate", sequence: 10, topic: "job.updated", payload: {} });
    socket.message({ type: "event", id: "opaque-low", sequence: 9, topic: "job.updated", payload: {} });
    expect(onEvent).toHaveBeenCalledTimes(1);
    expect(client.lastCursor).toBe("opaque-high");
    client.close();
    vi.unstubAllGlobals();
  });

  it("rejects non-positive, fractional, unsafe, and non-finite sequences", () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const onEvent = vi.fn();
    const client = new EventClient({ api: { request: vi.fn() }, onEvent });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    for (const sequence of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, Infinity, NaN]) {
      socket.message({ type: "event", id: `bad-${String(sequence)}`, sequence, topic: "job.updated", payload: {} });
    }
    expect(onEvent).not.toHaveBeenCalled();
    socket.message({ type: "event", id: "good", sequence: 1, topic: "job.updated", payload: {} });
    expect(onEvent).toHaveBeenCalledTimes(1);
    client.close();
    vi.unstubAllGlobals();
  });

  it("rejects malformed snapshot sequences without advancing the cursor or highest sequence", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const request = vi.fn(async () => new Response(JSON.stringify({
      data: [
        { id: "fractional", sequence: 99.5, topic: "job.updated", payload: {} },
        { id: "negative", sequence: -1, topic: "job.updated", payload: {} },
        { id: "unsafe", sequence: Number.MAX_SAFE_INTEGER + 1, topic: "job.updated", payload: {} },
      ],
      meta: { current_cursor: "malformed-boundary" },
    }), { status: 200 }));
    const onEvent = vi.fn();
    const client = new EventClient({ api: { request }, onEvent });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    expect(onEvent).not.toHaveBeenCalled();
    expect(client.lastCursor).toBeNull();
    client.close();
    vi.unstubAllGlobals();
  });

  it("adopts the canonical empty or stale REST boundary from current_sequence", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const request = vi.fn(async () => new Response(JSON.stringify({
      data: [{ id: "stale", sequence: 2, topic: "job.updated", payload: {} }],
      meta: { current_cursor: "canonical-10", current_sequence: 10 },
    }), { status: 200 }));
    const client = new EventClient({ api: { request } });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "event", id: "live-5", sequence: 5, topic: "job.updated", payload: {} });
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(client.lastCursor).toBe("canonical-10"));
    socket.message({ type: "event", id: "live-11", sequence: 11, topic: "job.updated", payload: {} });
    expect(client.lastCursor).toBe("live-11");
    client.close();
    vi.unstubAllGlobals();
  });

  it("preserves queued live events and replays them after a failed REST resync retry", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    let attempt = 0;
    const request = vi.fn(async () => {
      attempt += 1;
      if (attempt === 1) return new Response("unavailable", { status: 503 });
      return new Response(JSON.stringify({ data: [], meta: { current_cursor: "canonical-1", current_sequence: 1 } }), { status: 200 });
    });
    const onEvent = vi.fn();
    const client = new EventClient({ api: { request }, onEvent });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(1));
    socket.message({ type: "event", id: "queued", sequence: 1, topic: "job.updated", payload: {} });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(onEvent).toHaveBeenCalledWith(expect.objectContaining({ id: "queued" })));
    expect(onEvent).toHaveBeenCalledTimes(1);
    client.close();
    vi.unstubAllGlobals();
  });

  it("uses a later overflow boundary for the bounded follow-up resync", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const resolvers: Array<(response: Response) => void> = [];
    const request = vi.fn(() => new Promise<Response>((resolve) => resolvers.push(resolve)));
    const client = new EventClient({ api: { request: request as never } });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: "cursor-start", reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledWith("/events?after=cursor-start", expect.objectContaining({ signal: expect.any(AbortSignal) })));
    socket.message({ type: "overflow", after: "cursor-overflow", reason: "slow consumer" });
    resolvers[0]!(new Response(JSON.stringify({ data: [], meta: { current_cursor: "cursor-snapshot", current_sequence: 1 } }), { status: 200 }));
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    expect(request).toHaveBeenCalledTimes(2);
    resolvers[1]!(new Response(JSON.stringify({ data: [], meta: { current_cursor: "cursor-final", current_sequence: 2 } }), { status: 200 }));
    client.close();
    vi.unstubAllGlobals();
  });

  it("bounds queued resync messages and serializes overflow recovery", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const resolvers: Array<(response: Response) => void> = [];
    const request = vi.fn(() => new Promise<Response>((resolve) => resolvers.push(resolve)));
    const onEvent = vi.fn();
    const client = new EventClient({ api: { request: request as never }, onEvent });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(1));
    for (let sequence = 1; sequence <= 1001; sequence += 1) {
      socket.message({ type: "event", id: `opaque-${sequence}`, sequence, topic: "job.updated", payload: {} });
    }
    resolvers[0]!(new Response(JSON.stringify({ data: [], meta: { current_cursor: null } }), { status: 200 }));
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    expect(request).toHaveBeenCalledTimes(2);
    resolvers[1]!(new Response(JSON.stringify({ data: [], meta: { current_cursor: null } }), { status: 200 }));
    await vi.waitFor(() => expect(onEvent).toHaveBeenCalledTimes(1000));
    client.close();
    vi.unstubAllGlobals();
  });

  it("does not deliver live frames while a failed resync retry is still in flight", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const resolvers: Array<(response: Response) => void> = [];
    const request = vi.fn(() => new Promise<Response>((resolve) => resolvers.push(resolve)));
    const onEvent = vi.fn();
    const client = new EventClient({ api: { request: request as never }, onEvent });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(1));
    resolvers[0]!(new Response("unavailable", { status: 503 }));
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    socket.message({ type: "event", id: "retry-queued", sequence: 1, topic: "job.updated", payload: {} });
    expect(onEvent).not.toHaveBeenCalled();
    resolvers[1]!(new Response(JSON.stringify({ data: [], meta: { current_cursor: null, next_cursor: null } }), { status: 200 }));
    await vi.waitFor(() => expect(onEvent).toHaveBeenCalledWith(expect.objectContaining({ id: "retry-queued" })));
    client.close();
    vi.unstubAllGlobals();
  });

  it("retries an exhausted empty resync after reconnect", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    vi.useFakeTimers();
    let attempt = 0;
    const request = vi.fn(async () => {
      attempt += 1;
      return new Response(attempt <= 2 ? "unavailable" : JSON.stringify({ data: [], meta: { current_sequence: 2, current_cursor: "fresh" } }), { status: attempt <= 2 ? 503 : 200 });
    });
    const onResync = vi.fn();
    const client = new EventClient({ api: { request }, onResync });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    await vi.advanceTimersByTimeAsync(250);
    const reconnected = FakeSocket.instances.at(-1)!;
    reconnected.open();
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(3));
    await vi.waitFor(() => expect(onResync).toHaveBeenCalledTimes(1));
    expect(client.lastCursor).toBe("fresh");
    client.close();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("closes oversized raw frames before parsing them", () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const client = new EventClient({ api: { request: vi.fn() } });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.onmessage?.(new MessageEvent("message", { data: `{"type":"event","id":"x","sequence":1,"topic":"x","payload":${JSON.stringify("x".repeat(70_000))}}` }));
    expect(socket.readyState).toBe(3);
    client.close();
    vi.unstubAllGlobals();
  });

  it("rejects event and log-tail frames with unbounded or invalid fields", () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const onEvent = vi.fn();
    const onLogTail = vi.fn();
    const client = new EventClient({ api: { request: vi.fn() }, onEvent, onLogTail });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    for (const message of [
      { type: "event", id: "x", sequence: 1, topic: "x", payload: [] },
      { type: "event", id: "x", sequence: 2, topic: "x" },
      { type: "event", id: "x", sequence: 3, topic: "x".repeat(5_000), payload: {} },
      { type: "log_tail", id: "x", sequence: 4, item_id: 1, offset: 0, text: "x" },
      { type: "log_tail", id: "x", sequence: 5, item_id: null, offset: 1.5, text: "x" },
      { type: "log_tail", id: "x", sequence: 6, item_id: null, offset: 0, text: "x".repeat(5_000) },
    ]) socket.message(message);
    expect(onEvent).not.toHaveBeenCalled();
    expect(onLogTail).not.toHaveBeenCalled();
    client.close();
    vi.unstubAllGlobals();
  });

  it("bounds queued resync frames by serialized byte size", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    let resolveRequest!: (response: Response) => void;
    const request = vi.fn(() => new Promise<Response>((resolve) => { resolveRequest = resolve; }));
    const onEvent = vi.fn();
    const client = new EventClient({ api: { request: request as never }, onEvent });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(1));
    for (let sequence = 1; sequence <= 20; sequence += 1) {
      socket.message({ type: "event", id: `byte-${sequence}`, sequence, topic: "job.updated", payload: { text: "x".repeat(20_000) } });
    }
    resolveRequest(new Response(JSON.stringify({ data: [], meta: { current_cursor: null, next_cursor: null } }), { status: 200 }));
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    resolveRequest(new Response(JSON.stringify({ data: [], meta: { current_cursor: null, next_cursor: null } }), { status: 200 }));
    await vi.waitFor(() => expect(onEvent).toHaveBeenCalledTimes(13));
    client.close();
    vi.unstubAllGlobals();
  });

  it("rejects malformed and oversized REST snapshots before resync delivery", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const request = vi.fn(async () => new Response(JSON.stringify({ data: [{ id: "bad", sequence: 1, topic: "job.updated", payload: [] }] }), { status: 200 }));
    const onResync = vi.fn();
    const client = new EventClient({ api: { request }, onResync });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    expect(onResync).not.toHaveBeenCalled();
    client.close();

    const oversizedBody = JSON.stringify({ data: [], meta: { current_cursor: null, next_cursor: null }, padding: "x".repeat(70_000) });
    const oversizedRequest = vi.fn(async () => new Response(oversizedBody, { status: 200 }));
    const oversizedClient = new EventClient({ api: { request: oversizedRequest }, onResync });
    oversizedClient.connect();
    const oversizedSocket = FakeSocket.instances.at(-1)!;
    oversizedSocket.open();
    oversizedSocket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(oversizedRequest).toHaveBeenCalledTimes(2));
    expect(onResync).not.toHaveBeenCalled();
    oversizedClient.close();
  });

  it("does not adopt a page boundary beyond the last row when the 1000-row cap truncates recovery", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    vi.useFakeTimers();
    const row = (sequence: number) => ({ id: `row-${sequence}`, sequence, topic: "job.updated", payload: {} });
    const requests: string[] = [];
    const request = vi.fn(async (path: string) => {
      requests.push(path);
      if (requests.length === 1) return new Response(JSON.stringify({
        data: Array.from({ length: 600 }, (_, index) => row(index + 1)),
        meta: { current_cursor: "page-1-boundary", next_cursor: "opaque-page-2" },
      }), { status: 200 });
      if (requests.length === 2) return new Response(JSON.stringify({
        data: Array.from({ length: 600 }, (_, index) => row(index + 601)),
        meta: { current_cursor: "server-snapshot-boundary", current_sequence: 2000, next_cursor: "opaque-page-3" },
      }), { status: 200 });
      return new Response(JSON.stringify({ data: [], meta: { current_cursor: "server-snapshot-boundary", current_sequence: 2000, next_cursor: null } }), { status: 200 });
    });
    const client = new EventClient({ api: { request } });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    expect(client.lastCursor).toBe("row-1000");
    await vi.advanceTimersByTimeAsync(250);
    const reconnected = FakeSocket.instances.at(-1)!;
    reconnected.open();
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(3));
    expect(requests[2]).toBe("/events?after=row-1000");
    client.close();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("reads REST snapshots through a bounded byte stream without calling unbounded text", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const oversized = new Uint8Array(64 * 1024 + 1);
    const request = vi.fn(async () => ({
      ok: true,
      status: 200,
      body: new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(oversized);
          controller.close();
        },
      }),
      text: vi.fn(() => { throw new Error("unbounded text fallback used"); }),
    }) as unknown as Response);
    const onResync = vi.fn();
    const client = new EventClient({ api: { request }, onResync });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    expect(onResync).not.toHaveBeenCalled();
    client.close();
    vi.unstubAllGlobals();
  });

  it("times out a REST request that ignores AbortSignal and preserves recovery for reconnect", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    vi.useFakeTimers();
    const request = vi.fn(() => new Promise<Response>(() => undefined));
    const client = new EventClient({ api: { request } });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: "opaque-start", reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(1));
    await Promise.resolve();
    await vi.advanceTimersByTimeAsync(20_001);
    await vi.waitFor(() => expect(socket.readyState).toBe(3));
    await vi.advanceTimersByTimeAsync(250);
    const reconnected = FakeSocket.instances.at(-1)!;
    reconnected.open();
    await vi.waitFor(() => expect(request.mock.calls.length).toBeGreaterThanOrEqual(2));
    expect(request).toHaveBeenLastCalledWith("/events?after=opaque-start", expect.objectContaining({ signal: expect.any(AbortSignal) }));
    client.close();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("aborts a hung resync on close without treating cancellation as a failed recovery", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    let signal!: AbortSignal;
    const request = vi.fn((_path: string, init?: RequestInit) => new Promise<Response>((_resolve, reject) => {
      signal = init?.signal as AbortSignal;
      signal.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")), { once: true });
    }));
    const client = new EventClient({ api: { request } });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(1));
    client.close();
    expect(signal.aborted).toBe(true);
    await Promise.resolve();
    expect(request).toHaveBeenCalledTimes(1);
    vi.unstubAllGlobals();
  });

  it("does not let snapshot sequence metadata without a canonical cursor drop queued recovery events", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    let resolveRequest!: (response: Response) => void;
    const request = vi.fn(() => new Promise<Response>((resolve) => { resolveRequest = resolve; }));
    const onEvent = vi.fn();
    const client = new EventClient({ api: { request }, onEvent });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(1));
    socket.message({ type: "event", id: "queued-before-boundary", sequence: 5, topic: "job.updated", payload: {} });
    resolveRequest(new Response(JSON.stringify({ data: [], meta: { current_cursor: null, current_sequence: 10 } }), { status: 200 }));
    await vi.waitFor(() => expect(onEvent).toHaveBeenCalledWith(expect.objectContaining({ id: "queued-before-boundary" })));
    expect(client.lastCursor).toBe("queued-before-boundary");
    client.close();
    vi.unstubAllGlobals();
  });

  it("rejects a snapshot whose known cursor row disagrees with current_sequence", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const request = vi.fn(async () => new Response(JSON.stringify({
      data: [{ id: "known-boundary", sequence: 5, topic: "job.updated", payload: {} }],
      meta: { current_cursor: "known-boundary", current_sequence: 6 },
    }), { status: 200 }));
    const onResync = vi.fn();
    const client = new EventClient({ api: { request }, onResync });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    expect(onResync).not.toHaveBeenCalled();
    client.close();
    vi.unstubAllGlobals();
  });

  it("rejects repeated pagination cursors without lexical cursor comparison", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const request = vi.fn(async (path: string) => new Response(JSON.stringify({
      data: [{ id: path.includes("page-2") ? "row-2" : "row-1", sequence: path.includes("page-2") ? 2 : 1, topic: "job.updated", payload: {} }],
      meta: { current_cursor: "boundary", next_cursor: "opaque-page-2" },
    }), { status: 200 }));
    const onResync = vi.fn();
    const client = new EventClient({ api: { request }, onResync });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(4));
    expect(onResync).not.toHaveBeenCalled();
    client.close();
    vi.unstubAllGlobals();
  });

  it("safely rejects a response without a streaming reader instead of using unbounded text", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const text = vi.fn(async () => JSON.stringify({ data: [], meta: { current_cursor: null, next_cursor: null } }));
    const request = vi.fn(async () => ({ ok: true, status: 200, body: null, text }) as unknown as Response);
    const onResync = vi.fn();
    const client = new EventClient({ api: { request }, onResync });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    expect(text).not.toHaveBeenCalled();
    expect(onResync).not.toHaveBeenCalled();
    client.close();
    vi.unstubAllGlobals();
  });

  it("times out and cancels a response body that never finishes reading", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    vi.useFakeTimers();
    let cancelled = false;
    const request = vi.fn(async () => ({
      ok: true,
      status: 200,
      body: new ReadableStream<Uint8Array>({
        cancel() { cancelled = true; },
      }),
    }) as unknown as Response);
    const client = new EventClient({ api: { request } });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: "opaque-start", reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(1));
    await vi.advanceTimersByTimeAsync(10_001);
    await vi.waitFor(() => expect(cancelled).toBe(true));
    client.close();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("shares one signal across paginated snapshot requests and creates a new signal for retry", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const signals: AbortSignal[] = [];
    let attempt = 0;
    const request = vi.fn(async (path: string, init?: RequestInit) => {
      signals.push(init?.signal as AbortSignal);
      attempt += 1;
      if (attempt === 1) return new Response("unavailable", { status: 503 });
      if (path === "/events") {
        return new Response(JSON.stringify({ data: [], meta: { current_cursor: null, next_cursor: "page-2" } }), { status: 200 });
      }
      return new Response(JSON.stringify({ data: [], meta: { current_cursor: null, next_cursor: null } }), { status: 200 });
    });
    const client = new EventClient({ api: { request } });
    client.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    socket.message({ type: "gap", after: null, reason: "retention" });
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(3));
    expect(signals[1]).toBe(signals[2]);
    expect(signals[0]).not.toBe(signals[1]);
    client.close();
    vi.unstubAllGlobals();
  });
});
