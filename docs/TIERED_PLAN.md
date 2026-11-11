# BuchCraft — Tiered Execution Plan

**Parent Standard:** [`RUST_ARTCRAFT_STANDARD.md`](../../mcp-central-docs/standards/RUST_ARTCRAFT_STANDARD.md)  
**Menu Bar Standard:** [`CRAFT_MENU_BAR_STANDARD.md`](../../mcp-central-docs/standards/rust-artcraft/CRAFT_MENU_BAR_STANDARD.md)  
**Architecture Model:** 4-Tier ArtCraft Incremental Delivery  
**Status:** Active  

---

## Architecture Flow

```
Tier 1: Core Engine ──> Tier 2: MCP / CLI ──> Tier 3: Desktop UI ──> Tier 4: Menu Bar, Help & Packaging
(buchcraft-core)       (buchcraft-mcp)       (buchcraft-app)       (Hardened Workstation)
```

---

## Tier Specifications

### Tier 1 — Core Double-Entry & Tax Engine (`buchcraft-core`)
- [x] L1 `Command::dispatch` universal bookkeeping execution bus.
- [x] SQLite WAL ledger with immutability triggers and cent-exact math.
- [x] Austrian EKR seed chart with tax keys (V20/V10/V13/U20/U10/U13/IG).
- [x] Automated VAT line calculation, Storno reversing links, and month-close locks.
- [x] Bank CSV import and transaction deduplication engine.
- [x] Drafts inbox fed by cross-craft bus (`asset.depreciation_due`).
- [x] `#![forbid(unsafe_code)]` and zero-panic clippy lints.

### Tier 2 — Headless Driver & FastMCP (`buchcraft-mcp`)
- [x] Unified CLI `buchcraft-cli` with `--json` output.
- [x] FastMCP stdio server communicating over JSON-RPC 2.0 (22 tools).
- [x] U30 Kennzahlen calculation and Saldenliste exports.
- [x] Headless `--smoke` verification without window initialization.

### Tier 3 — Desktop Workstation UI (`buchcraft-app`)
- [x] Immediate-mode `egui` 0.37 / `wgpu` workstation with 8 integrated views.
- [x] Interactive booking dialog with EKR account search.
- [x] Receipts inbox with preview and bus draft approval.
- [x] DE/EN bilingual UI support and light/dark theme toggle.

### Tier 4 — Craft Menu Bar Standard, Help Suite & Hardened Verification
- [ ] Universal 8-menu bar (`File`, `Edit`, `View`, `Insert`, `Modify`, `Commands`, `Window`, `Help`) with `switch_on_hover`.
- [ ] Mandatory 5-dialog Help suite (`Command Palette...`, `Keyboard Shortcuts...`, `Project Files & Docs...`, `Licenses & Notices...`, `About BuchCraft`).
- [ ] Fleet UI/UX Pattern 2 adherence: viewport-centric navigation, top button row mapped to View toggles, panel visibility managed via `View` / `Window`.
- [ ] Tofu Guard verification (100% font glyph coverage, zero tofu placeholder boxes).
- [ ] Headless `--smoke` parity validation.
- [ ] Standalone release binary packaging and NSIS installer.
