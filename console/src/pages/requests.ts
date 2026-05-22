import { listRequests } from "../api";
import { h, mount } from "../dom";
import { fmtIso, fmtMs, fmtUsd, shortId } from "../format";
import { toastError } from "../toast";
import type { RequestLogEntry } from "../types";
import { formGroup, kv, th } from "../ui";

export async function renderRequests(
  target: HTMLElement,
  params: URLSearchParams,
  signal: AbortSignal,
): Promise<void> {
  const since = h("input", { type: "datetime-local" }) as HTMLInputElement;
  const keyIdInput = h("input", { type: "text", placeholder: "key id", value: params.get("key_id") ?? "" }) as HTMLInputElement;
  const modelInput = h("input", { type: "text", placeholder: "model" }) as HTMLInputElement;
  const providerInput = h("input", { type: "text", placeholder: "provider" }) as HTMLInputElement;
  const limitInput = h("input", { type: "number", min: "1", max: "10000", value: "100" }) as HTMLInputElement;
  const errorOnly = h("input", { type: "checkbox" }) as HTMLInputElement;

  const tbody = h("tbody");
  const detail = h("div");

  let loaded: RequestLogEntry[] = [];

  const layout = h("div", {}, [
    h("div", { class: "page-header" }, [
      h("h1", {}, "Requests"),
      h("div", { class: "controls" }, [
        h("button", { class: "ghost", events: { click: () => fetchAndRender() } }, "Refresh"),
      ]),
    ]),
    h("div", { class: "filter-row" }, [
      formGroup("Since (UTC)", since),
      formGroup("Key id", keyIdInput),
      formGroup("Model", modelInput),
      formGroup("Provider", providerInput),
      formGroup("Limit", limitInput),
      h("label", { style: { display: "flex", gap: "6px", alignItems: "center", marginBottom: 0 } }, [
        errorOnly,
        h("span", {}, "Errors only"),
      ]),
      h("button", { events: { click: () => fetchAndRender() } }, "Apply"),
    ]),
    h("div", { class: "table-wrap" }, [
      h("table", {}, [
        h("thead", {}, h("tr", {}, [
          th("When"),
          th("Key"),
          th("Model"),
          th("Provider"),
          th("Status"),
          th("Latency"),
          th("Attempts"),
          th("Cost"),
        ])),
        tbody,
      ]),
    ]),
    h("div", { style: { marginTop: "12px", display: "flex", justifyContent: "center" } }, [
      h("button", {
        class: "ghost",
        events: { click: () => loadMore() },
      }, "Load more (older)"),
    ]),
    detail,
  ]);
  mount(target, layout);

  async function fetchAndRender(): Promise<void> {
    loaded = [];
    detail.replaceChildren();
    try {
      const res = await listRequests({
        since: since.value ? new Date(since.value).toISOString() : undefined,
        key_id: keyIdInput.value.trim() || undefined,
        model: modelInput.value.trim() || undefined,
        provider: providerInput.value.trim() || undefined,
        limit: Number(limitInput.value) || 100,
      }, signal);
      loaded = res.entries;
      render();
    } catch (e) {
      if ((e as { name?: string }).name === "AbortError") return;
      toastError("Failed to load requests", e);
    }
  }

  async function loadMore(): Promise<void> {
    const last = loaded[loaded.length - 1];
    if (!last) {
      await fetchAndRender();
      return;
    }
    try {
      const olderThan = new Date(new Date(last.timestamp).getTime() - 1).toISOString();
      const res = await listRequests({
        since: olderThan,
        key_id: keyIdInput.value.trim() || undefined,
        model: modelInput.value.trim() || undefined,
        provider: providerInput.value.trim() || undefined,
        limit: Number(limitInput.value) || 100,
      }, signal);
      const fresh = res.entries.filter((e) => !loaded.some((l) => l.id === e.id));
      loaded.push(...fresh);
      render();
    } catch (e) {
      if ((e as { name?: string }).name === "AbortError") return;
      toastError("Load more failed", e);
    }
  }

  function render(): void {
    tbody.replaceChildren();
    const filtered = errorOnly.checked ? loaded.filter((e) => e.status >= 400) : loaded;
    if (filtered.length === 0) {
      tbody.appendChild(h("tr", {}, h("td", { colspan: 8, class: "empty" }, "No requests match.")));
      return;
    }
    for (const r of filtered) {
      tbody.appendChild(h("tr", {
        class: "clickable",
        events: { click: () => renderDetail(r) },
      }, [
        h("td", { class: "mono" }, fmtIso(r.timestamp)),
        h("td", {}, h("a", { href: `#/keys/${encodeURIComponent(r.key_id)}` }, shortId(r.key_id, 10))),
        h("td", {}, r.model),
        h("td", {}, r.provider),
        h("td", {}, h("span", { class: `badge ${r.status >= 400 ? "err" : "ok"}` }, String(r.status))),
        h("td", {}, fmtMs(r.latency_ms)),
        h("td", {}, String(r.attempts.length || 1)),
        h("td", {}, fmtUsd(r.cost_usd)),
      ]));
    }
  }

  function renderDetail(r: RequestLogEntry): void {
    const attemptsBody = h("tbody");
    for (const a of r.attempts) {
      attemptsBody.appendChild(h("tr", {}, [
        h("td", {}, h("code", {}, a.provider)),
        h("td", {}, a.model),
        h("td", {}, h("span", { class: `badge ${a.outcome === "success" ? "ok" : "err"}` }, a.outcome)),
        h("td", {}, String(a.status)),
        h("td", {}, fmtMs(a.latency_ms)),
        h("td", {}, a.error ?? "-"),
      ]));
    }
    detail.replaceChildren(
      h("div", { class: "detail-panel" }, [
        h("h3", {}, "Request detail"),
        kv([
          ["Id", r.id],
          ["When", fmtIso(r.timestamp)],
          ["Key", r.key_id],
          ["Principal", r.principal_id],
          ["Model", r.model],
          ["Provider", r.provider],
          ["Status", String(r.status)],
          ["Stream", r.stream ? "yes" : "no"],
          ["Latency", fmtMs(r.latency_ms)],
          ["Input tokens", String(r.input_tokens)],
          ["Output tokens", String(r.output_tokens)],
          ["Cost", fmtUsd(r.cost_usd)],
          ["Error", r.error ?? "-"],
        ]),
        h("h3", { style: { marginTop: "16px" } }, "Attempt chain"),
        r.attempts.length === 0
          ? h("p", { class: "empty" }, "No attempt detail captured.")
          : h("div", { class: "table-wrap" }, [
            h("table", {}, [
              h("thead", {}, h("tr", {}, [
                th("Provider"),
                th("Model"),
                th("Outcome"),
                th("Status"),
                th("Latency"),
                th("Error"),
              ])),
              attemptsBody,
            ]),
          ]),
      ]),
    );
  }

  await fetchAndRender();
}
