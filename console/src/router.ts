export type PageHandler = (
  outlet: HTMLElement,
  params: URLSearchParams,
  signal: AbortSignal,
) => void | Promise<void>;

interface Route {
  path: string;
  handler: PageHandler;
}

const routes: Route[] = [];
let currentController: AbortController | null = null;
let outlet: HTMLElement | null = null;
let onBeforeRoute: ((path: string) => boolean | void) | null = null;

export function registerRoute(path: string, handler: PageHandler): void {
  routes.push({ path, handler });
}

export function mountRouter(target: HTMLElement, before?: (path: string) => boolean | void): void {
  outlet = target;
  if (before) onBeforeRoute = before;
  window.addEventListener("hashchange", route);
  route();
}

export function navigate(path: string): void {
  if (`#${path}` === window.location.hash) {
    route();
    return;
  }
  window.location.hash = path;
}

export function currentPath(): string {
  const h = window.location.hash.replace(/^#/, "");
  return h || "/dashboard";
}

function matchRoute(path: string): { route: Route; params: URLSearchParams } | null {
  const [pathOnly, queryStr] = path.split("?");
  const params = new URLSearchParams(queryStr ?? "");
  for (const r of routes) {
    const rParts = r.path.split("/").filter(Boolean);
    const pParts = pathOnly.split("/").filter(Boolean);
    if (rParts.length !== pParts.length) continue;
    let ok = true;
    for (let i = 0; i < rParts.length; i++) {
      if (rParts[i].startsWith(":")) {
        params.set(rParts[i].slice(1), pParts[i]);
      } else if (rParts[i] !== pParts[i]) {
        ok = false;
        break;
      }
    }
    if (ok) return { route: r, params };
  }
  return null;
}

async function route(): Promise<void> {
  if (!outlet) return;
  const path = currentPath();
  if (onBeforeRoute) {
    const allow = onBeforeRoute(path);
    if (allow === false) return;
  }
  if (currentController) currentController.abort();
  currentController = new AbortController();
  const match = matchRoute(path);
  if (!match) {
    outlet.textContent = `No page at ${path}`;
    return;
  }
  try {
    await match.route.handler(outlet, match.params, currentController.signal);
  } catch (e) {
    if ((e as { name?: string }).name === "AbortError") return;
    outlet.textContent = `Error rendering ${path}: ${(e as Error).message}`;
  }
}
