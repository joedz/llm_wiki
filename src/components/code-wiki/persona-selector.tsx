// Persona selector for the code-wiki dashboard launcher.
//
// Mirrors UA's `PersonaSelector` (`packages/dashboard/src/components/PersonaSelector.tsx`):
// three buttons (`non-technical` / `junior` / `experienced`).
// Toggling updates the `useCodeWikiPersonaStore` and the next
// `code_wiki_open_dashboard` call gets the new persona in its
// `?persona=` URL query string.

import { useTranslation } from "react-i18next";

import {
  useCodeWikiPersonaStore,
  type Persona,
} from "@/stores/code-wiki-persona-store";

const ORDER: Persona[] = ["non-technical", "junior", "experienced"];

export function PersonaSelector() {
  const { t } = useTranslation();
  const persona = useCodeWikiPersonaStore((s) => s.persona);
  const setPersona = useCodeWikiPersonaStore((s) => s.setPersona);

  return (
    <div
      className="inline-flex items-center gap-1 rounded-md border border-border bg-card p-1 text-xs"
      role="radiogroup"
      aria-label={t("codeWiki.persona.label", "Dashboard persona")}
    >
      {ORDER.map((p) => {
        const active = persona === p;
        return (
          <button
            key={p}
            type="button"
            role="radio"
            aria-checked={active}
            onClick={() => setPersona(p)}
            className={
              "rounded px-2 py-0.5 transition-colors " +
              (active
                ? "bg-foreground text-background"
                : "text-muted-foreground hover:text-foreground")
            }
          >
            {t(`codeWiki.persona.${p}`, DEFAULT_LABEL(p))}
          </button>
        );
      })}
    </div>
  );
}

function DEFAULT_LABEL(p: Persona): string {
  switch (p) {
    case "non-technical":
      return "Overview";
    case "junior":
      return "Learn";
    case "experienced":
      return "Deep dive";
  }
}
