import { createSignal } from "solid-js";

export type Work = { running: number; queued: number };

const [work, setWork] = createSignal<Work>({ running: 0, queued: 0 }, { equals: (a, b) => a.running === b.running && a.queued === b.queued });

/** What the worker has in hand right now, for anything that wants to show it without asking again. */
export const workState = work;
export const publishWork = (next: Work) => setWork({ running: next.running, queued: next.queued });
