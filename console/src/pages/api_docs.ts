import { getToken } from "../api";
import { h, mount } from "../dom";
import { ApiError } from "../types";
import { toastError } from "../toast";
import { th } from "../ui";

interface OperationSpec {
  summary?: string;
  description?: string;
  parameters?: ParamSpec[];
  requestBody?: { required?: boolean; content?: Record<string, ContentSpec> };
  responses?: Record<string, ResponseSpec>;
}

interface ParamSpec {
  name: string;
  in: string;
  required?: boolean;
  schema?: Record<string, unknown>;
}

interface ContentSpec {
  schema?: Record<string, unknown>;
}

interface ResponseSpec {
  description?: string;
  content?: Record<string, ContentSpec>;
}

interface OpenApiSpec {
  info?: { title?: string; version?: string };
  paths?: Record<string, Record<string, OperationSpec>>;
  components?: { schemas?: Record<string, Record<string, unknown>> };
}

const METHODS = ["get", "post", "put", "patch", "delete"];

export async function renderApiDocs(
  target: HTMLElement,
  signal: AbortSignal,
): Promise<void> {
  const summary = h("div", { class: "card" }, "Loading API spec...");
  const groupsHost = h("div");
  const rawLink = h("a", {
    href: "/admin/openapi.json",
    target: "_blank",
    rel: "noopener",
  }, "/admin/openapi.json (raw OpenAPI 3.1 JSON)");

  const layout = h("div", {}, [
    h("div", { class: "page-header" }, [
      h("h1", {}, "API"),
      h("div", { class: "controls" }, [
        h("button", {
          class: "ghost",
          events: { click: () => load() },
        }, "Refresh"),
      ]),
    ]),
    h("p", { class: "help-block" }, [
      "Every endpoint requires ",
      h("code", {}, "Authorization: Bearer <admin-token>") as HTMLElement,
      ". Full spec: ",
      rawLink as HTMLElement,
      ".",
    ]),
    summary,
    groupsHost,
  ]);

  mount(target, layout);
  await load();

  async function load(): Promise<void> {
    summary.replaceChildren("Loading API spec...");
    groupsHost.replaceChildren();
    try {
      const spec = await fetchSpec(signal);
      renderSummary(spec, summary);
      renderGroups(spec, groupsHost);
    } catch (e) {
      toastError(e);
      summary.replaceChildren(
        h("p", {}, `Could not load /admin/openapi.json: ${
          e instanceof Error ? e.message : String(e)
        }`),
      );
    }
  }
}

async function fetchSpec(signal: AbortSignal): Promise<OpenApiSpec> {
  const token = getToken();
  const headers = new Headers();
  if (token) headers.set("Authorization", `Bearer ${token}`);
  const resp = await fetch("/admin/openapi.json", { headers, signal });
  if (!resp.ok) {
    throw new ApiError(resp.status, "spec_failed", resp.statusText);
  }
  return (await resp.json()) as OpenApiSpec;
}

function renderSummary(spec: OpenApiSpec, into: HTMLElement): void {
  const title = spec.info?.title ?? "Marg admin API";
  const ver = spec.info?.version ?? "";
  const groups = groupPaths(spec.paths ?? {});
  const opCount = Object.values(spec.paths ?? {})
    .flatMap((methods) => Object.keys(methods))
    .filter((m) => METHODS.includes(m.toLowerCase())).length;
  into.replaceChildren(
    h("div", { class: "kv" }, [
      h("div", { class: "k" }, "Spec"),
      h("div", { class: "v" }, title),
      h("div", { class: "k" }, "Version"),
      h("div", { class: "v" }, ver || "unknown"),
      h("div", { class: "k" }, "Groups"),
      h("div", { class: "v" }, String(groups.length)),
      h("div", { class: "k" }, "Endpoints"),
      h("div", { class: "v" }, String(opCount)),
    ]),
  );
}

