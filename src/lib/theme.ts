// Light/dark choice. "system" follows the Mac's appearance setting; the CSS in app.css does that
// when <html> has no data-theme. src/app.html applies the saved choice before the page draws.

export type Theme = "system" | "light" | "dark";

const KEY = "theme";

export function savedTheme(): Theme {
  try {
    const value = localStorage.getItem(KEY);
    return value === "light" || value === "dark" ? value : "system";
  } catch {
    return "system";
  }
}

export function applyTheme(theme: Theme) {
  if (theme === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
  try {
    if (theme === "system") localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, theme);
  } catch {}
}
