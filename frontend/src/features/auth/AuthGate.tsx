import { createSignal, onMount, Show, type JSX } from "solid-js";
import type { ManagerApi } from "../../lib/api";
import { t } from "../../lib/i18n";
import { Button, Icon, Logo } from "../../ui";

export type AuthGateApi = Pick<ManagerApi, "bootstrap" | "logout" | "restore">;

/** What the shell needs from the session: lock it, and know when that is in flight. */
export type SessionControls = { lock: () => void; busy: () => boolean };

interface AuthGateProps {
  api: AuthGateApi;
  /** A function child receives the session controls and places Lock itself; plain content gets a default Lock button. */
  children: JSX.Element | ((session: SessionControls) => JSX.Element);
}

export function AuthGate(props: AuthGateProps) {
  const [unlocked, setUnlocked] = createSignal(false);
  const [masterToken, setMasterToken] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);

  onMount(() => {
    void props.api.restore().then(() => setUnlocked(true)).catch(() => undefined);
  });

  const lock = async () => {
    setBusy(true);
    try {
      await props.api.logout();
    } catch {
      // Lock locally even when the network/session revocation request fails.
    } finally {
      setUnlocked(false);
      setBusy(false);
    }
  };

  const unlock = async (event: SubmitEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    const candidate = masterToken();
    setMasterToken("");
    try {
      await props.api.bootstrap(candidate);
      setUnlocked(true);
    } catch {
      setError(t("Unable to unlock manager session."));
    } finally {
      setBusy(false);
    }
  };

  const session: SessionControls = { lock: () => void lock(), busy };

  return (
    <Show
      when={unlocked()}
      fallback={
        <main class="unlock" aria-labelledby="unlock-title">
          <form class="unlock-card" onSubmit={unlock}>
            <Logo height={30} />
            <h1 id="unlock-title">{t("Unlock manager")}</h1>
            <label for="master-token">{t("Master token")}</label>
            <input
              id="master-token"
              name="master-token"
              class="input"
              type="password"
              autocomplete="off"
              required
              value={masterToken()}
              onInput={(event) => setMasterToken(event.currentTarget.value)}
            />
            <Button variant="primary" type="submit" disabled={busy()}>
              {t("Unlock")}
            </Button>
            <Show when={error()}>
              <p class="notice bad" role="alert">{error()}</p>
            </Show>
          </form>
        </main>
      }
    >
      {(() => {
        const content = props.children;
        return typeof content === "function" ? (
          content(session)
        ) : (
          <div>
            <Button variant="quiet" disabled={busy()} onClick={() => void lock()}>
              <Icon name="lock" /> {t("Lock")}
            </Button>
            {content}
          </div>
        );
      })()}
    </Show>
  );
}
