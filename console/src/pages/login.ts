import { probeToken, setToken } from "../api";
import { h } from "../dom";

export function showLogin(onSuccess: () => void): void {
  const tokenInput = h("input", {
    type: "password",
    placeholder: "marg_live_...",
    autocomplete: "off",
    spellcheck: "false",
  }) as HTMLInputElement;

  const errorEl = h("div", { class: "badge err", style: { display: "none", marginTop: "10px" } });

  const submitBtn = h(
    "button",
    {
      class: "primary",
      style: { width: "100%", marginTop: "12px" },
      type: "submit",
    },
    "Sign in",
  );

  const form = h(
    "form",
    {
      events: {
        submit: async (e) => {
          e.preventDefault();
          const value = tokenInput.value.trim();
          if (!value) return;
          submitBtn.setAttribute("disabled", "");
          errorEl.textContent = "";
          (errorEl as HTMLElement).style.display = "none";
          try {
            await probeToken(value);
            setToken(value);
            onSuccess();
          } catch (err) {
            const msg = err instanceof Error ? err.message : String(err);
            errorEl.textContent = msg;
            (errorEl as HTMLElement).style.display = "inline-block";
            submitBtn.removeAttribute("disabled");
          }
        },
      },
    },
    [
      h("div", { class: "form-group" }, [
        h("label", { for: "token" }, "Admin token"),
        tokenInput,
      ]),
      submitBtn,
      errorEl,
    ],
  );

  const hint = h("div", { class: "hint" }, [
    h("div", {}, "Where do tokens come from?"),
    h("ul", {}, [
      h("li", {}, "First-boot token: file printed in the marg start log (default ./marg-admin.token)."),
      h("li", {}, "Rotation: marg admin tokens create on the host, or from the Admin tokens page once signed in."),
      h("li", {}, "Marg only stores the hash. The plaintext is shown once at creation."),
    ]),
  ]);

  const card = h("div", { class: "login-card" }, [
    h("h1", {}, "Marg Console"),
    h("p", { class: "hint" }, "Paste an admin Bearer token to continue."),
    form,
    hint,
  ]);

  const shell = h("div", { class: "login-shell" }, card);
  document.body.replaceChildren(shell);
  setTimeout(() => tokenInput.focus(), 30);
}
