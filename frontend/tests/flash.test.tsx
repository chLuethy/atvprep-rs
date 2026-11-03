import { createRoot, createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { FLASH_MS, flash, flashOnChange } from "../src/lib/flash";

afterEach(() => { vi.useRealTimers(); });

describe("flash", () => {
  it("marks an element for a moment and then lets go of it", () => {
    vi.useFakeTimers();
    const element = document.createElement("div");

    flash(element);
    expect(element).toHaveClass("flash");
    vi.advanceTimersByTime(FLASH_MS - 1);
    expect(element).toHaveClass("flash");
    vi.advanceTimersByTime(2);
    expect(element).not.toHaveClass("flash");
  });

  it("starts over when it is flashed again while still lit", () => {
    vi.useFakeTimers();
    const element = document.createElement("div");

    flash(element);
    vi.advanceTimersByTime(FLASH_MS - 100);
    flash(element);
    vi.advanceTimersByTime(FLASH_MS - 100);

    expect(element).toHaveClass("flash");
  });
});

describe("flashOnChange", () => {
  it("stays quiet when the element appears and flashes only when the watched value changes", () => {
    vi.useFakeTimers();
    const element = document.createElement("div");
    const [value, setValue] = createSignal("warning");
    createRoot(() => flashOnChange(value)(element));

    expect(element).not.toHaveClass("flash");
    setValue("warning");
    expect(element).not.toHaveClass("flash");
    setValue("ready");
    expect(element).toHaveClass("flash");
  });
});
