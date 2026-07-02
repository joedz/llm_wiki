// Zustand store for the code-wiki persona selector.
//
// Mirrors UA's `Persona` type from `packages/dashboard/src/store.ts`.
// Three personas — "non-technical" / "junior" / "experienced". In UA
// the default is "junior"; we match that. State persists to
// localStorage so the selection survives page reloads (mirroring
// UA's in-memory default + theme persistence on the side).

import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";

export type Persona = "non-technical" | "junior" | "experienced";

const DEFAULT_PERSONA: Persona = "junior";
const STORAGE_KEY = "llm-wiki.code-wiki.persona";

function is_persona(value: unknown): value is Persona {
  return (
    value === "non-technical" ||
    value === "junior" ||
    value === "experienced"
  );
}

interface PersonaState {
  persona: Persona;
  setPersona: (p: Persona) => void;
  isLearnMode: () => boolean;
}

export const useCodeWikiPersonaStore = create<PersonaState>()(
  persist(
    (set, get) => ({
      persona: DEFAULT_PERSONA,
      setPersona: (p) => set({ persona: p }),
      isLearnMode: () => {
        // UA's behavior: learn mode is active for non-technical
        // and junior personas; experienced users see the bare
        // dashboard with no learn panel.
        const p = get().persona;
        return p === "non-technical" || p === "junior";
      },
    }),
    {
      name: STORAGE_KEY,
      storage: createJSONStorage(() => localStorage),
      // Defensive: if the localStorage entry was set by an older
      // version with a different shape, fall back to default.
      partialize: (s) => ({ persona: s.persona }),
      onRehydrateStorage: () => (state) => {
        if (state && !is_persona(state.persona)) {
          state.persona = DEFAULT_PERSONA;
        }
      },
    },
  ),
);
