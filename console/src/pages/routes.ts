import { createRoute, getPolicy, listRoutes } from "../api";
import { h, mount } from "../dom";
import { fmtIso } from "../format";
import { toast, toastError } from "../toast";
import type { ConfigRoute, PersistedRoute, SplitEntry } from "../types";
import { formGroup, openDrawer, th } from "../ui";

export async function renderRoutes(target: HTMLElement, signal: AbortSignal): Promise<void> {
  const persistedBody = h("tbody");
  const configBody = h("tbody");
  const layout = h("div", {}, [
    h("div", { class: "page-header" }, [
      h("h1", {}, "Routes"),
      h("div", { class: "controls" }, [
        h("button", { class: "ghost", events: { click: () => refresh() } }, "Refresh"),
        h("button", { class: "primary", events: { click: () => openCreate(refresh) } }, "Add persisted route"),
      ]),
    ]),
    h("h3", { style: { marginTop: "16px", fontSize: "14px" } }, "Persisted routes"),
    h("p", { class: "help-block" }, "Routes you create here are stored and survive restart."),
    h("div", { class: "table-wrap" }, [
      h("table", {}, [
        h("thead", {}, h("tr", {}, [
          th("Position"),
          th("Match"),
          th("Primary / split"),
          th("Fallbacks"),
          th("Created"),
        ])),
        persistedBody,
      ]),
    ]),
    h("h3", { style: { marginTop: "24px", fontSize: "14px" } }, "Config routes (read-only)"),
    h("p", { class: "help-block" }, "Edit marg.toml on the host and click Reload on the Policy page to refresh these."),
    h("div", { class: "table-wrap" }, [
      h("table", {}, [
        h("thead", {}, h("tr", {}, [
          th("#"),
          th("Match"),
          th("Primary / split"),
          th("Fallbacks"),
        ])),
        configBody,
      ]),
    ]),
  ]);
  mount(target, layout);

  async function refresh(): Promise<void> {
    try {
      const [pers, pol] = await Promise.all([
        listRoutes(signal),
        getPolicy(signal),
      ]);
      persistedBody.replaceChildren();
      if (pers.routes.length === 0) {
        persistedBody.appendChild(h("tr", {}, h("td", { colspan: 5, class: "empty" }, "No persisted routes yet.")));
      } else {
        for (const r of pers.routes) {
          persistedBody.appendChild(persistedRow(r));
        }
      }
      configBody.replaceChildren();
      if (pol.config_routes.length === 0) {
        configBody.appendChild(h("tr", {}, h("td", { colspan: 4, class: "empty" }, "No config routes.")));
      } else {
        pol.config_routes.forEach((r, i) => {
          configBody.appendChild(configRow(r, i));
        });
      }
    } catch (e) {
      if ((e as { name?: string }).name === "AbortError") return;
      toastError("Failed to load routes", e);
    }
  }

  await refresh();
}

function persistedRow(r: PersistedRoute): HTMLElement {
  return h("tr", {}, [
    h("td", {}, String(r.position ?? "-")),
    h("td", {}, matchSummary(r.match_model, r.match_team)),
    h("td", {}, primarySummary(r.primary, r.primary_model, r.split)),
    h("td", {}, (r.fallbacks ?? []).join(", ") || "-"),
    h("td", { class: "mono" }, fmtIso(r.created_at)),
  ]);
}

function configRow(r: ConfigRoute, idx: number): HTMLElement {
  return h("tr", {}, [
    h("td", {}, String(idx + 1)),
    h("td", {}, matchSummary(r.match?.model ?? null, r.match?.team ?? null)),
    h("td", {}, primarySummary(r.primary ?? null, null, r.split ?? null)),
    h("td", {}, (r.fallback ?? []).join(", ") || "-"),
  ]);
}

function matchSummary(model: string | null, team: string | null): string {
  const parts: string[] = [];
  if (model) parts.push(`model=${model}`);
  if (team) parts.push(`team=${team}`);
  return parts.join(" ") || "(any)";
}

function primarySummary(
  primary: string | null | undefined,
  primaryModel: string | null | undefined,
  split: SplitEntry[] | null | undefined,
): string {
  if (split && split.length > 0) {
    return `split: ${split.map((s) => `${s.provider}${s.model ? `:${s.model}` : ""}@${s.weight}`).join(", ")}`;
  }
  if (primary) {
    return `primary: ${primary}${primaryModel ? `:${primaryModel}` : ""}`;
  }
  return "-";
}

