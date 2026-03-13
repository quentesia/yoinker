use eframe::egui;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use yoinker_common::{ClipboardEntry, Config, Request, Response};

const LOCK_PATH: &str = "/tmp/yoinker-gui.lock";

fn main() -> eframe::Result<()> {
    // Single-instance toggle: if already running, kill it and exit.
    // If stale lock (process dead), remove and continue.
    if let Ok(pid_str) = std::fs::read_to_string(LOCK_PATH) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            if pid != std::process::id() as i32 {
                if unsafe { libc::kill(pid, 0) } == 0 {
                    // It's running — kill it (toggle off) and exit
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                    std::fs::remove_file(LOCK_PATH).ok();
                    return Ok(());
                } else {
                    // Stale lock file from a crash — clean up
                    std::fs::remove_file(LOCK_PATH).ok();
                }
            }
        }
    }

    // Write our PID
    std::fs::write(LOCK_PATH, std::process::id().to_string()).ok();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let config = Config::load();

    // Fetch entries from daemon, auto-starting if needed
    let (entries, daemon_error) = rt.block_on(async {
        match send_with_autostart(&config).await {
            Ok(entries) => (entries, None),
            Err(e) => (Vec::new(), Some(e)),
        }
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([600.0, 500.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top(),
        ..Default::default()
    };

    let result = eframe::run_native(
        "Yoinker",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_pixels_per_point(1.5);
            Ok(Box::new(App::new(entries, config, rt, daemon_error)))
        }),
    );

    // Clean up lock file on exit
    std::fs::remove_file(LOCK_PATH).ok();
    result
}

struct App {
    entries: Vec<ClipboardEntry>,
    config: Config,
    rt: tokio::runtime::Runtime,
    query: String,
    selected: usize,
    should_close: bool,
    first_frame: bool,
    tagging: Option<usize>,
    tag_input: String,
    daemon_error: Option<String>,
}

impl App {
    fn new(
        entries: Vec<ClipboardEntry>,
        config: Config,
        rt: tokio::runtime::Runtime,
        daemon_error: Option<String>,
    ) -> Self {
        Self {
            entries,
            config,
            rt,
            query: String::new(),
            selected: 0,
            should_close: false,
            first_frame: true,
            tagging: None,
            tag_input: String::new(),
            daemon_error,
        }
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let query_lower = self.query.to_lowercase();
        let mut result: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                if self.query.is_empty() {
                    return true;
                }
                // Match against content preview or tag
                let content_match = e.content.preview(200).to_lowercase().contains(&query_lower);
                let tag_match = e
                    .tag
                    .as_ref()
                    .map(|t| t.to_lowercase().contains(&query_lower))
                    .unwrap_or(false);
                content_match || tag_match
            })
            .map(|(i, _)| i)
            .collect();
        // Pinned first; within pinned, exact tag matches first
        result.sort_by_key(|&i| {
            let e = &self.entries[i];
            let exact_tag = if !self.query.is_empty() {
                e.tag
                    .as_ref()
                    .map(|t| t.to_lowercase() == query_lower)
                    .unwrap_or(false)
            } else {
                false
            };
            (
                if exact_tag {
                    0
                } else if e.pinned {
                    1
                } else {
                    2
                },
                i,
            )
        });
        result
    }

    fn send_request(&self, req: Request) -> Option<Response> {
        let config = self.config.clone();
        self.rt.block_on(async { send(&config, req).await.ok() })
    }

    fn select_entry(&mut self, index: usize) {
        self.send_request(Request::Copy { index });
        self.should_close = true;
    }

    fn toggle_pin(&mut self, index: usize) {
        let pinned = self.entries[index].pinned;
        let req = if pinned {
            Request::Unpin { index }
        } else {
            Request::Pin { index }
        };
        if let Some(Response::Ok) = self.send_request(req) {
            self.entries[index].pinned = !pinned;
        }
    }

    fn delete_entry(&mut self, index: usize) {
        if let Some(Response::Ok) = self.send_request(Request::Delete { index }) {
            self.entries.remove(index);
        }
    }

    fn set_tag(&mut self, index: usize, tag: String) {
        let tag = if tag.is_empty() { None } else { Some(tag) };
        if let Some(Response::Ok) = self.send_request(Request::Tag {
            index,
            tag: tag.clone(),
        }) {
            self.entries[index].tag = tag;
        }
    }
}

