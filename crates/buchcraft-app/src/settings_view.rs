use crate::chat_dock::ChatDockState;
use buchcraft_core::llm::{
    discover_llms, resolve_api_key, save_llm_config, test_llm_connection, DiscoveryReport,
    LlmProvider,
};
use eframe::egui::{self, Color32, Frame, RichText, ScrollArea, Ui};
use std::sync::mpsc::{self, Receiver};

pub enum SettingsWorkerMsg {
    ScanDone(DiscoveryReport),
    TestDone(Result<String, String>),
}

#[derive(Default)]
pub struct SettingsState {
    pub report: Option<DiscoveryReport>,
    pub scanning: bool,
    pub testing: bool,
    pub test_result: Option<Result<String, String>>,
    pub save_msg: String,
    pub rx: Option<Receiver<SettingsWorkerMsg>>,
    pub auto_scanned: bool,
}

impl SettingsState {
    pub fn start_scan(&mut self, ollama_url: &str, lm_url: &str) {
        if self.scanning || self.testing {
            return;
        }
        let o_url = ollama_url.to_string();
        let l_url = lm_url.to_string();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.scanning = true;
        std::thread::spawn(move || {
            let rep = discover_llms(&o_url, &l_url);
            let _ = tx.send(SettingsWorkerMsg::ScanDone(rep));
        });
    }

    pub fn start_test(&mut self, chat: &ChatDockState) {
        if self.scanning || self.testing {
            return;
        }
        let cfg = chat.cfg.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.testing = true;
        self.test_result = None;
        std::thread::spawn(move || {
            let res = test_llm_connection(&cfg);
            let _ = tx.send(SettingsWorkerMsg::TestDone(res));
        });
    }

    pub fn poll(&mut self, chat: &mut ChatDockState) {
        if !self.auto_scanned {
            self.auto_scanned = true;
            let o_url = if chat.cfg.provider == LlmProvider::Ollama {
                chat.cfg.base_url.as_str()
            } else {
                "http://127.0.0.1:11434"
            };
            let l_url = if chat.cfg.provider == LlmProvider::LmStudio {
                chat.cfg.base_url.as_str()
            } else {
                "http://127.0.0.1:1234"
            };
            self.start_scan(o_url, l_url);
        }

        let Some(rx) = self.rx.as_ref() else {
            return;
        };
        if let Ok(msg) = rx.try_recv() {
            self.rx = None;
            match msg {
                SettingsWorkerMsg::ScanDone(rep) => {
                    self.scanning = false;
                    if chat.cfg.model.trim().is_empty() {
                        if let Some(prov) = rep.recommended_provider {
                            chat.cfg.provider = prov;
                            chat.cfg.base_url = prov.default_base_url().to_string();
                        }
                        if let Some(m) = &rep.recommended_model {
                            chat.cfg.model = m.clone();
                            let _ = save_llm_config(&chat.cfg);
                        }
                    }
                    self.report = Some(rep);
                }
                SettingsWorkerMsg::TestDone(res) => {
                    self.testing = false;
                    self.test_result = Some(res);
                }
            }
        }
    }
}

