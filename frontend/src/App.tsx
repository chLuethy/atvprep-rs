import { createRouter, useLocation, useNavigate, type RouteProps } from "@solidjs/router";
import { createContext, createEffect, createMemo, createSignal, onSettled, Show, useContext, type ParentComponent } from "solid-js";
import "./styles/app.css";
import type { ManagerApi } from "./lib/api";
import type { SessionControls } from "./features/auth/AuthGate";
import { ActivityPage } from "./features/activity/ActivityPage";
import { LibraryPage } from "./features/files/LibraryPage";
import { fileFilterQuery, fileSortQuery, parseFileFilter, parseFileSort, QUICK_FILTERS, sameFilter, type FileFilter, type FileSort } from "./features/files/filters";
import { JOB_QUICK_FILTERS, jobFilterQuery, parseJobFilter, sameJobFilter, type JobFilter } from "./features/activity/filters";
import { parseScheduleView, sameScheduleView, scheduleViewQuery, type ScheduleView } from "./features/schedules/filters";
import { OverviewPage } from "./features/overview/OverviewPage";
import { SchedulesPage } from "./features/schedules/SchedulesPage";
import { SettingsPage } from "./features/settings/SettingsPage";
import { ShortcutsPanel } from "./features/help/ShortcutsPanel";
import { EventClient } from "./lib/events";
import { N, t, tn } from "./lib/i18n";
import { isTyping } from "./lib/keys";
import { realtimeState } from "./lib/realtime";
import { searchTargets } from "./features/search/paletteSearch";
import { setRouteTitle, useWorkTitle } from "./lib/title";
import { OWN, useFilterAddress } from "./lib/facetFilter";
import { workState } from "./lib/work";
import { dismissTip, tipPending } from "./lib/tips";
import { Button, chooseTheme, CommandPalette, Icon, Kbd, LanguageSwitch, Logo, ThemeSwitch, type Command, type IconName, type Theme } from "./ui";

const Dashboard: ParentComponent = () => <section>{t("Overview")}</section>;
const Files: ParentComponent = () => <section>{t("Files")}</section>;
const Jobs: ParentComponent = () => <section>{t("Activity")}</section>;
const Schedules: ParentComponent = () => <section>{t("Schedules")}</section>;
const Settings: ParentComponent = () => <section>{t("Settings")}</section>;

/**
 * One route for the list and the open file, so choosing a file never remounts the list. The filter lives in
 * the address (`useFilterAddress`): a chosen state or property is a new history entry (Back undoes it), a search
 * replaces the entry, and an old or untidy address is rewritten to the canonical one. The order follows the
 * filter (`sort`, `dir`); a new order is a history entry too.
 */