fn relative_time(timestamp: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let diff = now.saturating_sub(timestamp);
    if diff < 60 {
        format!("{}s ago", diff)
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.should_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Read Tab state then consume Tab events so egui doesn't use them for focus cycling
        let tab_down =
            ctx.input_mut(|i| i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::Tab) > 0);
        let tab_up =
            ctx.input_mut(|i| i.count_and_consume_key(egui::Modifiers::SHIFT, egui::Key::Tab) > 0);

        {
            let filtered = self.filtered_indices();
            let filtered_len = filtered.len();

            // Clamp selection
            if filtered_len == 0 {
                self.selected = 0;
            } else if self.selected >= filtered_len {
                self.selected = filtered_len - 1;
            }

            // Keyboard handling
            let close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            if close {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }

            // Arrow keys / Ctrl+N/P / Tab/Shift+Tab to navigate
            let move_down = ctx.input(|i| {
                i.key_pressed(egui::Key::ArrowDown)
                    || (i.modifiers.ctrl && i.key_pressed(egui::Key::N))
            }) || tab_down;
            let move_up = ctx.input(|i| {
                i.key_pressed(egui::Key::ArrowUp)
                    || (i.modifiers.ctrl && i.key_pressed(egui::Key::P))
            }) || tab_up;
            let page_down = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::D));
            let page_up = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::U));

            if move_down && self.selected + 1 < filtered_len {
                self.selected += 1;
            }
            if move_up && self.selected > 0 {
                self.selected -= 1;
            }
            if page_down {
                self.selected = (self.selected + 8).min(filtered_len.saturating_sub(1));
            }
            if page_up {
                self.selected = self.selected.saturating_sub(8);
            }

            // Enter to select
            let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
            if enter && !filtered.is_empty() {
                let orig_idx = filtered[self.selected];
                self.select_entry(orig_idx);
                return;
            }

            // Ctrl+X to delete, Ctrl+P to toggle pin
            let ctrl_x = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::X));
            let ctrl_p_pressed = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::P));

            let ctrl_t = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::T));

            if !filtered.is_empty() {
                let orig_idx = filtered[self.selected];
                if ctrl_x {
                    self.delete_entry(orig_idx);
                    return;
                }
                if ctrl_p_pressed {
                    self.toggle_pin(orig_idx);
                    return;
                }
                if ctrl_t {
                    self.tag_input = self.entries[orig_idx].tag.clone().unwrap_or_default();
                    self.tagging = Some(orig_idx);
                }
            }
        }

        let focus_search = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::L));

        // Bottom keybind legend
        egui::TopBottomPanel::bottom("legend")
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(30, 30, 36))
                    .inner_margin(egui::Margin::symmetric(16, 6)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(
                            "Enter: select | Esc: close | Tab/↑↓: navigate | Ctrl+D/U: page | Ctrl+L: search | Ctrl+P: pin | Ctrl+T: tag | Ctrl+X: delete",
                        )
                        .color(egui::Color32::from_rgb(120, 120, 140))
                        .size(10.0),
                    );
                });
            });

        let panel_frame = egui::Frame::default()
            .fill(egui::Color32::from_rgb(30, 30, 36))
            .inner_margin(16.0)
            .corner_radius(12.0);

        egui::CentralPanel::default()
            .frame(panel_frame)
            .show(ctx, |ui| {
                // Title bar
                ui.horizontal(|ui| {
                    ui.heading(
                        egui::RichText::new("Yoinker")
                            .color(egui::Color32::from_rgb(200, 200, 220))
                            .size(20.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(egui::RichText::new("X").color(egui::Color32::LIGHT_GRAY))
                            .clicked()
                        {
                            self.should_close = true;
                        }
                    });
                });

                ui.add_space(8.0);

                // Search box
                let search_response = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Search...")
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Body),
                );

                if self.first_frame || focus_search {
                    search_response.request_focus();
                    self.first_frame = false;
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // Re-filter after possible query change
                let filtered = self.filtered_indices();
                let filtered_len = filtered.len();
                if filtered_len == 0 {
                    self.selected = 0;
                } else if self.selected >= filtered_len {
                    self.selected = filtered_len - 1;
                }

                if filtered.is_empty() {
                    ui.centered_and_justified(|ui| {
                        let msg = if let Some(err) = &self.daemon_error {
                            format!(
                                "Cannot connect to yoinkerd: {}\nHint: yoinker daemon start",
                                err
                            )
                        } else if self.entries.is_empty() {
                            "Clipboard history is empty".to_string()
                        } else {
                            "No matches".to_string()
                        };
                        ui.label(
                            egui::RichText::new(msg)
                                .color(if self.daemon_error.is_some() {
                                    egui::Color32::from_rgb(200, 100, 100)
                                } else {
                                    egui::Color32::GRAY
                                })
                                .size(14.0),
                        );
                    });
                } else {
                    let mut action_select: Option<usize> = None;
                    let mut action_pin: Option<usize> = None;
                    let mut action_delete: Option<usize> = None;

                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            for (list_idx, &orig_idx) in filtered.iter().enumerate() {
                                let entry = &self.entries[orig_idx];
                                let is_pinned = entry.pinned;
                                let is_selected = list_idx == self.selected;

                                let preview = entry.content.preview(80);
                                let time = relative_time(entry.timestamp);
                                let tag = entry.tag.clone();

                                let bg = if is_selected {
                                    egui::Color32::from_rgb(50, 55, 70)
                                } else if is_pinned {
                                    egui::Color32::from_rgb(40, 45, 55)
                                } else {
                                    egui::Color32::from_rgb(38, 38, 44)
                                };

                                let frame = egui::Frame::default()
                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                    .corner_radius(6.0)
                                    .fill(bg);

                                let response = frame
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            let text_resp = ui
                                                .vertical(|ui| {
                                                    ui.set_min_width(ui.available_width() - 60.0);

                                                    let text_color = if is_pinned {
                                                        egui::Color32::from_rgb(130, 200, 220)
                                                    } else {
                                                        egui::Color32::from_rgb(210, 210, 210)
                                                    };

                                                    let mut label = egui::RichText::new(&preview)
                                                        .color(text_color)
                                                        .size(13.0);
                                                    if is_pinned {
                                                        label = label.strong();
                                                    }
                                                    ui.label(label);

                                                    ui.horizontal(|ui| {
                                                        ui.label(
                                                            egui::RichText::new(&time)
                                                                .color(egui::Color32::GRAY)
                                                                .size(10.0),
                                                        );
                                                        if is_pinned {
                                                            ui.label(
                                                                egui::RichText::new("pinned")
                                                                    .color(egui::Color32::from_rgb(
                                                                        100, 180, 200,
                                                                    ))
                                                                    .size(10.0),
                                                            );
                                                        }
                                                        if let Some(t) = &tag {
                                                            ui.label(
                                                                egui::RichText::new(format!(
                                                                    "#{}",
                                                                    t
                                                                ))
                                                                .color(egui::Color32::from_rgb(
                                                                    180, 160, 100,
                                                                ))
                                                                .size(10.0)
                                                                .strong(),
                                                            );
                                                        }
                                                    });
                                                })
                                                .response;

                                            if text_resp.interact(egui::Sense::click()).clicked() {
                                                action_select = Some(orig_idx);
                                            }

                                            // Action buttons
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if ui
                                                        .small_button(
                                                            egui::RichText::new("x")
                                                                .color(egui::Color32::from_rgb(
                                                                    180, 80, 80,
                                                                ))
                                                                .size(12.0),
                                                        )
                                                        .on_hover_text("Delete")
                                                        .clicked()
                                                    {
                                                        action_delete = Some(orig_idx);
                                                    }
                                                    let pin_label =
                                                        if is_pinned { "unpin" } else { "pin" };
                                                    if ui
                                                        .small_button(
                                                            egui::RichText::new(pin_label)
                                                                .color(egui::Color32::from_rgb(
                                                                    100, 180, 200,
                                                                ))
                                                                .size(12.0),
                                                        )
                                                        .on_hover_text(if is_pinned {
                                                            "Unpin"
                                                        } else {
                                                            "Pin"
                                                        })
                                                        .clicked()
                                                    {
                                                        action_pin = Some(orig_idx);
                                                    }
                                                },
                                            );
                                        });
                                    })
                                    .response;

                                // Hover highlight (only when not selected)
                                if response.hovered() && !is_selected {
                                    ui.painter().rect_stroke(
                                        response.rect,
                                        6.0,
                                        egui::Stroke::new(
                                            1.0,
                                            egui::Color32::from_rgb(80, 80, 100),
                                        ),
                                        egui::epaint::StrokeKind::Outside,
                                    );
                                }
                                // Selected indicator
                                if is_selected {
                                    ui.painter().rect_stroke(
                                        response.rect,
                                        6.0,
                                        egui::Stroke::new(
                                            1.5,
                                            egui::Color32::from_rgb(100, 140, 200),
                                        ),
                                        egui::epaint::StrokeKind::Outside,
                                    );
                                }

                                ui.add_space(2.0);
                            }
                        });

                    // Apply deferred actions (button clicks)
                    if let Some(idx) = action_delete {
                        self.delete_entry(idx);
                    } else if let Some(idx) = action_pin {
                        self.toggle_pin(idx);
                    } else if let Some(idx) = action_select {
                        self.select_entry(idx);
                    }
                }
            });

        // Tag input modal
        if self.tagging.is_some() {
            let mut open = true;
            let mut submit = false;

            egui::Window::new("Tag Entry")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new("Enter a tag word for quick access:")
                            .color(egui::Color32::LIGHT_GRAY)
                            .size(12.0),
                    );
                    ui.add_space(4.0);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.tag_input)
                            .hint_text("e.g. email, address, sig...")
                            .desired_width(200.0),
                    );
                    resp.request_focus();
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            submit = true;
                        }
                        if ui.button("Remove tag").clicked() {
                            self.tag_input.clear();
                            submit = true;
                        }
                    });
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("Type the tag in search to quickly find this entry")
                            .color(egui::Color32::GRAY)
                            .size(9.0),
                    );
                });

            if submit {
                if let Some(idx) = self.tagging.take() {
                    let tag = self.tag_input.clone();
                    self.set_tag(idx, tag);
                }
            }
            if !open {
                self.tagging = None;
            }
        }
    }
}

