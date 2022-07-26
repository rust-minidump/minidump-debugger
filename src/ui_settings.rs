use eframe::egui;
use egui::Ui;

use crate::processor::ProcessingStatus;
use crate::MyApp;

impl MyApp {
    pub fn ui_settings(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        ui.add_space(20.0);
        ui.heading("choose minidump");
        ui.add_space(10.0);
        let message = match self.cur_status {
            ProcessingStatus::NoDump => "Select or drop a minidump!",
            ProcessingStatus::ReadingDump => "Reading minidump...",
            ProcessingStatus::RawProcessing => "Processing minidump...",
            ProcessingStatus::Symbolicating => "Minidump processed!",
            ProcessingStatus::Done => "Minidump processed!",
        };

        ui.horizontal(|ui| {
            ui.label(message);

            let cancellable = match self.cur_status {
                ProcessingStatus::NoDump | ProcessingStatus::Done => false,
                ProcessingStatus::ReadingDump
                | ProcessingStatus::RawProcessing
                | ProcessingStatus::Symbolicating => true,
            };
            ui.add_enabled_ui(cancellable, |ui| {
                if ui.button("❌ cancel").clicked() {
                    self.cancel_processing();
                }
            });
            let reprocessable = matches!(&self.minidump, Some(Ok(_)));
            ui.add_enabled_ui(reprocessable, |ui| {
                if ui.button("💫 reprocess").clicked() {
                    self.process_dump(self.minidump.as_ref().unwrap().as_ref().unwrap().clone());
                }
            });
        });

        if ui.button("Open file...").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("minidump", &["dmp"])
                .pick_file()
            {
                self.set_path(path);
            }
        }

        if let Some(picked_path) = &self.settings.picked_path {
            ui.horizontal(|ui| {
                ui.label("Picked file:");
                ui.monospace(picked_path);
            });
        }
        ui.add_space(60.0);
        ui.separator();
        ui.heading("symbol servers");
        ui.add_space(10.0);
        let mut to_remove = vec![];
        for (idx, (item, enabled)) in self.settings.symbol_urls.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.checkbox(enabled, "");
                ui.text_edit_singleline(item);
                if ui.button("❌").clicked() {
                    to_remove.push(idx);
                };
            });
        }
        for idx in to_remove.into_iter().rev() {
            self.settings.symbol_urls.remove(idx);
        }
        if ui.button("➕").clicked() {
            self.settings.symbol_urls.push((String::new(), true));
        }

        ui.add_space(20.0);
        ui.heading("local symbols");
        ui.add_space(10.0);
        let mut to_remove = vec![];
        for (idx, (item, enabled)) in self.settings.symbol_paths.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.checkbox(enabled, "");
                ui.text_edit_singleline(item);
                if ui.button("❌").clicked() {
                    to_remove.push(idx);
                };
            });
        }
        if ui.button("➕").clicked() {
            self.settings.symbol_paths.push((String::new(), true));
        }

        ui.add_space(20.0);
        ui.heading("misc settings");
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label("symbol cache");
            ui.checkbox(&mut self.settings.symbol_cache.1, "");
            ui.text_edit_singleline(&mut self.settings.symbol_cache.0);
        });
        ui.horizontal(|ui| {
            ui.label("http timeout secs");
            ui.text_edit_singleline(&mut self.settings.http_timeout_secs);
        });
        for idx in to_remove.into_iter().rev() {
            self.settings.symbol_paths.remove(idx);
        }
        ui.checkbox(
            &mut self.settings.raw_dump_brief,
            "hide memory dumps in raw mode",
        );

        ui.add_space(20.0);
        preview_files_being_dropped(ctx);

        // Collect dropped files:
        if let Some(dropped) = ctx.input().raw.dropped_files.get(0) {
            if let Some(path) = &dropped.path {
                self.set_path(path.clone());
            }
        }
    }
}

/// Preview hovering files:
fn preview_files_being_dropped(ctx: &egui::Context) {
    use egui::*;
    use std::fmt::Write as _;

    if !ctx.input().raw.hovered_files.is_empty() {
        let mut text = "Dropping files:\n".to_owned();
        for file in &ctx.input().raw.hovered_files {
            if let Some(path) = &file.path {
                write!(text, "\n{}", path.display()).ok();
            } else if !file.mime.is_empty() {
                write!(text, "\n{}", file.mime).ok();
            } else {
                text += "\n???";
            }
        }

        let painter =
            ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("file_drop_target")));

        let screen_rect = ctx.input().screen_rect();
        painter.rect_filled(screen_rect, 0.0, Color32::from_black_alpha(192));
        painter.text(
            screen_rect.center(),
            Align2::CENTER_CENTER,
            text,
            TextStyle::Heading.resolve(&ctx.style()),
            Color32::WHITE,
        );
    }
}
