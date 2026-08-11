// App theme (dark default, opt-in light). The choice is persisted locally and
// reflected as a data-theme attribute on <html>, which app.css keys its light
// palette off of. main.ts applies it before mount so the first paint is right.
export type Theme = "light" | "dark";

const LS_KEY = "palcalc.theme";

function read(): Theme {
  try {
    const v = localStorage.getItem(LS_KEY);
    if (v === "light" || v === "dark") return v;
  } catch {
    /* localStorage unavailable — fall through to default */
  }
  return "dark";
}

export const themeStore = $state<{ theme: Theme }>({ theme: read() });

function apply(t: Theme) {
  document.documentElement.dataset.theme = t;
}

/** Apply the saved theme to <html>. Call once, before mount. */
export function initTheme() {
  apply(themeStore.theme);
}

/** Flip between light and dark, persisting the choice. */
export function toggleTheme() {
  themeStore.theme = themeStore.theme === "dark" ? "light" : "dark";
  try {
    localStorage.setItem(LS_KEY, themeStore.theme);
  } catch {
    /* best-effort persistence */
  }
  apply(themeStore.theme);
}
