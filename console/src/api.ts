import {
  AdminTokenRecord,
  ApiError,
  ApiErrorBody,
  BudgetSnapshot,
  BudgetSpec,
  MargKey,
  PersistedRoute,
  PolicyView,
  ProviderHealth,
  RequestLogEntry,
  RouteSpec,
} from "./types";

const TOKEN_KEY = "marg.adminToken";
const API_BASE_KEY = "marg.apiBase";

export function getToken(): string | null {
  return sessionStorage.getItem(TOKEN_KEY);
}

export function setToken(token: string): void {
  sessionStorage.setItem(TOKEN_KEY, token);
}

export function clearToken(): void {
  sessionStorage.removeItem(TOKEN_KEY);
}

export function getApiBase(): string {
  const fromSession = sessionStorage.getItem(API_BASE_KEY);
  if (fromSession) return fromSession;
  const params = new URLSearchParams(window.location.search);
  const fromQuery = params.get("api");
  if (fromQuery) {
    sessionStorage.setItem(API_BASE_KEY, fromQuery);
    return fromQuery;
  }
  return window.location.origin;
}

async function call<T>(
  path: string,
  init: RequestInit & { signal?: AbortSignal } = {},
  parseJson = true,
): Promise<T> {
  const base = getApiBase();
  const token = getToken();
  const headers = new Headers(init.headers ?? {});
  if (token) headers.set("Authorization", `Bearer ${token}`);
  if (init.body && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }
  let resp: Response;
  try {
    resp = await fetch(`${base}${path}`, { ...init, headers });
  } catch (e) {
    throw new ApiError(0, "network_error", (e as Error).message);
  }
  if (!resp.ok) {
    let code = "unknown";
    let message = resp.statusText;
    try {
      const body = (await resp.json()) as ApiErrorBody;
      code = body.error?.code ?? code;
      message = body.error?.message ?? message;
    } catch (_) {
      // body was not the expected envelope; keep status text
    }
    throw new ApiError(resp.status, code, message);
  }
  if (!parseJson) return undefined as unknown as T;
  return (await resp.json()) as T;
}

export async function probeToken(token: string, signal?: AbortSignal): Promise<void> {
  const base = getApiBase();
  const resp = await fetch(`${base}/admin/auth/tokens`, {
    headers: { Authorization: `Bearer ${token}` },
    signal,
  });
  if (resp.status === 401) {
    throw new ApiError(401, "unauthorized", "Token rejected by Marg");
  }
  if (!resp.ok) {
    throw new ApiError(resp.status, "probe_failed", resp.statusText);
  }
}

export async function fetchMetrics(signal?: AbortSignal): Promise<string> {
  const base = getApiBase();
  const token = getToken();
  const headers = new Headers();
  if (token) headers.set("Authorization", `Bearer ${token}`);
  const resp = await fetch(`${base}/metrics`, { headers, signal });
  if (!resp.ok) {
    throw new ApiError(resp.status, "metrics_failed", resp.statusText);
  }
  return await resp.text();
}

export async function listKeys(
  params: { principal?: string; kind?: string; status?: string } = {},
  signal?: AbortSignal,
): Promise<{ keys: MargKey[] }> {
  const q = new URLSearchParams();
  if (params.principal) q.set("principal", params.principal);
  if (params.kind) q.set("kind", params.kind);
  if (params.status) q.set("status", params.status);
  const qs = q.toString();
  return call(`/admin/keys${qs ? `?${qs}` : ""}`, { signal });
}

export async function getKey(
  id: string,
  signal?: AbortSignal,
): Promise<{ key: MargKey; budget: BudgetSpec | null }> {
  return call(`/admin/keys/${encodeURIComponent(id)}`, { signal });
}

export async function createKey(
  body: {
    principal_id: string;
    kind: string;
    team?: string | null;
    daily_budget_usd: number;
    rpm: number;
  },
  signal?: AbortSignal,
): Promise<{ key: MargKey; token: string; budget: BudgetSpec }> {
  return call(`/admin/keys`, {
    method: "POST",
    body: JSON.stringify(body),
    signal,
  });
}

export async function revokeKey(id: string, signal?: AbortSignal): Promise<void> {
  await call(`/admin/keys/${encodeURIComponent(id)}`, { method: "DELETE", signal }, false);
}

export async function invalidateKey(id: string, signal?: AbortSignal): Promise<void> {
  await call(
    `/admin/keys/${encodeURIComponent(id)}/invalidate`,
    { method: "POST", signal },
    false,
  );
}

export async function upsertBudget(spec: BudgetSpec, signal?: AbortSignal): Promise<void> {
  await call(`/admin/budgets`, {
    method: "POST",
    body: JSON.stringify(spec),
    signal,
  }, false);
}

export async function getBudget(
  keyId: string,
  signal?: AbortSignal,
): Promise<BudgetSnapshot> {
  return call(`/admin/budgets/${encodeURIComponent(keyId)}`, { signal });
}

export async function listRoutes(signal?: AbortSignal): Promise<{ routes: PersistedRoute[] }> {
  return call(`/admin/routes`, { signal });
}

export async function createRoute(
  route: RouteSpec,
  signal?: AbortSignal,
): Promise<{ route: PersistedRoute }> {
  return call(`/admin/routes`, {
    method: "POST",
    body: JSON.stringify(route),
    signal,
  });
}

export async function getPolicy(signal?: AbortSignal): Promise<PolicyView> {
  return call(`/admin/policy`, { signal });
}

export async function reloadPolicy(signal?: AbortSignal): Promise<{
  reloaded: boolean;
  config_routes: number;
  stored_routes: number;
  pricing_entries: number;
}> {
  return call(`/admin/policy/reload`, { method: "POST", signal });
}

export async function providerHealth(
  signal?: AbortSignal,
): Promise<{ providers: ProviderHealth[] }> {
  return call(`/admin/providers/health`, { signal });
}

export async function listRequests(
  params: {
    since?: string;
    key_id?: string;
    model?: string;
    provider?: string;
    limit?: number;
  } = {},
  signal?: AbortSignal,
): Promise<{ entries: RequestLogEntry[] }> {
  const q = new URLSearchParams();
  if (params.since) q.set("since", params.since);
  if (params.key_id) q.set("key_id", params.key_id);
  if (params.model) q.set("model", params.model);
  if (params.provider) q.set("provider", params.provider);
  if (params.limit !== undefined) q.set("limit", String(params.limit));
  const qs = q.toString();
  return call(`/admin/requests${qs ? `?${qs}` : ""}`, { signal });
}

export async function listAdminTokens(
  signal?: AbortSignal,
): Promise<{ tokens: AdminTokenRecord[] }> {
  return call(`/admin/auth/tokens`, { signal });
}

export async function createAdminToken(
  label: string | undefined,
  signal?: AbortSignal,
): Promise<{ token_record: AdminTokenRecord; token: string }> {
  return call(`/admin/auth/tokens`, {
    method: "POST",
    body: JSON.stringify({ label }),
    signal,
  });
}

export async function revokeAdminToken(id: string, signal?: AbortSignal): Promise<void> {
  await call(
    `/admin/auth/tokens/${encodeURIComponent(id)}`,
    { method: "DELETE", signal },
    false,
  );
}

export async function getOpenApi(signal?: AbortSignal): Promise<{ info: { version: string } }> {
  return call(`/admin/openapi.json`, { signal });
}