fn find_yoinkerd() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("yoinkerd");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from("yoinkerd")
}

async fn send_with_autostart(config: &Config) -> Result<Vec<ClipboardEntry>, String> {
    // Try direct connection first
    if let Ok(Response::Entries(e)) = send(config, Request::List).await {
        return Ok(e);
    }

    // Try starting the daemon
    let yoinkerd = find_yoinkerd();
    let log_path = config
        .history_path
        .parent()
        .map(|p| p.join("yoinkerd.log"))
        .unwrap_or_else(|| PathBuf::from("/tmp/yoinkerd.log"));

    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let log_file = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;

    std::process::Command::new(&yoinkerd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file))
        .spawn()
        .map_err(|e| format!("cannot start yoinkerd: {}", e))?;

    // Wait for daemon to be ready
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Ok(Response::Entries(e)) = send(config, Request::List).await {
            return Ok(e);
        }
    }

    Err("daemon started but not responding".to_string())
}

async fn send(config: &Config, request: Request) -> Result<Response, String> {
    let stream = UnixStream::connect(&config.socket_path)
        .await
        .map_err(|e| format!("cannot connect: {}", e))?;

    let (reader, mut writer) = stream.into_split();

    let json = serde_json::to_string(&request).map_err(|e| e.to_string())?;
    writer
        .write_all(json.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    writer.write_all(b"\n").await.map_err(|e| e.to_string())?;
    writer.shutdown().await.map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|e| e.to_string())?;

    serde_json::from_str(line.trim()).map_err(|e| format!("invalid response: {}", e))
}
