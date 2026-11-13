//! BuchCraft desktop app (L3). Mutations go through `buchcraft_core::dispatch`
//! (the same path as CLI and MCP); reads use the typed ledger API directly.

use buchcraft_core::{dispatch, Command, Ctx};
use buchcraft_store::{Account, Balances, BankTx, Draft, Entry, Kreis, LedgerRow, U30Report};
use chrono::{Datelike, Local, NaiveDate};
use craft_biz::at::{format_date_de, parse_date};
use craft_biz::deadline::month_end;
use craft_biz::ekr::{AccountKind, TaxKey};
use craft_biz::i18n::Locale;
use craft_biz::Money;
use craft_biz_ui::bookend::{self, Bar, Spec};
use eframe::egui::{self, Color32, ColorImage, RichText};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Overview,
    BankInbox,
    NewEntry,
    Journal,
    Balances,
    AccountLedger,
    U30,
    Drafts,
}

impl View {
    pub const ALL: [View; 8] = [
        View::Overview,
        View::BankInbox,
        View::NewEntry,
        View::Journal,
        View::Balances,
        View::AccountLedger,
        View::U30,
        View::Drafts,
    ];

    pub fn parse(s: &str) -> Option<View> {
        match s.trim().to_ascii_lowercase().as_str() {
            "overview" | "dashboard" => Some(View::Overview),
            "bank" | "inbox" => Some(View::BankInbox),
            "new" | "book" => Some(View::NewEntry),
            "journal" => Some(View::Journal),
            "balance" | "balances" => Some(View::Balances),
            "ledger" => Some(View::AccountLedger),
            "u30" => Some(View::U30),
            "drafts" => Some(View::Drafts),
            _ => None,
        }
    }

