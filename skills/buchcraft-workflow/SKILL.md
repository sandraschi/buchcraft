---
name: buchcraft-workflow
description: Monthly and year-end workflows in BuchCraft: bank CSV import, AfA drafts, U30 check, and period closing.
---

# BuchCraft Monthly & Closing Workflow

Use this workflow to guide the user through routine bookkeeping and period closing in BuchCraft:

1. **Check status:** Call `buch_status` to inspect entry counts, unmatched bank lines, open drafts, and closed periods.
2. **Drain cross-app events:** If drafts or assets are pending, suggest `buch_inbox_drain` and review them with `buch_drafts`.
3. **Reconcile bank inbox:** Call `buch_bank_unmatched` to see unbooked bank statement lines. Suggest matching EKR accounts and tax keys, then book with `buch_bank_book` (after user confirmation).
4. **Verify VAT (UVA / U30):** Call `buch_u30` for `YYYY-MM` to review Kennzahlen (`000`, `006`, `022`, `029`, `060`, `065`, `070`, `072`, `095`). Remind the user that U30 in BuchCraft is a pre-fill worksheet, not a FinanzOnline filing.
5. **Review trial balance & journal:** Call `buch_balance` and `buch_journal` for the period to verify all balances look sane.
6. **Close the period:** Once everything is reconciled, offer `buch_period_close` for `YYYY-MM`. Closed periods reject new postings dated inside them; corrections require a Storno (`buch_entry_reverse`) in an open period.
