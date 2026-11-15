use buchcraft_core::llm::{
    append_chat_transcript, build_preprompt, call_llm_turn, execute_tool_call, is_read_only_tool,
    load_llm_config, ChatMessage, LlmConfig, LlmTurnOutput, PendingToolCall,
};
use buchcraft_core::Ctx;
use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, RichText, ScrollArea, Stroke, Ui};
use std::sync::mpsc::{self, Receiver};

pub enum ChatWorkerMsg {
    TurnDone(Result<LlmTurnOutput, String>),
}

pub struct ChatDockState {
    pub open: bool,
    pub maximized: bool,
    pub cfg: LlmConfig,
    pub input: String,
    pub messages: Vec<ChatMessage>,
    pub pending_confirm: Option<PendingToolCall>,
    pub last_error: Option<String>,
    pub show_prompt_modal: bool,
    pub open_settings_requested: bool,
    pub last_preprompt: String,
    pub last_tokens: usize,
    pub busy: bool,
    pub rx: Option<Receiver<ChatWorkerMsg>>,
    pub tool_rounds: u8,
}

impl Default for ChatDockState {
    fn default() -> Self {
        Self {
            open: false,
            maximized: false,
            cfg: load_llm_config(),
            input: String::new(),
            messages: vec![ChatMessage {
                role: "assistant".into(),
                content: "Hallo! Ich bin der BuchCraft-Berater (mit EKR/UGB-Skill). Frag mich z.B.:\n- 'Was ist eine Abschlussbuchung?'\n- 'Auf welches Konto buche ich Büromaterial mit 20% VSt?'\n- 'Zeig mir den aktuellen Status und offene Bankzeilen.'\n- 'Wie funktioniert die UVA (U30) in BuchCraft?'".into(),
                tool_name: None,
                tool_args: None,
                tool_result: None,
                is_error: false,
            }],
            pending_confirm: None,
            last_error: None,
            show_prompt_modal: false,
            open_settings_requested: false,
            last_preprompt: String::new(),
            last_tokens: 0,
            busy: false,
            rx: None,
            tool_rounds: 0,
        }
    }
}

impl ChatDockState {
    pub fn poll(&mut self, ctx: &mut Ctx, active_view: &str, mut on_mutated: impl FnMut()) {
        let Some(rx) = self.rx.as_ref() else {
            return;
        };
        if let Ok(ChatWorkerMsg::TurnDone(res)) = rx.try_recv() {
            self.busy = false;
            self.rx = None;
            match res {
                Ok(LlmTurnOutput::AssistantText(txt)) => {
                    self.last_error = None;
                    self.tool_rounds = 0;
                    let msg = ChatMessage {
                        role: "assistant".into(),
                        content: txt,
                        tool_name: None,
                        tool_args: None,
                        tool_result: None,
                        is_error: false,
                    };
                    if self.cfg.save_transcripts {
                        append_chat_transcript(&msg);
                    }
                    self.messages.push(msg);
                }
                Ok(LlmTurnOutput::ToolCalls(tcs)) => {
                    if let Some(tc) = tcs.into_iter().next() {
                        if self.tool_rounds >= 6 {
                            self.last_error = Some("Maximale Tool-Runden (6) erreicht.".into());
                            self.tool_rounds = 0;
                            return;
                        }
                        if is_read_only_tool(&tc.name) {
                            self.run_tool_and_continue(ctx, active_view, &tc, &mut on_mutated);
                        } else {
                            self.pending_confirm = Some(tc);
                        }
                    }
                }
                Err(e) => {
                    self.last_error = Some(e);
                }
            }
        }
    }

    fn run_tool_and_continue(
        &mut self,
        ctx: &mut Ctx,
        active_view: &str,
        tc: &PendingToolCall,
        on_mutated: &mut impl FnMut(),
    ) {
        self.tool_rounds += 1;
        let (out, is_err) = match execute_tool_call(ctx, &tc.name, &tc.arguments) {
            Ok(s) => {
                if !tc.read_only {
                    on_mutated();
                }
                (s, false)
            }
            Err(e) => (format!("{{\"error\": \"{e}\"}}"), true),
        };
        let msg = ChatMessage {
            role: "tool".into(),
            content: format!("[Tool {} Ergebnis]: {}", tc.name, out),
            tool_name: Some(tc.name.clone()),
            tool_args: Some(tc.arguments.clone()),
            tool_result: Some(out),
            is_error: is_err,
        };
        if self.cfg.save_transcripts {
            append_chat_transcript(&msg);
        }
        self.messages.push(msg);
        self.spawn_turn(ctx, active_view);
    }

    pub fn send_user_message(&mut self, ctx: &Ctx, active_view: &str) {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.busy {
            return;
        }
        self.input.clear();
        self.last_error = None;
        self.tool_rounds = 0;
        let msg = ChatMessage {
            role: "user".into(),
            content: text,
            tool_name: None,
            tool_args: None,
            tool_result: None,
            is_error: false,
        };
        if self.cfg.save_transcripts {
            append_chat_transcript(&msg);
        }
        self.messages.push(msg);
        self.spawn_turn(ctx, active_view);
    }