    fn label(self, l: Locale) -> &'static str {
        let de = l == Locale::De;
        match self {
            View::Overview => {
                if de {
                    "Übersicht"
                } else {
                    "Overview"
                }
            }
            View::BankInbox => {
                if de {
                    "Bank-Eingang"
                } else {
                    "Bank inbox"
                }
            }
            View::NewEntry => {
                if de {
                    "Buchen"
                } else {
                    "Book entry"
                }
            }
            View::Journal => "Journal",
            View::Balances => {
                if de {
                    "Saldenliste"
                } else {
                    "Trial balance"
                }
            }
            View::AccountLedger => {
                if de {
                    "Kontoblatt"
                } else {
                    "Account ledger"
                }
            }
            View::U30 => {
                if de {
                    "UVA (U30)"
                } else {
                    "VAT return (U30)"
                }
            }
            View::Drafts => {
                if de {
                    "Entwürfe"
                } else {
                    "Drafts"
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthSel {
    pub year: i32,
    pub month: u32,
}

impl MonthSel {
    pub fn of(d: NaiveDate) -> MonthSel {
        MonthSel {
            year: d.year(),
            month: d.month(),
        }
    }

    pub fn parse(s: &str) -> Option<MonthSel> {
        NaiveDate::parse_from_str(&format!("{}-01", s.trim()), "%Y-%m-%d")
            .ok()
            .map(MonthSel::of)
    }

    pub fn first(self) -> NaiveDate {
        NaiveDate::from_ymd_opt(self.year, self.month, 1).unwrap_or(NaiveDate::MIN)
    }

    pub fn last(self) -> NaiveDate {
        month_end(self.first()).unwrap_or(NaiveDate::MAX)
    }

    pub fn prev(self) -> MonthSel {
        if self.month == 1 {
            MonthSel {
                year: self.year - 1,
                month: 12,
            }
        } else {
            MonthSel {
                year: self.year,
                month: self.month - 1,
            }
        }
    }

    pub fn next(self) -> MonthSel {
        if self.month == 12 {
            MonthSel {
                year: self.year + 1,
                month: 1,
            }
        } else {
            MonthSel {
                year: self.year,
                month: self.month + 1,
            }
        }
    }

    pub fn label(self) -> String {
        format!("{}-{:02}", self.year, self.month)
    }
}

#[derive(Debug, Clone, Default)]
struct Kpi {
    bank: Money,
    revenue: Money,
    expense: Money,
    zahllast: Money,
}

#[derive(Default)]
struct Data {
    company: String,
    demo: bool,
    entries_total: i64,
    accounts: Vec<Account>,
    journal: Vec<Entry>,
    bank: Vec<BankTx>,
    balances: Option<Balances>,
    u30: Option<U30Report>,
    drafts: Vec<Draft>,
    ledger_opening: Money,
    ledger_rows: Vec<LedgerRow>,
    closed: Vec<String>,
    kpi: Kpi,
    trend: Vec<(String, Money, Money)>,
}

struct NewEntryForm {
    date: String,
    kreis: Kreis,
    description: String,
    account: Option<u32>,
    contra: Option<u32>,
    gross: String,
    tax: Option<TaxKey>,
}

enum Action {
    SetView(View),
    Seed,
    Demo,
    BankBook {
        id: i64,
        account: u32,
        tax: Option<TaxKey>,
    },
    BankImport(String),
    Reverse {
        id: i64,
        reason: String,
    },
    BookGross,
    ApproveDraft(i64),
    DiscardDraft(i64),
    DrainInbox,
    ClosePeriod(String),
}

/// Screenshot mode: render, capture one frame to PNG, close.
pub struct Shot {
    pub path: PathBuf,
    slot: Arc<Mutex<Option<Arc<ColorImage>>>>,
    requested: bool,
}

impl Shot {
    pub fn new(path: PathBuf) -> Shot {
        Shot {
            path,
            slot: Arc::new(Mutex::new(None)),
            requested: false,
        }
    }
}

const TAX_CHOICES: [Option<TaxKey>; 8] = [
    None,
    Some(TaxKey::V20),
    Some(TaxKey::V10),
    Some(TaxKey::V13),
    Some(TaxKey::U20),
    Some(TaxKey::U10),
    Some(TaxKey::U13),
    Some(TaxKey::IGE20),
];

fn tax_label(t: Option<TaxKey>) -> &'static str {
    t.map_or("-", TaxKey::as_str)
}

pub struct BuchApp {
    ctx: Ctx,
    db_label: String,
    pub locale: Locale,
    pub dark: bool,
    pub view: View,
    pub month: MonthSel,
    data: Data,
    dirty: bool,
    status: String,
    status_err: bool,
    bank_pick: HashMap<i64, (Option<u32>, Option<TaxKey>)>,
    storno: Option<(i64, String)>,
    form: NewEntryForm,
    ledger_account: u32,
    import_path: String,
    close_confirm: bool,
    pending: Vec<Action>,
    pub shot: Option<Shot>,
    frame: u64,
    bar: Bar,
    zoom: f32,
}

impl BuchApp {
    pub fn new(ctx: Ctx, db_label: String) -> BuchApp {
        let today = Local::now().date_naive();
        let month = ctx
            .ledger
            .entry_date_span()
            .ok()
            .flatten()
            .map_or(MonthSel::of(today), |(_, last)| MonthSel::of(last));
        let mut app = BuchApp {
            ctx,
            db_label,
            locale: Locale::from_env(),
            dark: false,
            view: View::Overview,
            month,
            data: Data::default(),
            dirty: true,
            status: String::new(),
            status_err: false,
            bank_pick: HashMap::new(),
            storno: None,
            form: NewEntryForm {
                date: format_date_de(today),
                kreis: Kreis::ER,
                description: String::new(),
                account: None,
                contra: Some(2800),
                gross: String::new(),
                tax: Some(TaxKey::V20),
            },
            ledger_account: 2800,
            import_path: String::new(),
            close_confirm: false,
            pending: Vec::new(),
            shot: None,
            bar: Bar::new(),
            zoom: 1.0,
            frame: 0,
        };
        app.reload();
        app
    }

    fn t(&self, de: &'static str, en: &'static str) -> &'static str {
        if self.locale == Locale::De {
            de
        } else {
            en
        }
    }

    fn fail(&mut self, e: impl std::fmt::Display) {
        self.status = e.to_string();
        self.status_err = true;
    }

    fn ok(&mut self, msg: String) {
        self.status = msg;
        self.status_err = false;
    }

    /// Reload everything the views show. Cheap: one local SQLite file.
    pub fn reload(&mut self) {
        self.dirty = false;
        if let Err(e) = self.load() {
            self.fail(e);
        }
    }

    fn load(&mut self) -> Result<(), String> {
        let l = &self.ctx.ledger;
        let e = |e: buchcraft_store::BuchError| e.to_string();
        let (first, last) = (self.month.first(), self.month.last());
        let stats = l.stats().map_err(e)?;
        let mut d = Data {
            company: l
                .meta("company")
                .map_err(e)?
                .unwrap_or_else(|| "BuchCraft".into()),
            demo: l.is_demo().map_err(e)?,
            entries_total: stats["entries"].as_i64().unwrap_or(0),
            accounts: l.accounts().map_err(e)?,
            journal: l.journal(first, last, None, 5000).map_err(e)?,
            bank: l.bank_unmatched(2000).map_err(e)?,
            balances: Some(l.balances(first, last).map_err(e)?),
            u30: Some(l.u30(&self.month.label()).map_err(e)?),
            drafts: l.drafts(true).map_err(e)?,
            closed: l.closed_periods().map_err(e)?,
            ..Data::default()
        };
        let (opening, rows) = match l.account_ledger(self.ledger_account, first, last) {
            Ok(v) => v,
            Err(_) => (Money::ZERO, Vec::new()),
        };
        d.ledger_opening = opening;
        d.ledger_rows = rows;
        // KPIs for the selected month.
        let to_date = l.balances(NaiveDate::MIN, last).map_err(e)?;
        d.kpi.bank = to_date
            .rows
            .iter()
            .find(|r| r.account == 2800)
            .map_or(Money::ZERO, |r| r.saldo);
        let (rev, exp) = rev_exp(d.balances.as_ref())?;
        d.kpi.revenue = rev;
        d.kpi.expense = exp;
        d.kpi.zahllast = d.u30.as_ref().map_or(Money::ZERO, |u| u.zahllast);
        // Six-month trend ending at the selected month.
        let mut m = self.month;
        let mut months = vec![m];
        for _ in 0..5 {
            m = m.prev();
            months.push(m);
        }
        months.reverse();
        for m in months {
            let b = l.balances(m.first(), m.last()).map_err(e)?;
            let (r, x) = rev_exp(Some(&b))?;
            d.trend.push((m.label(), r, x));
        }
        self.data = d;
        Ok(())
    }

    fn exec(&mut self, cmd: Command) -> Result<serde_json::Value, String> {
        let r = dispatch(&mut self.ctx, cmd);
        self.dirty = true;
        r
    }

    fn apply(&mut self, a: Action) {
        let de = self.locale == Locale::De;
        let result = match a {
            Action::SetView(v) => {
                self.view = v;
                return;
            }
            Action::Seed => self.exec(Command::AccountsSeed).map(|v| {
                format!(
                    "{}: {}",
                    if de {
                        "Konten angelegt"
                    } else {
                        "Accounts inserted"
                    },
                    v["inserted"]
                )
            }),
            Action::Demo => self.exec(Command::DemoSeed).map(|_| {
                self.month = MonthSel {
                    year: 2026,
                    month: 10,
                };
                (if de {
                    "Demo-Firma geladen (Oktober 2026)"
                } else {
                    "Demo company loaded (October 2026)"
                })
                .to_string()
            }),
            Action::BankBook { id, account, tax } => {
                self.bank_pick.remove(&id);
                self.exec(Command::BankBook {
                    id,
                    account,
                    tax_key: tax,
                    description: None,
                })
                .map(|v| {
                    format!(
                        "{} {}",
                        if de { "Gebucht" } else { "Booked" },
                        v["beleg_nr"].as_str().unwrap_or("")
                    )
                })
            }
            Action::BankImport(path) => self.exec(Command::BankImport { path }).map(|v| {
                format!(
                    "{}: {} / {}: {}",
                    if de { "Importiert" } else { "Imported" },
                    v["imported"],
                    if de { "Duplikate" } else { "duplicates" },
                    v["duplicates"]
                )
            }),
            Action::Reverse { id, reason } => {
                let today = Local::now().date_naive();
                self.exec(Command::EntryReverse {
                    id,
                    date: today,
                    reason,
                })
                .map(|v| format!("Storno {}", v["beleg_nr"].as_str().unwrap_or("")))
            }
            Action::BookGross => self.book_form(),
            Action::ApproveDraft(id) => self.exec(Command::DraftApprove { id }).map(|v| {
                format!(
                    "{} {}",
                    if de {
                        "Entwurf gebucht"
                    } else {
                        "Draft booked"
                    },
                    v["beleg_nr"].as_str().unwrap_or("")
                )
            }),
            Action::DiscardDraft(id) => self.exec(Command::DraftDiscard { id }).map(|_| {
                (if de {
                    "Entwurf verworfen"
                } else {
                    "Draft discarded"
                })
                .to_string()
            }),
            Action::DrainInbox => self.exec(Command::InboxDrain).map(|v| {
                format!(
                    "{}: {} / {}: {}",
                    if de { "Entwürfe" } else { "Drafts" },
                    v["drafted"].as_array().map_or(0, Vec::len),
                    if de { "abgelehnt" } else { "rejected" },
                    v["rejected"].as_array().map_or(0, Vec::len)
                )
            }),
            Action::ClosePeriod(m) => {
                self.close_confirm = false;
                self.exec(Command::PeriodClose { month: m.clone() })
                    .map(|_| {
                        format!(
                            "{} {m}",
                            if de {
                                "Monat abgeschlossen:"
                            } else {
                                "Month closed:"
                            }
                        )
                    })
            }
        };
        match result {
            Ok(msg) => self.ok(msg),
            Err(e) => self.fail(e),
        }
    }

    fn book_form(&mut self) -> Result<String, String> {
        let f = &self.form;
        let date = parse_date(&f.date).map_err(|e| e.to_string())?;
        let account = f.account.ok_or(if self.locale == Locale::De {
            "Konto wählen"
        } else {
            "choose an account"
        })?;
        let contra = f.contra.ok_or(if self.locale == Locale::De {
            "Gegenkonto wählen"
        } else {
            "choose a contra account"
        })?;
        let gross = Money::parse_de(&f.gross).map_err(|e| e.to_string())?;
        let cmd = Command::EntryBookGross {
            date,
            kreis: f.kreis,
            description: f.description.clone(),
            account,
            contra,
            gross,
            tax_key: f.tax,
            beleg_nr: None,
        };
        let v = self.exec(cmd)?;
        self.form.description.clear();
        self.form.gross.clear();
        Ok(format!(
            "{} {}",
            if self.locale == Locale::De {
                "Gebucht"
            } else {
                "Booked"
            },
            v["beleg_nr"].as_str().unwrap_or("")
        ))
    }

    fn account_label(&self, n: u32) -> String {
        self.data
            .accounts
            .iter()
            .find(|a| a.number == n)
            .map_or_else(
                || format!("{n:04}"),
                |a| format!("{:04} {}", a.number, a.name),
            )
    }

    fn account_combo(
        &self,
        ui: &mut egui::Ui,
        id: impl std::hash::Hash + std::fmt::Debug,
        value: &mut Option<u32>,
        width: f32,
    ) -> bool {
        let mut changed = false;
        let text = value.map_or_else(
            || self.t("Konto...", "Account...").to_string(),
            |n| self.account_label(n),
        );
        egui::ComboBox::from_id_salt(id)
            .selected_text(text)
            .width(width)
            .show_ui(ui, |ui| {
                for a in self.data.accounts.iter().filter(|a| a.active) {
                    if ui
                        .selectable_value(
                            value,
                            Some(a.number),
                            format!("{:04} {}", a.number, a.name),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                }
            });
        changed
    }

    // ---------- chrome ----------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("BuchCraft").strong().size(20.0));
            ui.separator();
            ui.label(RichText::new(&self.data.company).size(15.0));
            if self.data.demo {
                egui::Frame::new()
                    .fill(Color32::from_rgb(214, 120, 30))
                    .corner_radius(4)
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        ui.label(RichText::new("DEMO").color(Color32::WHITE).strong())
                    });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(if self.dark {
                        self.t("Hell", "Light")
                    } else {
                        self.t("Dunkel", "Dark")
                    })
                    .clicked()
                {
                    self.dark = !self.dark;
                }
                if ui
                    .button(if self.locale == Locale::De {
                        "EN"
                    } else {
                        "DE"
                    })
                    .clicked()
                {
                    self.locale = if self.locale == Locale::De {
                        Locale::En
                    } else {
                        Locale::De
                    };
                }
                ui.separator();
                if ui.button(">").clicked() {
                    self.month = self.month.next();
                    self.dirty = true;
                }
                let closed = self.data.closed.contains(&self.month.label());
                let label = if closed {
                    format!(
                        "{} ({})",
                        self.month.label(),
                        self.t("geschlossen", "closed")
                    )
                } else {
                    self.month.label()
                };
                ui.label(RichText::new(label).monospace().strong());
                if ui.button("<").clicked() {
                    self.month = self.month.prev();
                    self.dirty = true;
                }
                ui.label(self.t("Monat:", "Month:"));
            });
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        for v in View::ALL {
            let mut text = v.label(self.locale).to_string();
            if v == View::BankInbox && !self.data.bank.is_empty() {
                text = format!("{text}  ({})", self.data.bank.len());
            }
            if v == View::Drafts && !self.data.drafts.is_empty() {
                text = format!("{text}  ({})", self.data.drafts.len());
            }
            if ui
                .selectable_label(self.view == v, RichText::new(text).size(15.0))
                .clicked()
            {
                self.pending.push(Action::SetView(v));
            }
            ui.add_space(2.0);
        }
    }

    // ---------- views ----------

    fn view_overview(&mut self, ui: &mut egui::Ui) {
        if self.data.entries_total == 0 {
            ui.heading(self.t("Leeres Buch", "Empty ledger"));
            ui.label(self.t(
                "Noch keine Buchungen. Kontenplan anlegen und loslegen, oder die Demo-Firma laden.",
                "No entries yet. Seed the chart of accounts, or load the demo company.",
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .button(self.t("EKR-Kontenplan anlegen", "Seed EKR chart"))
                    .clicked()
                {
                    self.pending.push(Action::Seed);
                }
                if ui
                    .button(self.t("Demo-Firma laden", "Load demo company"))
                    .clicked()
                {
                    self.pending.push(Action::Demo);
                }
            });
            return;
        }
        ui.heading(format!(
            "{} {}",
            self.t("Übersicht", "Overview"),
            self.month.label()
        ));
        ui.add_space(6.0);
        let k = self.data.kpi.clone();
        let cards = [
            (
                self.t("Bank (2800)", "Bank (2800)"),
                k.bank,
                Color32::from_rgb(40, 110, 200),
            ),
            (
                self.t("Erlöse Monat", "Revenue (month)"),
                k.revenue,
                Color32::from_rgb(40, 150, 90),
            ),
            (
                self.t("Aufwand Monat", "Expenses (month)"),
                k.expense,
                Color32::from_rgb(214, 120, 30),
            ),
            (
                self.t("USt-Zahllast", "VAT payable"),
                k.zahllast,
                Color32::from_rgb(170, 60, 60),
            ),
        ];
        ui.horizontal(|ui| {
            for (label, value, color) in cards {
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_min_width(170.0);
                        ui.label(RichText::new(label).color(color).strong());
                        ui.label(RichText::new(value.format_eur()).size(22.0).monospace());
                    });
            }
        });
        ui.add_space(10.0);
        ui.label(
            RichText::new(self.t(
                "Erlöse und Aufwand, letzte 6 Monate",
                "Revenue and expenses, last 6 months",
            ))
            .strong(),
        );
        trend_chart(ui, &self.data.trend);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let n = self.data.bank.len();
            if n > 0
                && ui
                    .button(format!(
                        "{} {n} {}",
                        self.t("Bank-Eingang:", "Bank inbox:"),
                        self.t("offene Zeilen", "open lines")
                    ))
                    .clicked()
            {
                self.pending.push(Action::SetView(View::BankInbox));
            }
            let dn = self.data.drafts.len();
            if dn > 0
                && ui
                    .button(format!(
                        "{} {dn}",
                        self.t("Offene Entwürfe:", "Open drafts:")
                    ))
                    .clicked()
            {
                self.pending.push(Action::SetView(View::Drafts));
            }
            ui.label(format!(
                "{}: {}",
                self.t("Buchungen im Monat", "Entries this month"),
                self.data.journal.len()
            ));
        });
    }

    fn view_bank(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.t(
            "Bank-Eingang: offene Kontoauszugszeilen",
            "Bank inbox: unbooked statement lines",
        ));
        ui.label(self.t(
            "Konto und Steuerschlüssel wählen, dann buchen. Der Betrag ist brutto; die Vorsteuer wird abgespalten.",
            "Choose account and tax key, then book. The amount is gross; the VAT line is split off.",
        ));
        ui.horizontal(|ui| {
            ui.label(self.t("CSV importieren:", "Import CSV:"));
            ui.add(
                egui::TextEdit::singleline(&mut self.import_path)
                    .desired_width(360.0)
                    .hint_text("D:\\Bank\\george-2026-10.csv"),
            );
            if ui
                .add_enabled(
                    !self.import_path.trim().is_empty(),
                    egui::Button::new(self.t("Import", "Import")),
                )
                .clicked()
            {
                self.pending
                    .push(Action::BankImport(self.import_path.trim().to_string()));
            }
        });
        ui.separator();
        if self.data.bank.is_empty() {
            ui.label(self.t("Alles gebucht.", "Everything is booked."));
            return;
        }
        let rows = self.data.bank.clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("bank_grid")
                    .striped(true)
                    .num_columns(7)
                    .spacing([14.0, 8.0])
                    .show(ui, |ui| {
                        for h in [
                            self.t("Datum", "Date"),
                            self.t("Partner", "Partner"),
                            self.t("Text", "Reference"),
                            self.t("Betrag", "Amount"),
                            self.t("Konto", "Account"),
                            self.t("Steuer", "Tax"),
                            "",
                        ] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        for t in &rows {
                            ui.label(format_date_de(t.booking_date));
                            ui.label(t.partner.clone().unwrap_or_default());
                            ui.label(&t.reference);
                            money_label(ui, t.amount);
                            let mut pick =
                                self.bank_pick.get(&t.id).copied().unwrap_or((None, None));
                            if self.account_combo(ui, ("bank_acc", t.id), &mut pick.0, 230.0) {
                                pick.1 = pick
                                    .0
                                    .and_then(|n| self.data.accounts.iter().find(|a| a.number == n))
                                    .and_then(|a| a.default_tax_key);
                            }
                            egui::ComboBox::from_id_salt(("bank_tax", t.id))
                                .selected_text(tax_label(pick.1))
                                .width(70.0)
                                .show_ui(ui, |ui| {
                                    for c in TAX_CHOICES {
                                        ui.selectable_value(&mut pick.1, c, tax_label(c));
                                    }
                                });
                            self.bank_pick.insert(t.id, pick);
                            if ui
                                .add_enabled(
                                    pick.0.is_some(),
                                    egui::Button::new(self.t("Buchen", "Book")),
                                )
                                .clicked()
                            {
                                if let Some(account) = pick.0 {
                                    self.pending.push(Action::BankBook {
                                        id: t.id,
                                        account,
                                        tax: pick.1,
                                    });
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    fn view_new_entry(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.t("Neue Buchung (brutto)", "New entry (gross)"));
        ui.label(self.t(
            "Konto bekommt den Nettobetrag, die Steuerzeile wird erzeugt, das Gegenkonto bekommt minus brutto.",
            "The account gets the net amount, the tax line is generated, the contra account gets minus gross.",
        ));
        ui.add_space(6.0);
        let mut account = self.form.account;
        let mut contra = self.form.contra;
        egui::Grid::new("new_entry_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(self.t("Datum", "Date"));
                ui.add(egui::TextEdit::singleline(&mut self.form.date).desired_width(120.0));
                ui.end_row();
                ui.label(self.t("Belegkreis", "Document cycle"));
                ui.horizontal(|ui| {
                    for k in [Kreis::AR, Kreis::ER, Kreis::BK, Kreis::KA, Kreis::SO] {
                        ui.selectable_value(&mut self.form.kreis, k, k.as_str());
                    }
                });
                ui.end_row();
                ui.label(self.t("Text", "Description"));
                ui.add(egui::TextEdit::singleline(&mut self.form.description).desired_width(360.0));
                ui.end_row();
                ui.label(self.t("Konto", "Account"));
                if self.account_combo(ui, "form_account", &mut account, 300.0) {
                    self.form.tax = account
                        .and_then(|n| self.data.accounts.iter().find(|a| a.number == n))
                        .and_then(|a| a.default_tax_key);
                }
                ui.end_row();
                ui.label(self.t("Gegenkonto", "Contra account"));
                self.account_combo(ui, "form_contra", &mut contra, 300.0);
                ui.end_row();
                ui.label(self.t("Brutto", "Gross"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.form.gross)
                        .desired_width(120.0)
                        .hint_text("1.200,00"),
                );
                ui.end_row();
                ui.label(self.t("Steuer", "Tax key"));
                egui::ComboBox::from_id_salt("form_tax")
                    .selected_text(tax_label(self.form.tax))
                    .show_ui(ui, |ui| {
                        for c in TAX_CHOICES {
                            ui.selectable_value(&mut self.form.tax, c, tax_label(c));
                        }
                    });
                ui.end_row();
            });
        self.form.account = account;
        self.form.contra = contra;
        if let Ok(g) = Money::parse_de(&self.form.gross) {
            let split = match self.form.tax {
                Some(k) if k != TaxKey::IGE20 => Money::split_gross(g, k.rate_pct()).ok(),
                _ => Some((g, Money::ZERO)),
            };
            if let Some((net, tax)) = split {
                ui.label(
                    RichText::new(format!(
                        "{} {}   {} {}",
                        self.t("Netto", "Net"),
                        net.format_eur(),
                        self.t("Steuer", "Tax"),
                        tax.format_eur()
                    ))
                    .monospace(),
                );
            }
        }
        ui.add_space(6.0);
        if ui
            .button(RichText::new(self.t("Buchen", "Book")).strong())
            .clicked()
        {
            self.pending.push(Action::BookGross);
        }
    }

    fn view_journal(&mut self, ui: &mut egui::Ui) {
        ui.heading(format!(
            "{} {}",
            self.t("Journal", "Journal"),
            self.month.label()
        ));
        if self.data.journal.is_empty() {
            ui.label(self.t("Keine Buchungen in diesem Monat.", "No entries this month."));
            return;
        }
        let entries = self.data.journal.clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for e in &entries {
                    let total = Money::sum(
                        e.lines
                            .iter()
                            .filter(|l| !l.amount.is_negative())
                            .map(|l| l.amount),
                    )
                    .unwrap_or(Money::ZERO);
                    let mut head = format!(
                        "{}   {}   {}   {}",
                        e.beleg_nr,
                        format_date_de(e.date),
                        e.description,
                        total.format_de()
                    );
                    if e.reversed_by_id.is_some() {
                        head.push_str(self.t("   [storniert]", "   [reversed]"));
                    }
                    egui::CollapsingHeader::new(RichText::new(head).monospace())
                        .id_salt(("entry", e.id))
                        .show(ui, |ui| {
                            egui::Grid::new(("lines", e.id))
                                .striped(true)
                                .num_columns(4)
                                .show(ui, |ui| {
                                    for l in &e.lines {
                                        ui.label(
                                            RichText::new(format!("{:04}", l.account)).monospace(),
                                        );
                                        ui.label(&l.account_name);
                                        money_label(ui, l.amount);
                                        ui.label(match (&l.tax_key, l.is_tax) {
                                            (Some(k), true) => format!(
                                                "{k} ({})",
                                                self.t("Steuerzeile", "tax line")
                                            ),
                                            (Some(k), false) => k.clone(),
                                            _ => String::new(),
                                        });
                                        ui.end_row();
                                    }
                                });
                            if e.reversed_by_id.is_none() && e.reverses_id.is_none() {
                                let l_reason = self.t("Grund:", "Reason:");
                                let l_book = self.t("Storno buchen", "Book reversal");
                                let l_start = self.t("Stornieren...", "Reverse...");
                                let mut reverse_with: Option<String> = None;
                                let mut start = false;
                                match &mut self.storno {
                                    Some((id, reason)) if *id == e.id => {
                                        ui.horizontal(|ui| {
                                            ui.label(l_reason);
                                            ui.add(
                                                egui::TextEdit::singleline(reason)
                                                    .desired_width(260.0),
                                            );
                                            if ui
                                                .add_enabled(
                                                    !reason.trim().is_empty(),
                                                    egui::Button::new(l_book),
                                                )
                                                .clicked()
                                            {
                                                reverse_with = Some(reason.trim().to_string());
                                            }
                                        });
                                    }
                                    _ => {
                                        start = ui.button(l_start).clicked();
                                    }
                                }
                                if let Some(reason) = reverse_with {
                                    self.storno = None;
                                    self.pending.push(Action::Reverse { id: e.id, reason });
                                }
                                if start {
                                    self.storno = Some((e.id, String::new()));
                                }
                            }
                        });
                }
            });
    }

    fn view_balances(&mut self, ui: &mut egui::Ui) {
        ui.heading(format!(
            "{} {}",
            self.t("Saldenliste", "Trial balance"),
            self.month.label()
        ));
        let Some(b) = self.data.balances.clone() else {
            return;
        };
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("balance_grid")
                    .striped(true)
                    .num_columns(5)
                    .spacing([18.0, 6.0])
                    .show(ui, |ui| {
                        for h in [
                            self.t("Konto", "Account"),
                            self.t("Bezeichnung", "Name"),
                            self.t("Soll", "Debit"),
                            self.t("Haben", "Credit"),
                            self.t("Saldo", "Balance"),
                        ] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        for r in &b.rows {
                            ui.label(RichText::new(format!("{:04}", r.account)).monospace());
                            ui.label(&r.name);
                            ui.label(RichText::new(r.debit.format_de()).monospace());
                            ui.label(RichText::new(r.credit.format_de()).monospace());
                            money_label(ui, r.saldo);
                            ui.end_row();
                        }
                        ui.label("");
                        ui.label(RichText::new(self.t("Summe", "Total")).strong());
                        ui.label(
                            RichText::new(b.total_debit.format_de())
                                .monospace()
                                .strong(),
                        );
                        ui.label(
                            RichText::new(b.total_credit.format_de())
                                .monospace()
                                .strong(),
                        );
                        ui.label(if b.total_debit == b.total_credit {
                            RichText::new("= 0").color(Color32::from_rgb(40, 150, 90))
                        } else {
                            RichText::new("!=").color(Color32::RED)
                        });
                        ui.end_row();
                    });
            });
    }

    fn view_account_ledger(&mut self, ui: &mut egui::Ui) {
        ui.heading(format!(
            "{} {}",
            self.t("Kontoblatt", "Account ledger"),
            self.month.label()
        ));
        let mut acc = Some(self.ledger_account);
        if self.account_combo(ui, "ledger_account", &mut acc, 320.0) {
            if let Some(a) = acc {
                self.ledger_account = a;
                self.dirty = true;
            }
        }
        ui.label(
            RichText::new(format!(
                "{}: {}",
                self.t("Anfangssaldo", "Opening balance"),
                self.data.ledger_opening.format_eur()
            ))
            .monospace(),
        );
        let rows = self.data.ledger_rows.clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("ledger_grid")
                    .striped(true)
                    .num_columns(5)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        for h in [
                            self.t("Datum", "Date"),
                            self.t("Beleg", "Document"),
                            self.t("Text", "Description"),
                            self.t("Betrag", "Amount"),
                            self.t("Saldo", "Balance"),
                        ] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        for r in &rows {
                            ui.label(format_date_de(r.date));
                            ui.label(RichText::new(&r.beleg_nr).monospace());
                            ui.label(&r.description);
                            money_label(ui, r.amount);
                            ui.label(RichText::new(r.running.format_de()).monospace().strong());
                            ui.end_row();
                        }
                    });
            });
    }

    fn view_u30(&mut self, ui: &mut egui::Ui) {
        ui.heading(format!(
            "{} {}",
            self.t(
                "Umsatzsteuervoranmeldung (U30)",
                "VAT return pre-fill (U30)"
            ),
            self.month.label()
        ));
        let Some(u) = self.data.u30.clone() else {
            return;
        };
        egui::Grid::new("u30_grid")
            .striped(true)
            .num_columns(3)
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                for h in [
                    "KZ",
                    self.t("Bezeichnung", "Field"),
                    self.t("Betrag", "Amount"),
                ] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for (kz, v) in &u.kz {
                    let strong = kz == "095";
                    ui.label(RichText::new(kz).monospace().strong());
                    ui.label(kz_label(kz, self.locale));
                    let t = RichText::new(v.format_eur()).monospace();
                    ui.label(if strong { t.strong().size(17.0) } else { t });
                    ui.end_row();
                }
            });
        ui.add_space(6.0);
        ui.label(RichText::new(self.t(
            "Vorbefüllung aus den Buchungen. Keine Abgabe: vor dem Einreichen in FinanzOnline prüfen. Positive KZ 095 = Zahllast.",
            "Pre-filled from booked entries. Not a filing: check before submitting in FinanzOnline. Positive KZ 095 = payable.",
        )).weak());
        ui.add_space(10.0);
        let label = self.month.label();
        if self.data.closed.contains(&label) {
            ui.label(
                RichText::new(self.t("Dieser Monat ist abgeschlossen.", "This month is closed."))
                    .strong(),
            );
        } else {
            let confirm_label = self.t(
                "Ich will diesen Monat abschließen (nicht umkehrbar)",
                "I want to close this month (cannot be undone)",
            );
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.close_confirm, confirm_label);
                if ui
                    .add_enabled(
                        self.close_confirm,
                        egui::Button::new(self.t("Monat abschließen", "Close month")),
                    )
                    .clicked()
                {
                    self.pending.push(Action::ClosePeriod(label.clone()));
                }
            });
        }
    }

    fn view_drafts(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.t(
            "Entwürfe aus anderen Craft-Apps",
            "Drafts from other craft apps",
        ));
        ui.horizontal(|ui| {
            if ui
                .button(self.t("Eingang abholen", "Fetch inbox"))
                .clicked()
            {
                self.pending.push(Action::DrainInbox);
            }
            ui.label(
                RichText::new(self.ctx.bus.root().display().to_string())
                    .weak()
                    .monospace(),
            );
        });
        ui.separator();
        if self.data.drafts.is_empty() {
            ui.label(self.t("Keine offenen Entwürfe.", "No open drafts."));
            return;
        }
        let drafts = self.data.drafts.clone();
        for d in &drafts {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(
                    RichText::new(format!(
                        "{}  {}  {}",
                        d.source,
                        format_date_de(d.input.date),
                        d.input.description
                    ))
                    .strong(),
                );
                for l in &d.input.lines {
                    ui.label(
                        RichText::new(format!(
                            "{}   {}",
                            self.account_label(l.account),
                            l.amount.format_de()
                        ))
                        .monospace(),
                    );
                }
                ui.horizontal(|ui| {
                    if ui.button(self.t("Buchen", "Book")).clicked() {
                        self.pending.push(Action::ApproveDraft(d.id));
                    }
                    if ui.button(self.t("Verwerfen", "Discard")).clicked() {
                        self.pending.push(Action::DiscardDraft(d.id));
                    }
                });
            });
        }
    }

    fn handle_screenshot(&mut self, ui: &mut egui::Ui) {
        let Some(shot) = &mut self.shot else { return };
        ui.ctx().request_repaint();
        if !shot.requested && self.frame >= 8 {
            let slot = Arc::clone(&shot.slot);
            ui.ctx().request_screenshot(move |img| {
                *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(img);
            });
            shot.requested = true;
        }
        let img = shot
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(img) = img {
            let [w, h] = img.size;
            let res = image::save_buffer(
                &shot.path,
                img.as_raw(),
                w as u32,
                h as u32,
                image::ColorType::Rgba8,
            );
            match res {
                Ok(()) => eprintln!("[buchcraft-app] screenshot saved: {}", shot.path.display()),
                Err(e) => eprintln!("[buchcraft-app] screenshot failed: {e}"),
            }
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn rev_exp(b: Option<&Balances>) -> Result<(Money, Money), String> {
    let Some(b) = b else {
        return Ok((Money::ZERO, Money::ZERO));
    };
    let mut rev = Money::ZERO;
    let mut exp = Money::ZERO;
    for r in &b.rows {
        match r.kind {
            AccountKind::Revenue => rev = rev.checked_sub(r.saldo).map_err(|e| e.to_string())?,
            AccountKind::Expense => exp = exp.checked_add(r.saldo).map_err(|e| e.to_string())?,
            _ => {}
        }
    }
    Ok((rev, exp))
}

fn money_label(ui: &mut egui::Ui, m: Money) {
    let t = RichText::new(m.format_de()).monospace();
    ui.label(if m.is_negative() {
        t.color(Color32::from_rgb(190, 60, 60))
    } else {
        t
    });
}

fn kz_label(kz: &str, l: Locale) -> &'static str {
    let de = l == Locale::De;
    match kz {
        "000" => {
            if de {
                "Gesamtbetrag Lieferungen und Leistungen"
            } else {
                "Total supplies and services"
            }
        }
        "006" => {
            if de {
                "davon 13 %"
            } else {
                "of which 13 %"
            }
        }
        "022" => {
            if de {
                "davon 20 %"
            } else {
                "of which 20 %"
            }
        }
        "029" => {
            if de {
                "davon 10 %"
            } else {
                "of which 10 %"
            }
        }
        "060" => {
            if de {
                "Vorsteuer"
            } else {
                "Input VAT"
            }
        }
        "065" => {
            if de {
                "Vorsteuer aus ig. Erwerb"
            } else {
                "Input VAT, intra-EU acquisitions"
            }
        }
        "070" => {
            if de {
                "ig. Erwerbe gesamt"
            } else {
                "Intra-EU acquisitions, total"
            }
        }
        "072" => {
            if de {
                "ig. Erwerbe 20 %"
            } else {
                "Intra-EU acquisitions 20 %"
            }
        }
        "095" => {
            if de {
                "Zahllast (+) / Gutschrift (-)"
            } else {
                "Payable (+) / credit (-)"
            }
        }
        _ => "",
    }
}

/// Grouped bars per month: revenue (green) and expenses (orange).
fn trend_chart(ui: &mut egui::Ui, trend: &[(String, Money, Money)]) {
    let width = ui.available_width().clamp(300.0, 900.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 200.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let stroke_color = ui.visuals().weak_text_color();
    let text_color = ui.visuals().text_color();
    let max = trend
        .iter()
        .flat_map(|(_, r, e)| [r.cents().max(0), e.cents().max(0)])
        .max()
        .unwrap_or(0)
        .max(1) as f32;
    let base_y = rect.bottom() - 22.0;
    let plot_h = rect.height() - 40.0;
    painter.line_segment(
        [
            egui::pos2(rect.left(), base_y),
            egui::pos2(rect.right(), base_y),
        ],
        egui::Stroke::new(1.0, stroke_color),
    );
    let n = trend.len().max(1) as f32;
    let slot = rect.width() / n;
    let bar_w = (slot * 0.28).min(36.0);
    let green = Color32::from_rgb(40, 150, 90);
    let orange = Color32::from_rgb(214, 120, 30);
    for (i, (label, rev, exp)) in trend.iter().enumerate() {
        let cx = rect.left() + slot * (i as f32 + 0.5);
        for (j, (v, c)) in [(*rev, green), (*exp, orange)].into_iter().enumerate() {
            let h = (v.cents().max(0) as f32 / max) * plot_h;
            let x0 = cx + if j == 0 { -bar_w - 2.0 } else { 2.0 };
            let r = egui::Rect::from_min_max(
                egui::pos2(x0, base_y - h),
                egui::pos2(x0 + bar_w, base_y),
            );
            painter.rect_filled(r, 2.0, c);
        }
        painter.text(
            egui::pos2(cx, base_y + 4.0),
            egui::Align2::CENTER_TOP,
            label,
            egui::FontId::monospace(12.0),
            text_color,
        );
    }
    painter.text(
        rect.left_top() + egui::vec2(4.0, 2.0),
        egui::Align2::LEFT_TOP,
        "max ".to_string() + &Money::from_cents(max as i64).format_eur(),
        egui::FontId::proportional(12.0),
        stroke_color,
    );
}

impl BuchApp {
    fn paint_menu(&mut self, ui: &mut egui::Ui) {
        let docs = bookend::repo_docs(env!("CARGO_MANIFEST_DIR"));
        let spec = Spec {
            name: "BuchCraft",
            version: env!("CARGO_PKG_VERSION"),
            description: "The ledger, the bank inbox, and the VAT worksheet.",
            repository: "https://github.com/sandraschi/buchcraft",
            docs: &docs,
            dark: self.dark,
            zoom: self.zoom,
            file: Vec::new(),
            view: Vec::new(),
            middle: Vec::new(),
        };
        let change = self.bar.show(ui, &spec);
        if let Some(dark) = change.dark {
            self.dark = dark;
        }
        if let Some(zoom) = change.zoom {
            self.zoom = zoom;
        }
    }
}

impl eframe::App for BuchApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame += 1;
        if self.dirty {
            self.reload();
        }
        ui.ctx().set_visuals(if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });

        self.paint_menu(ui);
        egui::Panel::top("buch_top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::bottom("buch_status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let msg = if self.status.is_empty() {
                    self.t("Bereit.", "Ready.").to_string()
                } else {
                    self.status.clone()
                };
                let t = RichText::new(msg).monospace();
                ui.label(if self.status_err {
                    t.color(Color32::from_rgb(190, 60, 60))
                } else {
                    t.weak()
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("v{}  {}", buchcraft_core::VERSION, self.db_label))
                            .weak()
                            .monospace(),
                    );
                });
            });
        });
        egui::Panel::left("buch_nav")
            .min_size(190.0)
            .show(ui, |ui| self.nav(ui));
        egui::CentralPanel::default().show(ui, |ui| match self.view {
            View::Overview => self.view_overview(ui),
            View::BankInbox => self.view_bank(ui),
            View::NewEntry => self.view_new_entry(ui),
            View::Journal => self.view_journal(ui),
            View::Balances => self.view_balances(ui),
            View::AccountLedger => self.view_account_ledger(ui),
            View::U30 => self.view_u30(ui),
            View::Drafts => self.view_drafts(ui),
        });

        let actions = std::mem::take(&mut self.pending);
        for a in actions {
            self.apply(a);
        }
        self.handle_screenshot(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buchcraft_core::Confinement;
    use buchcraft_store::Ledger;
    use craft_biz::bus::Bus;
    use craft_biz_ui::bookend::{self, Bar, Spec};

    fn app() -> BuchApp {
        let ctx = Ctx {
            ledger: Ledger::open_in_memory().unwrap(),
            confinement: Confinement(None),
            bus: Bus::new(
                std::env::temp_dir().join(format!("buchcraft-app-test-{}", std::process::id())),
            ),
        };
        BuchApp::new(ctx, "memory".into())
    }

    #[test]
    fn empty_then_demo_then_bank_booking() {
        let mut a = app();
        assert_eq!(a.data.entries_total, 0);
        a.apply(Action::Demo);
        a.reload();
        assert!(a.data.demo);
        assert_eq!(a.month.label(), "2026-10");
        assert_eq!(a.data.bank.len(), 2);
        assert_eq!(a.data.kpi.zahllast, Money::parse_de("532,00").unwrap());
        assert_eq!(a.data.kpi.revenue, Money::parse_de("4.000,00").unwrap());
        // Book the Wiener Linien line through the same path the button uses.
        let id = a
            .data
            .bank
            .iter()
            .find(|t| t.reference == "Jahreskarte")
            .map(|t| t.id)
            .unwrap();
        a.apply(Action::BankBook {
            id,
            account: 7600,
            tax: Some(TaxKey::V10),
        });
        a.reload();
        assert!(!a.status_err, "{}", a.status);
        assert_eq!(a.data.bank.len(), 1);
    }

    #[test]
    fn form_errors_are_shown_not_panicked() {
        let mut a = app();
        a.apply(Action::Seed);
        a.form.gross = "abc".into();
        a.form.account = Some(7600);
        a.apply(Action::BookGross);
        assert!(a.status_err);
        a.form.gross = "12,00".into();
        a.form.description = "Papier".into();
        a.apply(Action::BookGross);
        assert!(!a.status_err, "{}", a.status);
    }

    #[test]
    fn views_and_months_parse() {
        for v in View::ALL {
            assert!(!v.label(Locale::De).is_empty());
        }
        assert_eq!(View::parse("u30"), Some(View::U30));
        assert_eq!(View::parse("bank"), Some(View::BankInbox));
        let m = MonthSel::parse("2026-12").unwrap();
        assert_eq!(m.next().label(), "2027-01");
        assert_eq!(m.prev().label(), "2026-11");
        assert_eq!(
            MonthSel::parse("2026-02").unwrap().last(),
            NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()
        );
        assert!(MonthSel::parse("2026-13").is_none());
    }
}
