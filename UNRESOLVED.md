# Unresolved Design Questions

Open questions only. Decided items live in their design files.

---

## 3. Extension Init Contract

**Status: Decided** — See `DESIGN-EXTENSIONS.md`.

Extensions implement the `Extension` trait with `fn init(&mut self, reg: &mut ExtensionRegistry)`.
Tools are closures registered via `ExtensionRegistry::tool(schema, closure)`.
Providers via `ExtensionRegistry::provider(api, provider)`.
`ExtensionFactory = Box<dyn Fn() -> Box<dyn Extension>>` provides per-session fresh instances.


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
| 3 | Extension init contract | Decided | `DESIGN-EXTENSIONS.md` |
| 4 | Extension file format | Decided | `DESIGN-EXTENSIONS.md` |
| 5 | SDK re-export surface | Decided | `DESIGN-SDK.md` |
| 6 | Extension sources / modroots | Open | `DESIGN-EXTENSIONS.md` |
| 7 | TUI rendering + tool output | Decided | `DESIGN-TUI.md`, `DESIGN-TOOL-RENDERING.md` |
| 8 | SDK distribution model | Decided | `DESIGN-SDK.md` |
| 9 | Binary synthesis model | Decided | `DESIGN.md` |
| 10 | App layer | Decided | `DESIGN.md` |