const Library: ParentComponent<{ api: ManagerApi | undefined; fileId: string | undefined }> = (props) => {
  const base = (id: string | undefined) => (id ? `/files/${encodeURIComponent(id)}` : "/files");
  const address = useFilterAddress<{ filter: FileFilter; sort: FileSort }>({
    page: "files",
    path: () => base(props.fileId),
    parse: (params) => ({ filter: parseFileFilter(params), sort: parseFileSort(params) }),
    query: ({ filter, sort }) => [fileFilterQuery(filter), fileSortQuery(sort)].filter(Boolean).join("&"),
    same: (a, b) => sameFilter(a.filter, b.filter) && fileSortQuery(a.sort) === fileSortQuery(b.sort),
  });
  const filter = createMemo(() => address.state().filter, { equals: sameFilter });
  const sort = createMemo(() => address.state().sort, { equals: (a, b) => fileSortQuery(a) === fileSortQuery(b) });
  return (
    <Show when={props.api} fallback={<Files />}>
      {(api) => (
        <LibraryPage
          api={api()}
          activeId={props.fileId}
          filter={filter()}
          sort={sort()}
          onFilter={(next, how) => address.go({ filter: next, sort: sort() }, how)}
          onSort={(next) => address.go({ filter: filter(), sort: next })}
          onOpen={(id) => address.open(base(id))}
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
  const base = (id: string | undefined) => (id ? `/jobs/${encodeURIComponent(id)}` : "/jobs");
  const address = useFilterAddress<JobFilter>({
    page: "jobs",
    path: () => base(props.jobId),
    parse: parseJobFilter,
    query: jobFilterQuery,
    same: sameJobFilter,
  });
  return (
    <Show when={props.api} fallback={<Jobs />}>
      {(api) => (
        <ActivityPage
          api={api()}
          activeId={props.jobId}
          filter={address.state()}
          onFilter={(next, how) => address.go(next, how)}
          onOpen={(id) => address.open(base(id))}
        />
      )}
    </Show>
  );
};

/** The schedule list keeps its filter and order in the address and the view memory like Files and Activity. */
const SchedulesRoute: ParentComponent<{ api: ManagerApi | undefined }> = (props) => {
  const address = useFilterAddress<ScheduleView>({
    page: "schedules",
    path: () => "/schedules",
    parse: parseScheduleView,
    query: scheduleViewQuery,
    same: sameScheduleView,
  });
  return (
    <Show when={props.api} fallback={<Schedules />}>
      {(api) => <SchedulesPage api={api()} filter={address.state()} onFilter={(next, how) => address.go(next, how)} />}
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
  const location = useLocation();
  const isCurrent = (href: string) => location.pathname === href || location.pathname.startsWith(`${href}/`);
  // The name is worded in the tracked part, so a language change words the title again.
  createEffect(() => {
    const item = NAV.find((entry) => isCurrent(entry.href));
    return item ? t(item.label) : "";
  }, (name) => setRouteTitle(name));
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
  onSettled(() => {
    document.addEventListener("keydown", onKeyDown);
    // Which release is running, for the person who just updated; a failure just leaves the line out.
    void props.api?.request("/version").then(async (response) => {
      const body = response.ok ? ((await response.json()) as { version?: unknown }) : null;
      if (typeof body?.version === "string") setVersion(body.version);
    }).catch(() => undefined);
    return () => document.removeEventListener("keydown", onKeyDown);
  });

  return (
    <div class="shell">
      <a class="sr-skip" href="#main-content">{t("Skip to content")}</a>
      <aside class="sidebar">
        <div class="brand"><Logo height={26} busy={workState().running > 0} /><span class="sr-only">atvprep-manager</span></div>
        <button
          type="button"
          class="btn quiet menu-toggle"
          aria-controls="main-navigation"
          aria-expanded={navOpen() ? "true" : "false"}
          onClick={() => setNavOpen((value) => !value)}
        >
          <Icon name="menu" size={16} /> {navOpen() ? t("Close") : t("Menu")}
        </button>
        <nav id="main-navigation" aria-label={t("Main")} class={navOpen() ? "nav open" : "nav"}>
          {NAV.map((item) => (
            <a href={item.href} class={["nav-link", { current: isCurrent(item.href) }]} aria-current={isCurrent(item.href) ? "page" : undefined} onClick={() => setNavOpen(false)}>
              <Icon name={item.icon} size={16} />
              <span>{t(item.label)}</span>
              <Show when={item.href === "/jobs" && workState().running + workState().queued > 0}>
                <span
                  class={["nav-count num", { running: workState().running > 0 }]}
                  role="img"
                  aria-label={workState().running > 0
                    ? tn("{count} job running", "{count} jobs running", workState().running)
                    : tn("{count} job waiting", "{count} jobs waiting", workState().queued)}
                >{workState().running > 0 ? workState().running : workState().queued}</span>
              </Show>
            </a>
          ))}
        </nav>
        <div class="sidebar-foot">
          <div class="foot-row">
            <Show when={props.session}>
              {(session) => (
                <Button variant="quiet" disabled={session().busy()} onClick={() => session().lock()}>
                  <Icon name="lock" /> {t("Lock")}
                </Button>
              )}
            </Show>
            <Button variant="quiet" aria-label={t("Keyboard shortcuts")} onClick={toggleHelp}>
              <Icon name="keyboard" /> {t("Shortcuts")} <Kbd>?</Kbd>
            </Button>
          </div>
          <div class="foot-appearance">
            <span class="label">{t("Appearance")}</span>
            <ThemeSwitch />
          </div>
          <LanguageSwitch />
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
      <CommandPalette open={paletteOpen()} commands={commands()} {...(props.api ? { search: (query: string) => searchTargets(props.api!, query, (path) => navigate(path)) } : {})} onClose={() => setPaletteOpen(false)} />
    </div>
  );
};

type AppProps = { api?: ManagerApi; session?: SessionControls };
const AppContext = createContext<AppProps>();
const AppRouter = createRouter({
  routes: [
    { path: "/dashboard", component: () => <Overview api={useContext(AppContext).api} /> },
    { path: "/files/:fileId?", component: (route: RouteProps<"/files/:fileId?">) => <Library api={useContext(AppContext).api} fileId={route.params.fileId} /> },
    { path: "/jobs/:jobId?", component: (route: RouteProps<"/jobs/:jobId?">) => <Activity api={useContext(AppContext).api} jobId={route.params.jobId} /> },
    { path: "/schedules", component: () => <SchedulesRoute api={useContext(AppContext).api} /> },
    { path: "/settings", component: () => <Show when={useContext(AppContext).api} fallback={<Settings />}>{(api) => <SettingsPage api={api()} />}</Show> },
    { path: "/", component: () => {
      const navigate = useNavigate();
      onSettled(() => { navigate("/files", { replace: true }); });
      return null;
    } },
  ],
});
const App: ParentComponent<AppProps> = (props) => {
  onSettled(() => {
    const api = props.api;
    if (!api) return;
    const events = new EventClient({
      api,
      onEvent: realtimeState.applyEvent,
      onLogTail: realtimeState.applyLogTail,
      onResync: realtimeState.applySnapshot,
    });
    events.connect();
    return () => events.close();
  });
  return (
    <AppContext value={props}>
      <AppRouter>{(routed) => <Shell session={props.session} api={props.api}>{routed.children}</Shell>}</AppRouter>
    </AppContext>
  );
};

export default App;
