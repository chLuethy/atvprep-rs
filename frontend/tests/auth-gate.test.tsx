import { fireEvent, render } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import { AuthGate } from "../src/features/auth/AuthGate";


describe("AuthGate", () => {
  it("restores a valid cookie-backed session on load", async () => {
    const api = { bootstrap: vi.fn(), restore: vi.fn().mockResolvedValue(undefined), logout: vi.fn() };
    const { findByRole } = render(() => <AuthGate api={api as never}><p>Protected manager</p></AuthGate>);

    expect(await findByRole("button", { name: "Lock" })).toBeInTheDocument();
    expect(api.restore).toHaveBeenCalledOnce();
  });

  it("unlocks only after a successful bootstrap and does not persist the master token", async () => {
    const api = { bootstrap: vi.fn().mockResolvedValue(undefined), restore: vi.fn().mockRejectedValue(new Error("no session")), logout: vi.fn().mockResolvedValue(undefined) };
    const { getByLabelText, getByRole, findByRole, queryByText } = render(() => (
      <AuthGate api={api as never}>
        <p>Protected manager</p>
      </AuthGate>
    ));

    expect(queryByText("Protected manager")).not.toBeInTheDocument();
    await fireEvent.input(getByLabelText("Master token"), { target: { value: "secret" } });
    await fireEvent.click(getByRole("button", { name: "Unlock" }));

    expect(api.bootstrap).toHaveBeenCalledWith("secret");
    expect(await findByRole("button", { name: "Lock" })).toBeInTheDocument();
    expect(queryByText("Protected manager")).toBeInTheDocument();
    await fireEvent.click(getByRole("button", { name: "Lock" }));
    expect(api.logout).toHaveBeenCalledOnce();
    await findByRole("heading", { name: "Unlock manager" });
    expect(queryByText("Protected manager")).not.toBeInTheDocument();
    expect(localStorage.length).toBe(0);
  });

  it("entsperrt bei zwei synchronen Submits nur einmal", async () => {
    let finish!: () => void;
    const api = { bootstrap: vi.fn(() => new Promise<void>((resolve) => { finish = resolve; })), restore: vi.fn().mockRejectedValue(new Error("no session")), logout: vi.fn() };
    const view = render(() => <AuthGate api={api}><p>Protected manager</p></AuthGate>);
    await fireEvent.input(view.getByLabelText("Master token"), { target: { value: "test-token" } });
    const form = view.getByLabelText("Master token").closest("form")!;
    fireEvent.submit(form);
    fireEvent.submit(form);
    expect(api.bootstrap).toHaveBeenCalledTimes(1);
    finish();
    expect(await view.findByRole("button", { name: "Lock" })).toBeInTheDocument();
  });

  it("shows one generic error for failed bootstrap", async () => {
    const api = { bootstrap: vi.fn().mockRejectedValue(new Error("secret detail")), restore: vi.fn().mockRejectedValue(new Error("no session")), logout: vi.fn() };
    const { getByLabelText, getByRole, findByRole, queryByText } = render(() => (
      <AuthGate api={api as never}>
        <p>Protected manager</p>
      </AuthGate>
    ));

    await fireEvent.input(getByLabelText("Master token"), { target: { value: "wrong" } });
    await fireEvent.click(getByRole("button", { name: "Unlock" }));

    expect(await findByRole("alert")).toHaveTextContent("Unable to unlock manager session.");
    expect(queryByText("secret detail")).not.toBeInTheDocument();
  });

  it("hands the lock control to a render-function child instead of drawing its own", async () => {
    const api = { bootstrap: vi.fn(), restore: vi.fn().mockResolvedValue(undefined), logout: vi.fn().mockResolvedValue(undefined) };
    const { findByRole, queryAllByRole, queryByText } = render(() => (
      <AuthGate api={api as never}>
        {(session) => <button type="button" onClick={() => session.lock()}>Lock from shell</button>}
      </AuthGate>
    ));

    const lock = await findByRole("button", { name: "Lock from shell" });
    expect(queryAllByRole("button", { name: "Lock" })).toHaveLength(0);
    await fireEvent.click(lock);
    expect(api.logout).toHaveBeenCalledOnce();
    expect(await findByRole("heading", { name: "Unlock manager" })).toBeInTheDocument();
    expect(queryByText("Lock from shell")).not.toBeInTheDocument();
  });
});