function openCreate(onCreated: () => void | Promise<void>): void {
  const position = h("input", { type: "number", placeholder: "auto (max + 1)" }) as HTMLInputElement;
  const matchModel = h("input", { type: "text", placeholder: "e.g. gpt-4*" }) as HTMLInputElement;
  const matchTeam = h("input", { type: "text", placeholder: "optional team" }) as HTMLInputElement;
  const mode = h("select", {}, [
    h("option", { value: "primary" }, "Single primary with optional fallbacks"),
    h("option", { value: "split" }, "Weighted split"),
  ]) as HTMLSelectElement;
  const primary = h("input", { type: "text", placeholder: "openai or openai:gpt-4o" }) as HTMLInputElement;
  const primaryModel = h("input", { type: "text", placeholder: "optional model override" }) as HTMLInputElement;
  const fallbacks = h("input", { type: "text", placeholder: "anthropic:claude-3-5-sonnet, google" }) as HTMLInputElement;

  const splitContainer = h("div");
  const addSplitRow = h("button", {
    class: "ghost",
    events: { click: () => splitContainer.appendChild(splitRow()) },
  }, "+ add split entry");

  function splitRow(): HTMLElement {
    const provider = h("input", { type: "text", placeholder: "provider" }) as HTMLInputElement;
    const weight = h("input", { type: "number", min: "1", value: "1" }) as HTMLInputElement;
    const modelOverride = h("input", { type: "text", placeholder: "model (optional)" }) as HTMLInputElement;
    const row = h("div", { class: "split-row" });
    row.appendChild(provider);
    row.appendChild(weight);
    row.appendChild(modelOverride);
    row.appendChild(h("button", { class: "ghost", events: { click: () => row.remove() } }, "X"));
    return row;
  }

  const primaryBlock = h("div", {}, [
    formGroup("Primary provider (use provider:model_override for renaming)", primary),
    formGroup("Model override on primary (optional)", primaryModel),
    formGroup("Fallbacks (comma-separated, ordered)", fallbacks),
  ]);
  const splitBlock = h("div", {}, [
    splitContainer,
    addSplitRow,
  ]);
  (splitBlock as HTMLElement).style.display = "none";

  mode.addEventListener("change", () => {
    if (mode.value === "split") {
      (primaryBlock as HTMLElement).style.display = "none";
      (splitBlock as HTMLElement).style.display = "block";
      if (splitContainer.children.length === 0) {
        splitContainer.appendChild(splitRow());
      }
    } else {
      (primaryBlock as HTMLElement).style.display = "block";
      (splitBlock as HTMLElement).style.display = "none";
    }
  });

  const body = h("div", {}, [
    formGroup("Position (lower = evaluated first)", position),
    formGroup("Match model (glob, optional)", matchModel),
    formGroup("Match team (optional)", matchTeam),
    formGroup("Type", mode),
    primaryBlock,
    splitBlock,
  ]);

  let onClose = () => undefined as void;
  onClose = openDrawer("Add persisted route", body, [
    { label: "Cancel", kind: "ghost", onClick: () => onClose() },
    {
      label: "Create",
      kind: "primary",
      onClick: async () => {
        try {
          const positionVal = position.value.trim() ? Number(position.value) : undefined;
          const matchModelVal = matchModel.value.trim() || null;
          const matchTeamVal = matchTeam.value.trim() || null;
          if (mode.value === "primary") {
            if (!primary.value.trim()) {
              toast("Primary provider is required", "error");
              return;
            }
            const fbs = fallbacks.value
              .split(",")
              .map((s) => s.trim())
              .filter((s) => s.length > 0);
            await createRoute({
              position: positionVal,
              match_model: matchModelVal,
              match_team: matchTeamVal,
              primary: primary.value.trim(),
              primary_model: primaryModel.value.trim() || null,
              fallbacks: fbs,
              split: [],
            });
          } else {
            const rows = Array.from(splitContainer.children) as HTMLElement[];
            const entries: SplitEntry[] = [];
            for (const row of rows) {
              const inputs = row.querySelectorAll("input");
              const providerVal = (inputs[0] as HTMLInputElement)?.value.trim() ?? "";
              const weightVal = Number((inputs[1] as HTMLInputElement)?.value);
              const modelVal = (inputs[2] as HTMLInputElement)?.value.trim();
              if (!providerVal || !weightVal || weightVal <= 0) continue;
              entries.push({
                provider: providerVal,
                weight: weightVal,
                model: modelVal || null,
              });
            }
            if (entries.length === 0) {
              toast("Add at least one split entry", "error");
              return;
            }
            await createRoute({
              position: positionVal,
              match_model: matchModelVal,
              match_team: matchTeamVal,
              fallbacks: [],
              split: entries,
            });
          }
          toast("Route created and policy reloaded", "ok");
          onClose();
          await onCreated();
        } catch (e) {
          toastError("Create failed", e);
        }
      },
    },
  ]);
}
