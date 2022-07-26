use crate::MyApp;
use eframe::egui;
use egui::{ComboBox, TextStyle, Ui};

pub struct LogUiState {
    pub cur_thread: Option<usize>,
    pub cur_frame: Option<usize>,
}

impl MyApp {
    pub fn ui_logs(&mut self, ui: &mut Ui, _ctx: &egui::Context) {
        let ui_state = &mut self.log_ui_state;
        if let Some(Ok(state)) = &self.processed {
            ui.horizontal(|ui| {
                ui.label("Thread: ");
                ComboBox::from_label(" ")
                    .width(400.0)
                    .selected_text(
                        ui_state
                            .cur_thread
                            .and_then(|thread| state.threads.get(thread).map(crate::threadname))
                            .unwrap_or_else(|| "<no thread>".to_owned()),
                    )
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_value(&mut ui_state.cur_thread, None, "<no thread>")
                            .changed()
                        {
                            ui_state.cur_frame = None;
                        };
                        for (idx, stack) in state.threads.iter().enumerate() {
                            if ui
                                .selectable_value(
                                    &mut ui_state.cur_thread,
                                    Some(idx),
                                    crate::threadname(stack),
                                )
                                .changed()
                            {
                                ui_state.cur_frame = None;
                            };
                        }
                    });
                let thread = ui_state.cur_thread.and_then(|t| state.threads.get(t));
                if let Some(thread) = thread {
                    ui.label("Frame: ");
                    ComboBox::from_label("")
                        .width(400.0)
                        .selected_text(crate::frame_signature_from_indices(
                            state,
                            ui_state.cur_thread,
                            ui_state.cur_frame,
                        ))
                        .show_ui(ui, |ui| {
                            let no_name = crate::frame_signature_from_indices(
                                state,
                                ui_state.cur_thread,
                                None,
                            );
                            ui.selectable_value(&mut ui_state.cur_frame, None, no_name);
                            for (idx, _stack) in thread.frames.iter().enumerate() {
                                let name = crate::frame_signature_from_indices(
                                    state,
                                    ui_state.cur_thread,
                                    Some(idx),
                                );
                                ui.selectable_value(&mut ui_state.cur_frame, Some(idx), name);
                            }
                        });
                }
            });
        }

        // Print the logs
        egui::ScrollArea::vertical().show(ui, |ui| {
            let text = match (ui_state.cur_thread, ui_state.cur_frame) {
                (Some(t), Some(f)) => self.logger.string_for_frame(t, f),
                (Some(t), None) => self.logger.string_for_thread(t),
                _ => self.logger.string_for_all(),
            };
            ui.add(
                egui::TextEdit::multiline(&mut &**text)
                    .font(TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
    }
}
