export class SessionError extends Error {
  constructor() {
    super("session bootstrap failed");
    this.name = "SessionError";
  }
}

export class ManagerApi {
  private readonly baseUrl: string;
  private csrf: string | null = null;

  constructor(baseUrl = "/api/v1") {
    this.baseUrl = baseUrl.replace(/\/$/, "");
  }

  get csrfToken(): string | null {
    return this.csrf;
  }

  setCsrfToken(value: string): void {
    this.csrf = value;
  }

  async bootstrap(masterToken: string): Promise<void> {
    this.csrf = null;
    const response = await fetch(`${this.baseUrl}/auth/session`, {
      method: "POST",
      credentials: "include",
      headers: {
        Authorization: `Bearer ${masterToken}`,
        Accept: "application/json",
      },
    });
    if (!response.ok) throw new SessionError();
    const body = (await response.json()) as { csrf_token?: unknown };
    if (typeof body.csrf_token !== "string" || body.csrf_token.length === 0) {
      throw new SessionError();
    }
    this.csrf = body.csrf_token;
  }

  async restore(): Promise<void> {
    this.csrf = null;
    const response = await fetch(`${this.baseUrl}/auth/session`, {
      credentials: "include",
      cache: "no-store",
      headers: { Accept: "application/json" },
    });
    if (!response.ok) throw new SessionError();
    const body = (await response.json()) as { csrf_token?: unknown };
    if (typeof body.csrf_token !== "string" || body.csrf_token.length === 0) throw new SessionError();
    this.csrf = body.csrf_token;
  }

  async request(path: string, init: RequestInit = {}): Promise<Response> {
    const method = (init.method ?? "GET").toUpperCase();
    const headers = new Headers(init.headers);
    headers.set("Accept", "application/json");
    if (method !== "GET" && method !== "HEAD" && method !== "OPTIONS") {
      const apiPath = decodeURIComponent(`${this.baseUrl}${path.split("?", 1)[0]}`);
      // The bootstrap token expires after csrf_ttl_seconds; every mutation, logout included, gets its own.
      const csrf = await this.issueMutationCsrf(method, apiPath, csrfAction(method, apiPath));
      if (!csrf) throw new SessionError();
      headers.set("X-CSRF-Token", csrf);
      headers.set("Idempotency-Key", headers.get("Idempotency-Key") ?? crypto.randomUUID());
    }
    const response = await fetch(`${this.baseUrl}${path}`, {
      ...init,
      method,
      credentials: "include",
      headers,
    });
    if (response.status === 401) {
      this.csrf = null;
      throw new SessionError();
    }
    return response;
  }

  private async issueMutationCsrf(method: string, path: string, action: string): Promise<string> {
    const query = new URLSearchParams({ method, path, action });
    const response = await fetch(`${this.baseUrl}/auth/csrf?${query}`, {
      credentials: "include",
      headers: { Accept: "application/json" },
    });
    if (!response.ok) {
      this.csrf = null;
      throw new SessionError();
    }
    const body = (await response.json()) as { csrf_token?: unknown };
    if (typeof body.csrf_token !== "string" || body.csrf_token.length === 0) {
      this.csrf = null;
      throw new SessionError();
    }
    return body.csrf_token;
  }

  async logout(): Promise<void> {
    try {
      await this.request("/auth/session", { method: "DELETE" });
    } finally {
      this.csrf = null;
    }
  }
}

function csrfAction(method: string, path: string): string {
  if (method === "DELETE" && path === "/api/v1/auth/session") return "auth-session";
  if (method === "PATCH" && path === "/api/v1/settings") return "settings";
  if (method === "POST" && path === "/api/v1/jobs") return "create-job";
  if (method === "POST" && path.endsWith("/inspect")) return "inspect";
  if (method === "POST" && path === "/api/v1/scans") return "scan";
  if (method === "POST" && path === "/api/v1/jobs/clear") return "clear-jobs";
  if (method === "POST" && path === "/api/v1/files/dismiss-missing") return "dismiss-missing";
  if (method === "POST" && path === "/api/v1/schedules") return "create-schedule";
  if (method === "PATCH" && /^\/api\/v1\/schedules\/[^/]+$/.test(path)) return "update-schedule";
  if (method === "DELETE" && /^\/api\/v1\/schedules\/[^/]+$/.test(path)) return "delete-schedule";
  if (method === "POST" && /^\/api\/v1\/schedules\/[^/]+\/run$/.test(path)) return "run-schedule";
  if (method === "POST" && path.endsWith("/cancel")) return "cancel-job";
  if (method === "POST" && path.endsWith("/retry")) return "retry-job";
  return "";
}