function renderGroups(spec: OpenApiSpec, into: HTMLElement): void {
  const paths = spec.paths ?? {};
  const groups = groupPaths(paths);
  if (groups.length === 0) {
    into.replaceChildren(h("p", { class: "help-block" }, "No paths in spec."));
    return;
  }
  const children: HTMLElement[] = [];
  for (const g of groups) {
    children.push(renderGroup(g));
  }
  into.replaceChildren(...children);
}

interface GroupDef {
  name: string;
  endpoints: Array<{ method: string; path: string; op: OperationSpec }>;
}

function groupPaths(paths: Record<string, Record<string, OperationSpec>>): GroupDef[] {
  const groups = new Map<string, GroupDef>();
  for (const [path, methods] of Object.entries(paths)) {
    const name = groupName(path);
    if (!groups.has(name)) {
      groups.set(name, { name, endpoints: [] });
    }
    const g = groups.get(name)!;
    for (const [method, op] of Object.entries(methods)) {
      if (!METHODS.includes(method.toLowerCase())) continue;
      g.endpoints.push({ method: method.toUpperCase(), path, op });
    }
  }
  // Sort endpoints within each group by path then method order
  for (const g of groups.values()) {
    g.endpoints.sort((a, b) => {
      if (a.path !== b.path) return a.path.localeCompare(b.path);
      return METHODS.indexOf(a.method.toLowerCase()) - METHODS.indexOf(b.method.toLowerCase());
    });
  }
  return Array.from(groups.values()).sort((a, b) => a.name.localeCompare(b.name));
}

function groupName(path: string): string {
  // /admin/keys -> Keys, /admin/auth/tokens -> Auth, /admin/openapi.json -> Meta
  const segs = path.split("/").filter(Boolean);
  if (segs[0] !== "admin") return capitalize(segs[0] ?? "other");
  if (segs[1] === "openapi.json") return "Meta";
  if (!segs[1]) return "Admin";
  return capitalize(segs[1]);
}

function capitalize(s: string): string {
  return s.length > 0 ? s[0].toUpperCase() + s.slice(1) : s;
}

function renderGroup(group: GroupDef): HTMLElement {
  const body = h("tbody");
  for (const ep of group.endpoints) {
    body.appendChild(renderEndpointRow(ep.method, ep.path, ep.op));
  }
  return h("section", { style: { marginTop: "20px" } }, [
    h("h3", { style: { fontSize: "14px", marginBottom: "8px" } }, group.name),
    h("div", { class: "table-wrap" }, [
      h("table", {}, [
        h("thead", {}, h("tr", {}, [
          th("Method"),
          th("Path"),
          th("Summary"),
        ])),
        body,
      ]),
    ]),
  ]);
}

function renderEndpointRow(method: string, path: string, op: OperationSpec): HTMLElement {
  const expanded = h("tr", { style: { display: "none" } }, [
    h("td", { attrs: { colspan: "3" } }, renderEndpointDetail(op)),
  ]);
  const row = h("tr", {
    style: { cursor: "pointer" },
    events: {
      click: () => {
        expanded.style.display = expanded.style.display === "none" ? "" : "none";
      },
    },
  }, [
    h("td", {}, [methodBadge(method)]),
    h("td", { class: "mono", style: { fontSize: "12px" } }, path),
    h("td", { style: { color: "var(--muted)" } }, op.summary ?? ""),
  ]);
  const wrap = h("tbody", { style: { display: "contents" } }, [row, expanded]);
  return wrap;
}

function methodBadge(method: string): HTMLElement {
  const colors: Record<string, string> = {
    GET: "#2563eb",
    POST: "#16a34a",
    PUT: "#ca8a04",
    PATCH: "#ca8a04",
    DELETE: "#dc2626",
  };
  const bg = colors[method] ?? "#6b7280";
  return h("span", {
    class: "mono",
    style: {
      background: bg,
      color: "white",
      padding: "2px 6px",
      borderRadius: "3px",
      fontSize: "10px",
      fontWeight: "600",
    },
  }, method);
}

