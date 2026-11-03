import { fireEvent, render } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Button } from "../src/ui/Button";
import { Kbd } from "../src/ui/Kbd";
import { ProgressBar } from "../src/ui/ProgressBar";
import { StatusChip } from "../src/ui/StatusChip";
import { applyTheme, readStoredTheme, ThemeSwitch } from "../src/ui/ThemeSwitch";

afterEach(() => {
  document.documentElement.removeAttribute("data-theme");
  vi.restoreAllMocks();
});

describe("StatusChip", () => {
  it("states the status in words and decorates it with a hidden icon, never colour alone", () => {
    const view = render(() => <StatusChip kind="warn" icon="tag">Fix needed</StatusChip>);

    const chip = view.getByText("Fix needed").closest(".chip");
    expect(chip).toHaveClass("chip", "warn");
    const icon = chip?.querySelector("svg");
    expect(icon).not.toBeNull();
    expect(icon).toHaveAttribute("aria-hidden", "true");
  });

  it("breathes only while it is live, and never lights up because of a change", () => {
    const [label, setLabel] = createSignal("Running");
    const view = render(() => <StatusChip kind="neutral" icon="activity" live>{label()}</StatusChip>);
    const chip = view.getByText("Running").closest(".chip") as HTMLElement;
    expect(chip).toHaveClass("live");

    setLabel("Done");

    expect(chip).not.toHaveClass("flash");
    const still = render(() => <StatusChip kind="neutral" icon="dot">Queued</StatusChip>);
    expect(still.getByText("Queued").closest(".chip")).not.toHaveClass("live");
  });

  it("supports every semantic kind", () => {
    for (const kind of ["ok", "warn", "bad", "neutral"] as const) {
      const view = render(() => <StatusChip kind={kind} icon="dot">{kind}</StatusChip>);
      expect(view.getByText(kind).closest(".chip")).toHaveClass(kind);
      view.unmount();
    }
  });
});

describe("Button", () => {
  it("renders a real button with a variant class and forwards clicks", async () => {
    const onClick = vi.fn();
    const view = render(() => <Button variant="primary" onClick={onClick}>Apply</Button>);

    const button = view.getByRole("button", { name: "Apply" });
    expect(button).toHaveAttribute("type", "button");
    expect(button).toHaveClass("btn", "primary");
    await fireEvent.click(button);
    expect(onClick).toHaveBeenCalledOnce();
  });

  it("does not fire when disabled", async () => {
    const onClick = vi.fn();
    const view = render(() => <Button disabled onClick={onClick}>Apply</Button>);

    await fireEvent.click(view.getByRole("button", { name: "Apply" }));
    expect(onClick).not.toHaveBeenCalled();
    expect(view.getByRole("button", { name: "Apply" })).toBeDisabled();
  });
});

describe("Kbd", () => {
  it("shows a shortcut hint that assistive tech skips", () => {
    const view = render(() => <Kbd>F</Kbd>);
    const hint = view.getByText("F");
    expect(hint.tagName).toBe("KBD");
    expect(hint).toHaveAttribute("aria-hidden", "true");
  });
});

describe("theme", () => {
  it("follows the system by leaving data-theme unset, and forces light or dark otherwise", () => {
    applyTheme("dark");
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    applyTheme("light");
    expect(document.documentElement).toHaveAttribute("data-theme", "light");
    applyTheme("system");
    expect(document.documentElement).not.toHaveAttribute("data-theme");
  });

  it("reads a stored choice and ignores junk or a throwing storage", () => {
    vi.spyOn(Storage.prototype, "getItem").mockReturnValueOnce("dark");
    expect(readStoredTheme()).toBe("dark");
    vi.spyOn(Storage.prototype, "getItem").mockReturnValueOnce("neon");
    expect(readStoredTheme()).toBe("system");
    vi.spyOn(Storage.prototype, "getItem").mockImplementationOnce(() => { throw new Error("blocked"); });
    expect(readStoredTheme()).toBe("system");
  });

  it("switches the theme from a labelled group and survives a throwing storage write", async () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    const view = render(() => <ThemeSwitch />);

    expect(view.getByRole("group", { name: "Appearance" })).toBeInTheDocument();
    expect(view.getByRole("button", { name: "System" })).toHaveAttribute("aria-pressed", "true");
    await fireEvent.click(view.getByRole("button", { name: "Dark" }));

    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    expect(view.getByRole("button", { name: "Dark" })).toHaveAttribute("aria-pressed", "true");
    expect(view.getByRole("button", { name: "System" })).toHaveAttribute("aria-pressed", "false");
  });
});

describe("ProgressBar", () => {
  it("is a progressbar that carries its value and name, and fills by transform rather than width", () => {
    const [value, setValue] = createSignal(40);
    const view = render(() => <ProgressBar value={value()} label="Job progress" />);

    const bar = view.getByRole("progressbar", { name: "Job progress" });
    expect(bar).toHaveAttribute("aria-valuenow", "40");
    expect(bar).toHaveAttribute("aria-valuemin", "0");
    expect(bar).toHaveAttribute("aria-valuemax", "100");
    const fill = bar.querySelector(".bar-fill") as HTMLElement;
    expect(fill.getAttribute("style")).toContain("scaleX(0.4)");
    expect(fill.getAttribute("style")).not.toMatch(/width/);

    setValue(75);
    expect(bar).toHaveAttribute("aria-valuenow", "75");
    expect(fill.getAttribute("style")).toContain("scaleX(0.75)");
  });

  it("carries on from where the same tracked bar stood when it is built anew", () => {
    const first = render(() => <ProgressBar value={30} track="job:1" label="first" />);
    first.unmount();

    const view = render(() => <ProgressBar value={60} track="job:1" label="again" />);

    const fill = view.getByRole("progressbar").querySelector(".bar-fill") as HTMLElement;
    expect(fill.style.transform).toBe("scaleX(0.6)"); // ends at the new value, having started from 0.3 with the next frame
  });

  it("keeps the fill inside the bar whatever it is given", () => {
    const view = render(() => <><ProgressBar value={-20} label="low" /><ProgressBar value={140} label="high" /></>);

    expect(view.getByRole("progressbar", { name: "low" })).toHaveAttribute("aria-valuenow", "0");
    expect(view.getByRole("progressbar", { name: "high" })).toHaveAttribute("aria-valuenow", "100");
  });
});
