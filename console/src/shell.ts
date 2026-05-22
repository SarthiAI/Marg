import { clearToken, getOpenApi, getToken } from "./api";
import { h } from "./dom";
import { currentPath, mountRouter, navigate, registerRoute } from "./router";
import { showLogin } from "./pages/login";
import { renderDashboard } from "./pages/dashboard";
import { renderKeysList, renderKeyDetail } from "./pages/keys";
import { renderBudgets } from "./pages/budgets";
import { renderRoutes } from "./pages/routes";
import { renderPolicy } from "./pages/policy";
import { renderProviders } from "./pages/providers";
import { renderRequests } from "./pages/requests";
import { renderAdminTokens } from "./pages/admin_tokens";

interface NavLink { path: string; label: string; }
const links: NavLink[] = [
  { path: "/dashboard", label: "Dashboard" },
  { path: "/keys", label: "Keys" },
  { path: "/budgets", label: "Budgets" },
  { path: "/routes", label: "Routes" },
  { path: "/policy", label: "Policy" },
  { path: "/providers", label: "Providers" },
  { path: "/requests", label: "Requests" },
  { path: "/admin-tokens", label: "Admin tokens" },
];

let version = "";

function buildShell(): { outlet: HTMLElement; titleEl: HTMLElement } {
  const titleEl = h("div", { class: "title" });
  const tokenChipEl = buildTokenChip();
  const themeBtn = h(
    "button",
    {
      class: "ghost",
      title: "Toggle theme",
      events: {
        click: () => {
          const cur = document.documentElement.dataset.theme === "light" ? "dark" : "light";
          document.documentElement.dataset.theme = cur;
          localStorage.setItem("marg.theme", cur);
        },
      },
    },
    "Theme",
  );
  const topbar = h("div", { class: "topbar" }, [
    h("div", { class: "brand" }, "Marg"),
    titleEl,
    h("div", { class: "actions" }, [themeBtn, tokenChipEl]),
  ]);

  const rail = h(
    "nav",
    { class: "rail" },
    links.map((l) =>
      h(
        "a",
        {
          href: `#${l.path}`,
          dataset: { path: l.path },
        },
        l.label,
      ),
    ),
  );

  const outlet = h("main", { class: "main" });

  const footer = h("div", { class: "footer" }, [
    h("span", { id: "footer-version" }, "Marg console"),
    h("a", { href: "/admin/openapi.json", target: "_blank", rel: "noopener" }, "API spec"),
  ]);

  const app = h("div", { class: "app" }, [
    topbar,
    h("div", { class: "layout" }, [rail, outlet]),
    footer,
  ]);

  document.body.replaceChildren(app);
  return { outlet, titleEl };
}

function buildTokenChip(): HTMLElement {
  const tk = getToken();
  const prefix = tk ? `${tk.slice(0, 16)}...` : "no token";
  return h("div", { class: "token-chip" }, [
    h("span", { class: "dot" }),
    h("span", { class: "mono" }, prefix),
    h(
      "button",
      {
        class: "ghost",
        events: {
          click: () => {
            clearToken();
            window.location.reload();
          },
        },
      },
      "Sign out",
    ),
  ]);
}

function highlightNav(path: string): void {
  const links = document.querySelectorAll<HTMLAnchorElement>(".rail a[data-path]");
  for (const a of links) {
    const p = a.dataset.path ?? "";
    if (path === p || path.startsWith(p + "/")) {
      a.classList.add("active");
    } else {
      a.classList.remove("active");
    }
  }
}

function updateFooter(): void {
  const el = document.getElementById("footer-version");
  if (!el) return;
  el.textContent = version ? `Marg ${version}` : "Marg console";
}

export function mountApp(): void {
  const theme = localStorage.getItem("marg.theme");
  if (theme === "light" || theme === "dark") {
    document.documentElement.dataset.theme = theme;
  }
  if (!getToken()) {
    showLogin(mountApp);
    return;
  }
  const { outlet, titleEl } = buildShell();
  registerRoute("/dashboard", (target, _params, signal) => {
    titleEl.textContent = "Dashboard";
    return renderDashboard(target, signal);
  });
  registerRoute("/keys", (target, _params, signal) => {
    titleEl.textContent = "Keys";
    return renderKeysList(target, signal);
  });
  registerRoute("/keys/:id", (target, params, signal) => {
    titleEl.textContent = "Key detail";
    return renderKeyDetail(target, params.get("id") ?? "", signal);
  });
  registerRoute("/budgets", (target, _params, signal) => {
    titleEl.textContent = "Budgets";
    return renderBudgets(target, signal);
  });
  registerRoute("/routes", (target, _params, signal) => {
    titleEl.textContent = "Routes";
    return renderRoutes(target, signal);
  });
  registerRoute("/policy", (target, _params, signal) => {
    titleEl.textContent = "Policy";
    return renderPolicy(target, signal);
  });
  registerRoute("/providers", (target, _params, signal) => {
    titleEl.textContent = "Providers";
    return renderProviders(target, signal);
  });
  registerRoute("/requests", (target, params, signal) => {
    titleEl.textContent = "Requests";
    return renderRequests(target, params, signal);
  });
  registerRoute("/admin-tokens", (target, _params, signal) => {
    titleEl.textContent = "Admin tokens";
    return renderAdminTokens(target, signal);
  });

  mountRouter(outlet, (path) => {
    highlightNav(path);
  });

  if (!window.location.hash) {
    navigate("/dashboard");
  } else {
    highlightNav(currentPath());
  }

  void getOpenApi().then((s) => {
    version = s.info?.version ?? "";
    updateFooter();
  }).catch(() => undefined);
}