function renderEndpointDetail(op: OperationSpec): HTMLElement {
  const sections: HTMLElement[] = [];
  if (op.description) {
    sections.push(
      h("p", { style: { margin: "6px 0", color: "var(--muted)" } }, op.description),
    );
  }
  if (op.parameters && op.parameters.length > 0) {
    sections.push(renderParameters(op.parameters));
  }
  if (op.requestBody) {
    sections.push(renderRequestBody(op.requestBody));
  }
  if (op.responses) {
    sections.push(renderResponses(op.responses));
  }
  if (sections.length === 0) {
    sections.push(
      h("p", { class: "help-block" }, "No additional schema in the spec for this endpoint."),
    );
  }
  return h("div", {
    style: {
      padding: "8px 12px",
      background: "var(--bg-soft, rgba(0,0,0,0.04))",
      borderLeft: "3px solid var(--accent, #2563eb)",
    },
  }, sections);
}

function renderParameters(params: ParamSpec[]): HTMLElement {
  const body = h("tbody");
  for (const p of params) {
    body.appendChild(h("tr", {}, [
      h("td", { class: "mono", style: { fontSize: "11px" } }, p.name),
      h("td", { class: "mono", style: { fontSize: "11px", color: "var(--muted)" } }, p.in),
      h("td", { class: "mono", style: { fontSize: "11px" } }, p.required ? "required" : ""),
      h("td", { class: "mono", style: { fontSize: "11px" } }, schemaSummary(p.schema)),
    ]));
  }
  return h("div", { style: { marginTop: "8px" } }, [
    h("div", { class: "help-block", style: { fontWeight: "600" } }, "Parameters"),
    h("div", { class: "table-wrap" }, [
      h("table", {}, [
        h("thead", {}, h("tr", {}, [
          th("Name"),
          th("In"),
          th("Required"),
          th("Type"),
        ])),
        body,
      ]),
    ]),
  ]);
}

function renderRequestBody(body: { required?: boolean; content?: Record<string, ContentSpec> }): HTMLElement {
  const ct = body.content ? Object.entries(body.content)[0] : undefined;
  const schema = ct?.[1].schema;
  return h("div", { style: { marginTop: "8px" } }, [
    h("div", { class: "help-block", style: { fontWeight: "600" } },
      body.required ? "Request body (required)" : "Request body"),
    h("pre", { class: "mono", style: { fontSize: "11px", margin: "4px 0", overflow: "auto" } },
      schema ? JSON.stringify(schema, null, 2) : "(no schema)"),
  ]);
}

function renderResponses(responses: Record<string, ResponseSpec>): HTMLElement {
  const items: HTMLElement[] = [];
  for (const [code, resp] of Object.entries(responses)) {
    const ct = resp.content ? Object.entries(resp.content)[0] : undefined;
    items.push(
      h("div", { style: { marginTop: "6px" } }, [
        h("div", { style: { display: "flex", gap: "8px", alignItems: "baseline" } }, [
          h("span", {
            class: "mono",
            style: {
              fontSize: "11px",
              fontWeight: "600",
              padding: "2px 6px",
              background: code.startsWith("2") ? "#16a34a" : "#dc2626",
              color: "white",
              borderRadius: "3px",
            },
          }, code),
          h("span", { style: { color: "var(--muted)", fontSize: "12px" } },
            resp.description ?? ""),
        ]),
        ct?.[1].schema ? h("pre", {
          class: "mono",
          style: { fontSize: "11px", margin: "4px 0 0 12px", overflow: "auto" },
        }, JSON.stringify(ct[1].schema, null, 2)) : h("span", {}, ""),
      ]),
    );
  }
  return h("div", { style: { marginTop: "8px" } }, [
    h("div", { class: "help-block", style: { fontWeight: "600" } }, "Responses"),
    ...items,
  ]);
}

function schemaSummary(schema?: Record<string, unknown>): string {
  if (!schema) return "";
  const type = schema.type;
  if (typeof type === "string") return type;
  if (Array.isArray(type)) return type.join(" | ");
  if ("$ref" in schema && typeof schema.$ref === "string") {
    const ref = schema.$ref as string;
    return ref.split("/").pop() ?? ref;
  }
  return "object";
}
