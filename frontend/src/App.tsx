import { A, Navigate, Router, Route, useLocation, useNavigate } from "@solidjs/router";
import { createEffect, createMemo, createSignal, onCleanup, onMount, Show, type ParentComponent } from "solid-js";
import "./styles/app.css";
import type { ManagerApi } from "./lib/api";
import type { SessionControls } from "./features/auth/AuthGate";
import { ActivityPage } from "./features/activity/ActivityPage";
import { LibraryPage } from "./features/files/LibraryPage";
import { fileFilterQuery, fileSortQuery, parseFileFilter, parseFileSort, QUICK_FILTERS, sameFilter, type FileFilter, type FileSort } from "./features/files/filters";
import { JOB_QUICK_FILTERS, jobFilterQuery, parseJobFilter, sameJobFilter } from "./features/activity/filters";
import { OverviewPage } from "./features/overview/OverviewPage";
import { SchedulesPage } from "./features/schedules/SchedulesPage";
import { SettingsPage } from "./features/settings/SettingsPage";
import { ShortcutsPanel } from "./features/help/ShortcutsPanel";
import { EventClient } from "./lib/events";
import { N, t, tn } from "./lib/i18n";
import { isTyping } from "./lib/keys";
import { realtimeState } from "./lib/realtime";
import { useWorkTitle } from "./lib/title";
import { recall, remember, type ViewPage } from "./lib/viewMemory";
import { workState } from "./lib/work";
import { dismissTip, tipPending } from "./lib/tips";
import { Button, chooseTheme, CommandPalette, Icon, Kbd, LanguageSwitch, Logo, ThemeSwitch, type Command, type IconName, type Theme } from "./ui";

const Dashboard: ParentComponent = () => <section>{t("Overview")}</section>;
const Files: ParentComponent = () => <section>{t("Files")}</section>;
const Jobs: ParentComponent = () => <section>{t("Activity")}</section>;
const Schedules: ParentComponent = () => <section>{t("Schedules")}</section>;
const Settings: ParentComponent = () => <section>{t("Settings")}</section>;

/**
 * Marks an address the page itself chose, so a bare one there means "show everything" (Reset filters, a cleared
 * quick filter, Back to such an entry). A bare address without the mark came from outside (navigation, the palette,
 * a link, a reload) and takes the last remembered state instead, by replacing the address so no history entry is
 * added. A query of its own always wins.
 */
const OWN = { own: true } as const;
const isOwn = (state: unknown) => typeof state === "object" && state !== null && (state as { own?: unknown }).own === true;

/**
 * Keeps the address of a page and its view memory in step: a bare address from outside takes the remembered
 * state, every other one is rewritten to its `canonical` query (`at` makes the address of a query) and
 * remembered, an empty one forgetting the state.
 */
const useRememberedAddress = (page: ViewPage, at: (query: string) => string, canonical: () => string) => {
  const navigate = useNavigate();
  const location = useLocation();
  createEffect(() => {
    const search = location.search.replace(/^\?/, "");
    if (search === "" && !isOwn(location.state)) {
      return navigate(at(recall(page) ?? ""), { replace: true, state: OWN });
    }
    const query = canonical();
    remember(page, query);
    if (search !== query) navigate(at(query), { replace: true, state: OWN });
  });
};

/**
 * One route for the list and the open file, so choosing a file never remounts the list. The filter lives in
 * the address: a chosen state or property is a new history entry (Back undoes it), a search replaces the entry,
 * and an old or untidy address is rewritten to the canonical one. The order follows the filter (`sort`, `dir`);
 * a new order is a history entry too.
 */
