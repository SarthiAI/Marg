import { providerHealth } from "../api";
import { h, mount } from "../dom";
import { fmtNumber } from "../format";
import { toastError } from "../toast";
import type { ProviderHealth } from "../types";
import { th } from "../ui";

const POLL_MS = 10_000;

export async function renderProviders(target: HTMLElement, signal: AbortSignal): Promise<void> {
  const tbody = h("tbody");
  const layout = h("div", {}, [
    h("div", { class: "page-header" }, [
      h("h1", {}, "Providers"),
      h("div", { class: "controls" }, [
        h("span", { class: "mono", style: { fontSize: "11px", color: "var(--muted)" } }, `Polling ${POLL_MS / 1000}s`),
        h("button", { class: "ghost", events: { click: () => refresh() } }, "Refresh"),
      ]),
    ]),
    h("p", { class: "help-block" }, "Counters come from this Marg process. Restart resets them. Use Grafana for fleet-wide history."),
    h("div", { class: "table-wrap" }, [
      h("table", {}, [
        h("thead", {}, h("tr", {}, [
          th("Name"),
          th("State"),
          th("Successes"),
          th("Errors 5xx"),
          th("Errors 4xx"),
          th("Timeouts"),
          th("Network"),
          th("Error rate"),
        ])),
        tbody,
      ]),
    ]),
  ]);
  mount(target, layout);

  async function refresh(): Promise<void> {
    try {
      const { providers } = await providerHealth(signal);
      tbody.replaceChildren();
      if (providers.length === 0) {
        tbody.appendChild(h("tr", {}, h("td", { colspan: 8, class: "empty" }, "No providers configured.")));
        return;
      }
      for (const p of providers) tbody.appendChild(row(p));
    } catch (e) {
      if ((e as { name?: string }).name === "AbortError") return;
      toastError("Failed to load providers", e);
    }
  }

  function row(p: ProviderHealth): HTMLElement {
    const errors = p.errors_5xx + p.errors_4xx + p.timeouts + p.network_errors;
    const total = errors + p.successes_total;
    const rate = total === 0 ? 0 : (errors / total);
    let state: "healthy" | "degraded" | "unhealthy" | "unknown";
    if (total < 100) state = "unknown";
    else if (rate < 0.01) state = "healthy";
    else if (rate < 0.1) state = "degraded";
    else state = "unhealthy";
    return h("tr", {}, [
      h("td", {}, h("code", {}, p.name)),
      h("td", {}, h("span", { class: `badge ${state}` }, state)),
      h("td", {}, fmtNumber(p.successes_total)),
      h("td", {}, fmtNumber(p.errors_5xx)),
      h("td", {}, fmtNumber(p.errors_4xx)),
      h("td", {}, fmtNumber(p.timeouts)),
      h("td", {}, fmtNumber(p.network_errors)),
      h("td", {}, total === 0 ? "-" : `${(rate * 100).toFixed(2)}%`),
    ]);
  }

  await refresh();
  const timer = window.setInterval(() => {
    if (signal.aborted) {
      window.clearInterval(timer);
      return;
    }
    void refresh();
  }, POLL_MS);
  signal.addEventListener("abort", () => window.clearInterval(timer));
}
