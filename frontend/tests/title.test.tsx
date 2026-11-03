import { render } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { APP_TITLE, useWorkTitle, workTitle } from "../src/lib/title";

describe("workTitle", () => {
  it("names what the manager is doing, so a tab left in the background tells you", () => {
    expect(workTitle({ running: 0, queued: 0 })).toBe(APP_TITLE);
    expect(workTitle({ running: 1, queued: 0 })).toBe(`1 running · ${APP_TITLE}`);
    expect(workTitle({ running: 2, queued: 3 })).toBe(`2 running, 3 waiting · ${APP_TITLE}`);
    expect(workTitle({ running: 0, queued: 3 })).toBe(`3 waiting · ${APP_TITLE}`);
  });
});

describe("useWorkTitle", () => {
  let work = { running: 1, queued: 2 };
  const api = { request: vi.fn(async () => new Response(JSON.stringify({ data: { files: {}, scans: {}, jobs: { running: work.running, queued: work.queued } } }), { status: 200 })) };
  const setHidden = (hidden: boolean) => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
    document.dispatchEvent(new Event("visibilitychange"));
  };
  const Probe = () => { useWorkTitle(api); return null; };

  beforeEach(() => { vi.useFakeTimers(); document.title = APP_TITLE; work = { running: 1, queued: 2 }; setHidden(false); });
  afterEach(() => { vi.useRealTimers(); setHidden(false); });

  it("puts the work in the title and takes it out again", async () => {
    render(() => <Probe />);
    await vi.advanceTimersByTimeAsync(0);
    expect(document.title).toBe(`1 running, 2 waiting · ${APP_TITLE}`);

    work = { running: 0, queued: 0 };
    await vi.advanceTimersByTimeAsync(5100);
    expect(document.title).toBe(APP_TITLE);
  });

  it("says Done when the work ended while nobody was looking, until the tab is seen again", async () => {
    render(() => <Probe />);
    await vi.advanceTimersByTimeAsync(0);
    setHidden(true);

    work = { running: 0, queued: 0 };
    await vi.advanceTimersByTimeAsync(5100);
    expect(document.title).toBe(`Done · ${APP_TITLE}`);

    setHidden(false);
    expect(document.title).toBe(APP_TITLE);
  });

  it("leaves the plain title alone when nothing ever ran", async () => {
    work = { running: 0, queued: 0 };
    render(() => <Probe />);
    await vi.advanceTimersByTimeAsync(0);
    setHidden(true);
    await vi.advanceTimersByTimeAsync(11000);

    expect(document.title).toBe(APP_TITLE);
  });

  it("restores the title when the page goes away", async () => {
    const view = render(() => <Probe />);
    await vi.advanceTimersByTimeAsync(0);

    view.unmount();

    expect(document.title).toBe(APP_TITLE);
  });
});