pub fn ui_llm_settings(ui: &mut Ui, st: &mut SettingsState, chat: &mut ChatDockState) {
    ui.heading("LLM & Chat Einstellungen (Discovery, BYOK & Agentic DE)");
    ui.label(
        RichText::new(
            "Kein blindes 'llama3' - scannt echte lokale Modelle (Ollama / LM Studio), nutzt Env-Keys oder BYOK, und zeigt die MCP-Anbindung für Agentic IDEs.",
        )
        .weak(),
    );
    ui.add_space(6.0);

    ScrollArea::vertical().show(ui, |ui| {
        ui.columns(2, |cols| {
            Frame::group(cols[0].style()).show(&mut cols[0], |ui| {
                ui.horizontal(|ui| {
                    ui.heading("1. Lokale LLMs & Env-Keys");
                    if ui
                        .add_enabled(
                            !st.scanning && !st.testing,
                            egui::Button::new(if st.scanning {
                                "Scanne..."
                            } else {
                                "Jetzt scannen"
                            }),
                        )
                        .clicked()
                    {
                        st.start_scan("http://127.0.0.1:11434", "http://127.0.0.1:1234");
                    }
                });
                ui.separator();

                if let Some(rep) = &st.report {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Ollama:").strong());
                        if rep.ollama_online {
                            ui.colored_label(
                                Color32::from_rgb(70, 180, 110),
                                format!("ONLINE ({} Modelle)", rep.ollama_models.len()),
                            );
                        } else {
                            ui.colored_label(Color32::from_rgb(220, 150, 50), "OFFLINE");
                        }
                    });
                    if let Some(err) = &rep.ollama_error {
                        ui.label(RichText::new(err).small().weak());
                    } else {
                        ui.label(RichText::new("Klicke ein Modell zum Auswählen:").small().weak());
                        ScrollArea::vertical()
                            .id_salt("buch_ollama_model_list")
                            .max_height(190.0)
                            .show(ui, |ui| {
                                for m in &rep.ollama_models {
                                    let selected = chat.cfg.provider == LlmProvider::Ollama
                                        && chat.cfg.model == m.name;
                                    let mut badges = Vec::new();
                                    if m.is_loaded {
                                        badges.push("VRAM".to_string());
                                    }
                                    if !m.parameter_size.is_empty() {
                                        badges.push(m.parameter_size.clone());
                                    }
                                    if m.supports_tools {
                                        badges.push("tools".to_string());
                                    }
                                    if m.is_embedding {
                                        badges.push("embed".to_string());
                                    }
                                    if m.size_gb > 0.0 {
                                        badges.push(format!("{:.1} GB", m.size_gb));
                                    }
                                    let label = if badges.is_empty() {
                                        m.name.clone()
                                    } else {
                                        format!("{} [{}]", m.name, badges.join(" | "))
                                    };
                                    if ui
                                        .add_enabled(
                                            !m.is_embedding,
                                            egui::Button::selectable(selected, label),
                                        )
                                        .clicked()
                                    {
                                        chat.cfg.provider = LlmProvider::Ollama;
                                        chat.cfg.base_url = rep.ollama_url.clone();
                                        chat.cfg.model = m.name.clone();
                                        st.save_msg.clear();
                                    }
                                }
                            });
                    }

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("LM Studio:").strong());
                        if rep.lm_studio_online {
                            ui.colored_label(
                                Color32::from_rgb(70, 180, 110),
                                format!("ONLINE ({} Modelle)", rep.lm_studio_models.len()),
                            );
                        } else {
                            ui.label(RichText::new("OFFLINE").weak());
                        }
                    });
                    if rep.lm_studio_online {
                        for m in &rep.lm_studio_models {
                            let selected = chat.cfg.provider == LlmProvider::LmStudio
                                && chat.cfg.model == m.name;
                            if ui.selectable_label(selected, &m.name).clicked() {
                                chat.cfg.provider = LlmProvider::LmStudio;
                                chat.cfg.base_url = rep.lm_studio_url.clone();
                                chat.cfg.model = m.name.clone();
                            }
                        }
                    }

                    ui.add_space(6.0);
                    ui.label(RichText::new("Environment API-Keys:").strong());
                    ui.horizontal_wrapped(|ui| {
                        for (k, ok) in &rep.env_keys {
                            if *ok {
                                ui.colored_label(
                                    Color32::from_rgb(70, 180, 110),
                                    format!("[{k}: SET]"),
                                );
                            } else {
                                ui.label(RichText::new(format!("[{k}: -]")).small().weak());
                            }
                        }
                    });
                } else {
                    ui.spinner();
                    ui.label("Suche lokale Modelle und Environment-Keys...");
                }
            });

            Frame::group(cols[1].style()).show(&mut cols[1], |ui| {
                ui.heading("2. Aktiver Provider & BYOK");
                ui.separator();

                ui.horizontal_wrapped(|ui| {
                    for p in LlmProvider::ALL {
                        if ui
                            .selectable_label(chat.cfg.provider == p, p.label())
                            .clicked()
                        {
                            chat.cfg.provider = p;
                            chat.cfg.base_url = p.default_base_url().to_string();
                            let hint = p.default_model_hint();
                            if !hint.is_empty() {
                                chat.cfg.model = hint.to_string();
                            }
                            st.test_result = None;
                        }
                    }
                });

                if chat.cfg.provider == LlmProvider::OpenAiCompat {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Presets:").small());
                        if ui.small_button("OpenAI").clicked() {
                            chat.cfg.base_url = "https://api.openai.com".into();
                            chat.cfg.model = "gpt-4o-mini".into();
                        }
                        if ui.small_button("OpenRouter").clicked() {
                            chat.cfg.base_url = "https://openrouter.ai/api".into();
                            chat.cfg.model = "google/gemini-2.5-flash".into();
                        }
                        if ui.small_button("Groq").clicked() {
                            chat.cfg.base_url = "https://api.groq.com/openai".into();
                            chat.cfg.model = "llama-3.3-70b-versatile".into();
                        }
                    });
                } else if chat.cfg.provider == LlmProvider::Gemini {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Gemini Modelle:").small());
                        for m in ["gemini-2.5-flash", "gemini-2.5-pro", "gemini-2.0-flash"] {
                            if ui.small_button(m).clicked() {
                                chat.cfg.model = m.into();
                            }
                        }
                    });
                }

                ui.add_space(4.0);
                egui::Grid::new("buch_llm_settings_grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Base URL:");
                        ui.add(
                            egui::TextEdit::singleline(&mut chat.cfg.base_url)
                                .desired_width(260.0),
                        );
                        ui.end_row();

                        ui.label("Model:");
                        ui.add(
                            egui::TextEdit::singleline(&mut chat.cfg.model).desired_width(260.0),
                        );
                        ui.end_row();

                        ui.label("API Key (BYOK):");
                        ui.add(
                            egui::TextEdit::singleline(&mut chat.cfg.api_key)
                                .password(true)
                                .hint_text("leer = Env-Key nutzen (falls gesetzt)")
                                .desired_width(260.0),
                        );
                        ui.end_row();
                    });

                let key_status = if chat.cfg.provider.is_local(&chat.cfg.base_url) {
                    "Lokal - kein API-Key erforderlich."
                } else if !chat.cfg.api_key.trim().is_empty() {
                    "BYOK-Key eingetragen."
                } else if resolve_api_key(&chat.cfg).is_some() {
                    "Env-Key automatisch erkannt (leer lassen ok)."
                } else {
                    "Kein API-Key gefunden - bitte BYOK eingeben oder Env-Key setzen."
                };
                ui.label(RichText::new(key_status).small().weak());

                ui.add_space(6.0);
                ui.checkbox(&mut chat.cfg.tools_enabled, "Tool-Calling im Chat aktivieren");
                ui.checkbox(
                    &mut chat.cfg.save_transcripts,
                    "Chat-Transkripte lokal speichern (.jsonl)",
                );
                ui.label("Extra System-Hinweis:");
                ui.text_edit_multiline(&mut chat.cfg.user_extra_prompt);

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !st.testing && !st.scanning,
                            egui::Button::new(if st.testing {
                                "Teste..."
                            } else {
                                "Verbindung testen"
                            }),
                        )
                        .clicked()
                    {
                        st.start_test(chat);
                    }
                    if ui.button("Einstellungen speichern").clicked() {
                        match save_llm_config(&chat.cfg) {
                            Ok(()) => st.save_msg = "Gespeichert.".into(),
                            Err(e) => st.save_msg = format!("Fehler: {e}"),
                        }
                    }
                    if !st.save_msg.is_empty() {
                        ui.colored_label(Color32::from_rgb(70, 180, 110), &st.save_msg);
                    }
                });

                if let Some(res) = &st.test_result {
                    ui.add_space(4.0);
                    match res {
                        Ok(msg) => {
                            ui.colored_label(Color32::from_rgb(70, 180, 110), msg);
                        }
                        Err(err) => {
                            ui.colored_label(
                                Color32::from_rgb(220, 70, 70),
                                format!("Test fehlgeschlagen: {err}"),
                            );
                        }
                    }
                }

                ui.add_space(10.0);
                ui.separator();
                ui.heading("3. Coop mit Agentic DE (MCP)");
                ui.label(
                    RichText::new(
                        "BuchCraft bringt einen vollständigen stdio MCP-Server mit (Tools + Bundled Skills). In Antigravity / Claude / Cursor ist kein separater API-Key nötig:",
                    )
                    .small()
                    .weak(),
                );
                let exe_hint = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.join("buchcraft-mcp.exe")))
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "buchcraft-mcp.exe".into());
                ui.monospace(format!("Command: {exe_hint}"));
                ui.monospace("CLI Smoke: .\\start.ps1 -Smoke | MCP: .\\start.ps1 -Mcp");
            });
        });
    });
}