const Library: ParentComponent<{ api: ManagerApi | undefined; fileId: string | undefined }> = (props) => {
  const navigate = useNavigate();
  const location = useLocation();
  const base = (id: string | undefined) => (id ? `/files/${encodeURIComponent(id)}` : "/files");
  const query = (filter: FileFilter, sort: FileSort) => [fileFilterQuery(filter), fileSortQuery(sort)].filter(Boolean).join("&");
  const at = (id: string | undefined, search: string) => `${base(id)}${search ? `?${search}` : ""}`;
  const params = () => new URLSearchParams(location.search);
  const filter = createMemo(() => parseFileFilter(params()), undefined, { equals: sameFilter });
  const sort = createMemo(() => parseFileSort(params()), undefined, { equals: (a, b) => fileSortQuery(a) === fileSortQuery(b) });
  useRememberedAddress("files", (search) => at(props.fileId, search), () => query(filter(), sort()));
  return (
    <Show when={props.api} fallback={<Files />}>
      {(api) => (
        <LibraryPage
          api={api()}
          activeId={props.fileId}
          filter={filter()}
          sort={sort()}
          onFilter={(next, how) => navigate(at(props.fileId, query(next, sort())), { replace: how.replace, state: OWN })}
          onSort={(next) => navigate(at(props.fileId, query(filter(), next)), { state: OWN })}
          onOpen={(id) => navigate(`${base(id)}${location.search}`, { state: OWN })}
        />
      )}
    </Show>
  );
};

/**
 * Same shape as the files page: one route for the list and the open job, and the filter in the address. A chosen
 * value or order is a new history entry, a search replaces the entry, and an untidy address is rewritten.
 */
const Activity: ParentComponent<{ api: ManagerApi | undefined; jobId: string | undefined }> = (props) => {
  const navigate = useNavigate();
  const location = useLocation();
  const base = (id: string | undefined) => (id ? `/jobs/${encodeURIComponent(id)}` : "/jobs");
  const at = (id: string | undefined, query: string) => `${base(id)}${query ? `?${query}` : ""}`;
  const filter = createMemo(() => parseJobFilter(new URLSearchParams(location.search)), undefined, { equals: sameJobFilter });
  useRememberedAddress("jobs", (query) => at(props.jobId, query), () => jobFilterQuery(filter()));
  return (
    <Show when={props.api} fallback={<Jobs />}>
      {(api) => (
        <ActivityPage
          api={api()}
          activeId={props.jobId}
          filter={filter()}
          onFilter={(next, how) => navigate(at(props.jobId, jobFilterQuery(next)), { replace: how.replace, state: OWN })}
          onOpen={(id) => navigate(`${base(id)}${location.search}`, { state: OWN })}
        />
      )}
    </Show>
  );
};

const Overview: ParentComponent<{ api: ManagerApi | undefined }> = (props) => {
  const navigate = useNavigate();
  return (
    <Show when={props.api} fallback={<Dashboard />}>
      {(api) => <OverviewPage api={api()} onNavigate={(href) => navigate(href)} />}
    </Show>
  );
};

const NAV: ReadonlyArray<{ href: string; label: string; icon: IconName }> = [
  { href: "/files", label: N("Files"), icon: "library" },
  { href: "/jobs", label: N("Activity"), icon: "activity" },
  { href: "/schedules", label: N("Schedules"), icon: "clock" },
  { href: "/dashboard", label: N("Overview"), icon: "overview" },
  { href: "/settings", label: N("Settings"), icon: "settings" },
];

const STATUS_COMMANDS: ReadonlyArray<{ status: string; label: string }> = [
  { status: "", label: N("All files") },
  { status: "clean", label: N("Ready") },
  { status: "warning", label: N("Warning") },
  { status: "blocker", label: N("Blocker") },
  { status: "unknown", label: N("Not verified") },
  { status: "probe_failed", label: N("Inspection failed") },
];
const THEMES: ReadonlyArray<{ value: Theme; label: string }> = [
  { value: "system", label: N("System") },
  { value: "light", label: N("Light") },
  { value: "dark", label: N("Dark") },
];

