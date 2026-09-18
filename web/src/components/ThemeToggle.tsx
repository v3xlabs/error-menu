import { FiMoon, FiSun } from "solid-icons/fi";
import { createEffect, createSignal, Show } from "solid-js";

const THEME_STORAGE_KEY = "error-menu-theme";

type Theme = "light" | "dark";

const applyTheme = (theme: Theme): void => {
  document.documentElement.classList.toggle("dark", theme === "dark");
};

const readStoredTheme = (): Theme | null => {
  const stored = localStorage.getItem(THEME_STORAGE_KEY);

  return stored === "light" || stored === "dark" ? stored : null;
};

const preferredTheme = (): Theme => (globalThis.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");

export const ThemeToggle = () => {
  const [theme, setTheme] = createSignal<Theme>("light");

  createEffect(
    () => undefined,
    () => {
      const initial = readStoredTheme() ?? preferredTheme();

      setTheme(initial);
      applyTheme(initial);
    },
  );

  const toggleTheme = (): void => {
    const next: Theme = theme() === "dark" ? "light" : "dark";

    setTheme(next);
    applyTheme(next);
    localStorage.setItem(THEME_STORAGE_KEY, next);
  };

  return (
    <button
      type="button"
      onClick={toggleTheme}
      aria-label={theme() === "dark" ? "Switch to light mode" : "Switch to dark mode"}
      title={theme() === "dark" ? "Switch to light mode" : "Switch to dark mode"}
      class="flex size-8 items-center justify-center rounded-md border border-slate-300 text-slate-700 hover:bg-slate-100 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
    >
      <Show when={theme() === "dark"} fallback={<FiMoon size={16} />}>
        <FiSun size={16} />
      </Show>
    </button>
  );
};
