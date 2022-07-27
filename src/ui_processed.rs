use crate::processor::ProcessingStatus;
use crate::{MyApp, Tab};
use eframe::egui;
use egui::{ComboBox, Ui};
use egui_extras::{Size, StripBuilder, TableBody, TableBuilder};
use minidump_common::utils::basename;
use minidump_processor::{CallStack, InlineFrame, ProcessState, StackFrame};

pub struct ProcessedUiState {
    pub cur_thread: usize,
    pub cur_frame: usize,
}

impl MyApp {
    pub fn ui_processed(&mut self, ui: &mut Ui, _ctx: &egui::Context) {
        if let Some(Err(e)) = &self.minidump {
            ui.label("Minidump couldn't be read!");
            ui.label(e.to_string());
            return;
        }
        if let Some(state) = &self.processed {
            match state {
                Ok(state) => {
                    self.ui_processed_good(ui, &state.clone());
                }
                Err(e) => {
                    ui.label("Minidump couldn't be processed!");
                    ui.label(e.to_string());
                }
            }
        }
    }

    fn ui_processed_good(&mut self, ui: &mut Ui, state: &ProcessState) {
        // let is_symbolicated = self.cur_status == ProcessingStatus::Done;
        StripBuilder::new(ui)
            .size(Size::relative(0.5))
            .size(Size::remainder())
            .size(Size::exact(18.0))
            .vertical(|mut strip| {
                strip.cell(|ui| {
                    self.ui_processed_data(ui, state);
                });
                strip.cell(|ui| {
                    ui.horizontal(|ui| {
                        ui.label("Thread: ");
                        ComboBox::from_label("  ")
                            .width(400.0)
                            .selected_text(
                                state
                                    .threads
                                    .get(self.processed_ui_state.cur_thread)
                                    .map(crate::threadname)
                                    .unwrap_or_default(),
                            )
                            .show_ui(ui, |ui| {
                                for (idx, stack) in state.threads.iter().enumerate() {
                                    if ui
                                        .selectable_value(
                                            &mut self.processed_ui_state.cur_thread,
                                            idx,
                                            crate::threadname(stack),
                                        )
                                        .changed()
                                    {
                                        self.processed_ui_state.cur_frame = 0;
                                    };
                                }
                            });
                    });

                    ui.separator();

                    if let Some(stack) = state.threads.get(self.processed_ui_state.cur_thread) {
                        self.ui_processed_backtrace(ui, stack);
                    }
                });
                strip.cell(|ui| {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        let stats = self.analysis_state.stats.lock().unwrap();
                        let symbols = stats.pending_symbols.lock().unwrap().clone();
                        let (t_done, t_todo) = stats.processor_stats.get_thread_count();
                        let frames_walked = stats.processor_stats.get_frame_count();

                        let estimated_frames_per_thread = 10.0;
                        let estimated_progress = if t_todo == 0 {
                            0.0
                        } else {
                            let ratio = frames_walked as f32
                                / (t_todo as f32 * estimated_frames_per_thread);
                            ratio.min(0.9)
                        };
                        let in_progress = self.cur_status < ProcessingStatus::Done;
                        let progress = if in_progress { estimated_progress } else { 1.0 };

                        ui.label(format!(
                            "fetching symbols {}/{}",
                            symbols.symbols_processed, symbols.symbols_requested
                        ));
                        ui.label(format!("processing threads {}/{}", t_done, t_todo));
                        ui.label(format!("frames walked {}", frames_walked));

                        let progress_bar = egui::ProgressBar::new(progress)
                            .show_percentage()
                            .animate(in_progress);

                        ui.add(progress_bar);
                    });
                });
            });
    }

    fn ui_processed_data(&mut self, ui: &mut Ui, state: &ProcessState) {
        let cur_threadname = state
            .threads
            .get(self.processed_ui_state.cur_thread)
            .map(crate::threadname)
            .unwrap_or_default();

        StripBuilder::new(ui)
            .size(Size::relative(0.5))
            .size(Size::relative(0.5))
            .horizontal(|mut strip| {
                strip.cell(|ui| {
                    crate::listing(
                        ui,
                        1,
                        [
                            ("OS".to_owned(), state.system_info.os.to_string()),
                            (
                                "OS version".to_owned(),
                                state
                                    .system_info
                                    .format_os_version()
                                    .map(|s| s.clone().into_owned())
                                    .unwrap_or_default(),
                            ),
                            ("CPU".to_owned(), state.system_info.cpu.to_string()),
                            (
                                "CPU info".to_owned(),
                                state.system_info.cpu_info.clone().unwrap_or_default(),
                            ),
                            // ("Process Create Time".to_owned(), state.process_create_time.map(|s| format!("{:?}", s)).unwrap_or_default()),
                            // ("Process Crash Time".to_owned(), format!("{:?}", state.time)),
                            (
                                "Crash Reason".to_owned(),
                                state
                                    .crash_reason
                                    .map(|r| r.to_string())
                                    .unwrap_or_default(),
                            ),
                            (
                                "Crash Assertion".to_owned(),
                                state.assertion.clone().unwrap_or_default(),
                            ),
                            (
                                "Crash Address".to_owned(),
                                state
                                    .crash_address
                                    .map(|addr| format!("0x{:08x}", addr))
                                    .unwrap_or_default(),
                            ),
                            ("Crashing Thread".to_owned(), cur_threadname.clone()),
                        ],
                    );
                });
                strip.cell(|ui| {
                    ui.add_space(10.0);
                    ui.heading(format!("Thread {}", cur_threadname));
                    if let Some(thread) = state.threads.get(self.processed_ui_state.cur_thread) {
                        crate::listing(
                            ui,
                            2,
                            [(
                                "last_error_value".to_owned(),
                                thread
                                    .last_error_value
                                    .map(|e| e.to_string())
                                    .unwrap_or_default(),
                            )],
                        );
                        if let Some(frame) = thread.frames.get(self.processed_ui_state.cur_frame) {
                            ui.add_space(20.0);
                            ui.heading(format!("Frame {}", self.processed_ui_state.cur_frame));
                            let regs = frame
                                .context
                                .valid_registers()
                                .map(|(name, val)| (name.to_owned(), format!("0x{:08x}", val)));
                            crate::listing(ui, 3, regs);
                        }
                    }
                })
            });
    }

    fn ui_processed_backtrace(&mut self, ui: &mut Ui, stack: &CallStack) {
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right().with_cross_align(egui::Align::Center))
            .column(Size::initial(60.0).at_least(40.0))
            .column(Size::initial(80.0).at_least(40.0))
            .column(Size::initial(160.0).at_least(40.0))
            .column(Size::initial(160.0).at_least(40.0))
            .column(Size::remainder().at_least(60.0))
            .resizable(true)
            .header(20.0, |mut header| {
                header.col(|ui| {
                    ui.heading("Frame");
                });
                header.col(|ui| {
                    ui.heading("Trust");
                });
                header.col(|ui| {
                    ui.heading("Module");
                });
                header.col(|ui| {
                    ui.heading("Source");
                });
                header.col(|ui| {
                    ui.heading("Signature");
                });
            })
            .body(|mut body| {
                let mut frame_count = 0;
                for (frame_idx, frame) in stack.frames.iter().enumerate() {
                    for inline in frame.inlines.iter().rev() {
                        let frame_num = frame_count;
                        frame_count += 1;
                        self.ui_inline_frame(&mut body, frame_num, frame, inline);
                    }

                    let frame_num = frame_count;
                    frame_count += 1;
                    self.ui_real_frame(&mut body, frame_idx, frame_num, frame);
                }
            });
    }

    fn ui_real_frame(
        &mut self,
        body: &mut TableBody,
        frame_idx: usize,
        frame_num: usize,
        frame: &StackFrame,
    ) {
        let row_height = 18.0;

        body.row(row_height, |mut row| {
            row.col(|ui| {
                ui.centered_and_justified(|ui| {
                    if ui.link(frame_num.to_string()).clicked() {
                        self.processed_ui_state.cur_frame = frame_idx;
                    }
                });
            });
            row.col(|ui| {
                let trust = match frame.trust {
                    minidump_processor::FrameTrust::None => "none",
                    minidump_processor::FrameTrust::Scan => "scan",
                    minidump_processor::FrameTrust::CfiScan => "cfi scan",
                    minidump_processor::FrameTrust::FramePointer => "frame pointer",
                    minidump_processor::FrameTrust::CallFrameInfo => "cfi",
                    minidump_processor::FrameTrust::PreWalked => "prewalked",
                    minidump_processor::FrameTrust::Context => "context",
                };
                ui.centered_and_justified(|ui| {
                    if ui.link(trust).clicked() {
                        self.tab = Tab::Logs;
                        self.log_ui_state.cur_thread = Some(self.processed_ui_state.cur_thread);
                        self.log_ui_state.cur_frame = Some(frame_idx);
                    }
                });
            });
            row.col(|ui| {
                if let Some(module) = &frame.module {
                    ui.centered_and_justified(|ui| {
                        ui.label(basename(&module.name));
                    });
                }
            });
            row.col(|ui| {
                let mut label = String::new();
                crate::frame_source(&mut label, frame).unwrap();
                ui.label(label);
            });
            row.col(|ui| {
                let mut label = String::new();
                crate::frame_signature(&mut label, frame).unwrap();
                ui.label(label);
            });
        });
    }

    fn ui_inline_frame(
        &mut self,
        body: &mut TableBody,
        frame_num: usize,
        real_frame: &StackFrame,
        frame: &InlineFrame,
    ) {
        let row_height = 18.0;

        body.row(row_height, |mut row| {
            row.col(|ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(frame_num.to_string());
                });
            });
            row.col(|ui| {
                let trust = "inlined";
                ui.centered_and_justified(|ui| {
                    ui.label(trust);
                });
            });
            row.col(|ui| {
                if let Some(module) = &real_frame.module {
                    ui.centered_and_justified(|ui| {
                        ui.label(basename(&module.name));
                    });
                }
            });
            row.col(|ui| {
                if let (Some(source_file), Some(line)) =
                    (frame.source_file_name.as_ref(), frame.source_line.as_ref())
                {
                    ui.label(format!("{}: {line}", basename(source_file)));
                }
            });
            row.col(|ui| {
                ui.label(&frame.function_name);
            });
        });
    }
}