const Shell: ParentComponent<{ session?: SessionControls; api?: ManagerApi }> = (props) => {
  if (props.api) useWorkTitle(props.api);
  const [navOpen, setNavOpen] = createSignal(false);
  const [paletteOpen, setPaletteOpen] = createSignal(false);
  const [helpOpen, setHelpOpen] = createSignal(false);
  const [tip, setTip] = createSignal(tipPending("keys"));
  const [version, setVersion] = createSignal<string | null>(null);
  const navigate = useNavigate();
  let helpPanel: HTMLElement | undefined;
  let helpOpener: Element | null = null;

  const learned = () => {
    dismissTip("keys");
    setTip(false);
  };
  const openHelp = () => {
    helpOpener = document.activeElement;
    setHelpOpen(true);
    learned();
    queueMicrotask(() => helpPanel?.focus());
  };
  const closeHelp = () => {
    setHelpOpen(false);
    if (helpOpener instanceof HTMLElement) helpOpener.focus();
  };
  const toggleHelp = () => (helpOpen() ? closeHelp() : openHelp());

  const commands = (): Command[] => [
    ...NAV.map((item) => ({ id: `go-${item.label.toLowerCase()}`, label: t("Go to {page}", { page: t(item.label) }), hint: t("Page"), run: () => navigate(item.href) })),
    ...STATUS_COMMANDS.map((entry) => ({
      id: `files-${entry.status || "all"}`,
      label: t("Files: show {state}", { state: t(entry.label) }),
      hint: t("Filter"),
      // "All files" asks for everything, so it carries the page's own mark and forgets the remembered state.
      run: () => (entry.status ? navigate(`/files?status=${entry.status}`) : navigate("/files", { state: OWN })),
    })),
    ...QUICK_FILTERS.map((entry) => ({
      id: `files-quick-${entry.id}`,
      label: t("Files: {quick}", { quick: t(entry.label) }),
      hint: t("Quick filter"),
      run: () => navigate(`/files?${fileFilterQuery(entry.filter)}`),
    })),
    ...JOB_QUICK_FILTERS.map((entry) => ({
      id: `jobs-quick-${entry.id}`,
      label: t("Activity: {quick}", { quick: t(entry.label) }),
      hint: t("Quick filter"),
      run: () => navigate(`/jobs?${jobFilterQuery(entry.filter)}`),
    })),
    ...THEMES.map((theme) => ({ id: `theme-${theme.value}`, label: t("Theme: {theme}", { theme: t(theme.label) }), hint: t("Appearance"), run: () => chooseTheme(theme.value) })),
    { id: "help-shortcuts", label: t("Show keyboard shortcuts"), hint: t("Help"), run: openHelp },
    ...(props.session ? [{ id: "lock", label: t("Lock manager"), hint: t("Session"), run: () => props.session?.lock() }] : []),
  ];

  const onKeyDown = (event: KeyboardEvent) => {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      learned();
      setPaletteOpen((open) => !open);
      return;
    }
    if (event.key === "?" && !event.metaKey && !event.ctrlKey && !event.altKey && !isTyping(event.target)) {
      event.preventDefault();
      toggleHelp();
    }
  };
  onMount(() => {
    document.addEventListener("keydown", onKeyDown);
    // Which release is running, for the person who just updated; a failure just leaves the line out.
    void props.api?.request("/version").then(async (response) => {
      const body = response.ok ? ((await response.json()) as { version?: unknown }) : null;
      if (typeof body?.version === "string") setVersion(body.version);
    }).catch(() => undefined);
  });
  onCleanup(() => document.removeEventListener("keydown", onKeyDown));

  return (
    <div class="shell">
      <a class="sr-skip" href="#main-content">{t("Skip to content")}</a>
      <aside class="sidebar">
        <div class="brand"><Logo height={26} busy={workState().running > 0} /><span class="sr-only">atvprep-manager</span></div>
        <button
          type="button"
          class="btn quiet menu-toggle"
          aria-controls="main-navigation"
          aria-expanded={navOpen()}
          aria-label={navOpen() ? t("Close navigation") : t("Open navigation")}
          onClick={() => setNavOpen((value) => !value)}
        >
          <Icon name="menu" size={16} /> {navOpen() ? t("Close") : t("Menu")}
        </button>
        <nav id="main-navigation" aria-label={t("Main")} class={navOpen() ? "nav open" : "nav"}>
          {NAV.map((item) => (
            <A href={item.href} class="nav-link" activeClass="current" inactiveClass="" onClick={() => setNavOpen(false)}>
              <Icon name={item.icon} size={16} />
              <span>{t(item.label)}</span>
              <Show when={item.href === "/jobs" && workState().running + workState().queued > 0}>
                <span
                  class="nav-count num"
                  classList={{ running: workState().running > 0 }}
                  role="img"
                  aria-label={workState().running > 0
                    ? tn("{count} job running", "{count} jobs running", workState().running)
                    : tn("{count} job waiting", "{count} jobs waiting", workState().queued)}
                >{workState().running > 0 ? workState().running : workState().queued}</span>
              </Show>
            </A>
          ))}
        </nav>
        <div class="sidebar-foot">
          <LanguageSwitch />
          <ThemeSwitch />
          <Button variant="quiet" aria-label={t("Keyboard shortcuts")} onClick={toggleHelp}>
            <Icon name="keyboard" /> {t("Shortcuts")} <Kbd>?</Kbd>
          </Button>
          <Show when={props.session}>
            {(session) => (
              <Button variant="quiet" disabled={session().busy()} onClick={() => session().lock()}>
                <Icon name="lock" /> {t("Lock")}
              </Button>
            )}
          </Show>
          <Show when={version()}>{(value) => <p class="app-version">{t("Version {version}", { version: value() })}</p>}</Show>
        </div>
      </aside>
      <main id="main-content" class="content" tabindex="-1">
        <Show when={tip()}>
          <div class="tip" role="note" aria-label={t("Keyboard tip")}>
            <p>{t("Press {help} for the keyboard shortcuts, {palette} for commands.", { help: "\u0001", palette: "\u0002" }).split(/(\u0001|\u0002)/).map((part) => part === "\u0001" ? <Kbd>?</Kbd> : part === "\u0002" ? <><Kbd>⌘</Kbd> <Kbd>K</Kbd></> : part)}</p>
            <Button variant="quiet" onClick={learned}>{t("Got it")}</Button>
          </div>
        </Show>
        {props.children}
      </main>
      <Show when={helpOpen()}>
        <section
          class="help-panel"
          role="region"
          aria-label={t("Keyboard shortcuts")}
          tabindex="-1"
          ref={(element) => (helpPanel = element)}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.stopPropagation();
              closeHelp();
            }
          }}
        >
          <header>
            <h2>{t("Keyboard shortcuts")}</h2>
            <Button variant="quiet" aria-label={t("Close shortcuts")} onClick={closeHelp}><Icon name="x" /></Button>
          </header>
          <ShortcutsPanel />
        </section>
      </Show>
      <CommandPalette open={paletteOpen()} commands={commands()} onClose={() => setPaletteOpen(false)} />
    </div>
  );
};

const App: ParentComponent<{ api?: ManagerApi; session?: SessionControls }> = (props) => {
  const api = props.api;
  onMount(() => {
    if (!api) return;
    const events = new EventClient({
      api,
      onEvent: realtimeState.applyEvent,
      onLogTail: realtimeState.applyLogTail,
      onResync: realtimeState.applySnapshot,
    });
    events.connect();
    onCleanup(() => events.close());
  });
  return (
    <Router root={(routed) => <Shell session={props.session} api={api}>{routed.children}</Shell>}>
      <Route path="/dashboard" component={() => <Overview api={api} />} />
      <Route path="/files/:fileId?" component={(route) => <Library api={api} fileId={route.params.fileId} />} />
      <Route path="/jobs/:jobId?" component={(route) => <Activity api={api} jobId={route.params.jobId} />} />
      <Route path="/schedules" component={() => (api ? <SchedulesPage api={api} /> : <Schedules />)} />
      <Route path="/settings" component={() => (api ? <SettingsPage api={api} /> : <Settings />)} />
      <Route path="/" component={() => <Navigate href="/files" />} />
    </Router>
  );
};

export default App;
