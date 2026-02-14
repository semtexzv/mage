# Unresolved Design Questions

Open questions only. Decided items live in their design files.

---

## 3. Extension Init Contract

**Status: Partially Decided** — See `DESIGN-EXTENSIONS.md`.

Convention-based detection. Sync init. `mage::prelude`: yes.

Open: registry method signature (lambda vs direct). Leaning lambda
for extensions with lifecycle hooks (so each session gets a fresh instance).

---

## 6. Extension Sources and Module Roots

**Status: Open** — See `DESIGN-EXTENSIONS.md`.

Rethinking: system + project modroots. Project modroot builds
project-local binary. Global mage delegates to project binary.
UI notification for binary switching. Previous 4-tier model under revision.

---

## Decision Log

| # | Question | Status | Design File |
|---|---|---|---|
| 1 | Reproducible builds | Decided | `DESIGN-REPRODUCIBLE-BUILDS.md` |
| 2 | Tool execution & rendering | Decided | `DESIGN-TOOL-RENDERING.md`, `DESIGN-TUI.md` |
| 3 | Extension init contract | Partially Decided | `DESIGN-EXTENSIONS.md` |
| 4 | Extension file format | Decided | `DESIGN-EXTENSIONS.md` |
| 5 | SDK re-export surface | Decided | `DESIGN-SDK.md` |
| 6 | Extension sources / modroots | Open | `DESIGN-EXTENSIONS.md` |
| 7 | TUI rendering + tool output | Decided | `DESIGN-TUI.md`, `DESIGN-TOOL-RENDERING.md` |