    fn spawn_turn(&mut self, ctx: &Ctx, active_view: &str) {
        let prep = build_preprompt(ctx, active_view, &self.cfg.user_extra_prompt);
        self.last_preprompt = prep.full_text.clone();
        self.last_tokens = prep.estimated_tokens;
        let cfg = self.cfg.clone();
        let history = self.messages.clone();

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.busy = true;
        std::thread::spawn(move || {
            let res = call_llm_turn(&cfg, &prep.full_text, &history);
            let _ = tx.send(ChatWorkerMsg::TurnDone(res));
        });
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        ctx: &mut Ctx,
        active_view: &str,
        mut on_mutated: impl FnMut(),
    ) {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        ui.horizontal(|ui| {
            ui.heading("BuchCraft Berater");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("X").clicked() {
                    self.open = false;
                }
                if ui
                    .small_button(if self.maximized { "Normal" } else { "Max" })
                    .clicked()
                {
                    self.maximized = !self.maximized;
                }
                if ui.small_button("Clear").clicked() {
                    self.messages.truncate(1);
                    self.last_error = None;
                    self.pending_confirm = None;
                }
            });
        });

        ui.horizontal_wrapped(|ui| {
            let model_txt = if self.cfg.model.trim().is_empty() {
                "Auto / Settings..."
            } else {
                self.cfg.model.as_str()
            };
            let pill = format!("{} | {}", self.cfg.provider.label(), model_txt);
            if ui.small_button(pill).clicked() {
                self.open_settings_requested = true;
            }
            ui.checkbox(&mut self.cfg.tools_enabled, "Tools");
            if ui.small_button("View prompt").clicked() {
                let prep = build_preprompt(ctx, active_view, &self.cfg.user_extra_prompt);
                self.last_preprompt = prep.full_text;
                self.last_tokens = prep.estimated_tokens;
                self.show_prompt_modal = true;
            }
            ui.label(
                RichText::new(format!("~{} tok / 16k", self.last_tokens))
                    .small()
                    .weak(),
            );
        });

        if !self.cfg.provider.is_local(&self.cfg.base_url) {
            ui.colored_label(
                Color32::from_rgb(210, 140, 40),
                format!(
                    "Hinweis: Chat sendet Buchungsdaten an {}.",
                    self.cfg.provider.label()
                ),
            );
        }

        ui.separator();

        let dark = ui.visuals().dark_mode;
        let (card_bg, card_alt, border, user_col, tool_col, assistant_col) = if dark {
            (
                Color32::from_rgb(30, 34, 44),
                Color32::from_rgb(24, 28, 36),
                Color32::from_rgb(65, 75, 95),
                Color32::from_rgb(130, 190, 255),
                Color32::from_rgb(110, 210, 150),
                Color32::from_rgb(235, 240, 248),
            )
        } else {
            (
                Color32::from_rgb(246, 248, 252),
                Color32::from_rgb(238, 244, 238),
                Color32::from_rgb(205, 212, 224),
                Color32::from_rgb(24, 90, 180),
                Color32::from_rgb(25, 125, 65),
                Color32::from_rgb(20, 24, 32),
            )
        };

        ScrollArea::vertical()
            .id_salt("buch_chat_scroll")
            .max_height((ui.available_height() - 115.0).max(120.0))
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for m in &self.messages {
                    let (prefix, color, fill) = match m.role.as_str() {
                        "user" => ("Du", user_col, card_alt),
                        "tool" => ("Tool", tool_col, card_alt),
                        _ => ("BuchCraft", assistant_col, card_bg),
                    };
                    Frame::new()
                        .fill(fill)
                        .stroke(Stroke::new(1.0, border))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::same(8))
                        .show(ui, |ui| {
                            ui.label(RichText::new(prefix).size(11.5).strong().color(color));
                            ui.label(&m.content);
                        });
                    ui.add_space(5.0);
                }

                if let Some(tc) = self.pending_confirm.clone() {
                    Frame::group(ui.style()).show(ui, |ui| {
                        ui.label(
                            RichText::new(format!("Schreib-Aktion bestätigen: {}", tc.name))
                                .strong()
                                .color(Color32::from_rgb(220, 150, 40)),
                        );
                        ui.label(tc.arguments.to_string());
                        ui.horizontal(|ui| {
                            if ui.button("Allow once").clicked() {
                                self.pending_confirm = None;
                                self.run_tool_and_continue(ctx, active_view, &tc, &mut on_mutated);
                            }
                            if ui.button("Deny").clicked() {
                                self.pending_confirm = None;
                                self.messages.push(ChatMessage {
                                    role: "tool".into(),
                                    content: "Vom Benutzer abgelehnt.".into(),
                                    tool_name: Some(tc.name),
                                    tool_args: None,
                                    tool_result: None,
                                    is_error: false,
                                });
                            }
                        });
                    });
                }

                if let Some(err) = self.last_error.clone() {
                    Frame::group(ui.style()).show(ui, |ui| {
                        ui.colored_label(Color32::from_rgb(210, 60, 60), format!("Fehler: {err}"));
                        ui.horizontal(|ui| {
                            if ui.small_button("Retry").clicked() {
                                self.last_error = None;
                                self.spawn_turn(ctx, active_view);
                            }
                            if ui.small_button("LLM & Settings").clicked() {
                                self.open_settings_requested = true;
                            }
                        });
                    });
                }
            });

        ui.separator();
        if self.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Denke nach...");
            });
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let send_clicked = ui
                .add_enabled(!self.busy, egui::Button::new("Senden"))
                .clicked();
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .hint_text("Frage zur Buchhaltung, Abschlussbuchung, EKR, U30...")
                    .desired_width(ui.available_width()),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (send_clicked || enter) && !self.busy {
                self.send_user_message(ctx, active_view);
            }
        });

        if self.show_prompt_modal {
            egui::Window::new("Chat Preprompt (Skills + Live Status)")
                .open(&mut self.show_prompt_modal)
                .default_width(600.0)
                .show(ui.ctx(), |ui| {
                    ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                        ui.monospace(&self.last_preprompt);
                    });
                });
        }
    }
}
