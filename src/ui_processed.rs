use std::ops::Range;

use crate::processor::ProcessingStatus;
use crate::{MyApp, Tab};
use eframe::egui;
use egui::text::LayoutJob;
use egui::{Color32, ComboBox, Context, FontId, TextFormat, Ui};
use egui_extras::{Size, StripBuilder, TableBody, TableBuilder};
use minidump_common::utils::basename;
use minidump_processor::{CallStack, ProcessState, StackFrame};

pub struct ProcessedUiState {
    pub cur_thread: usize,
    pub cur_frame: usize,
}

use inline_shim::*;
#[cfg(feature = "inline")]
mod inline_shim {
    pub use minidump_processor::InlineFrame;
    use minidump_processor::StackFrame;
    pub fn get_inline_frames(frame: &StackFrame) -> &[InlineFrame] {
        &frame.inlines
    }
}

#[cfg(not(feature = "inline"))]
mod inline_shim {
    use minidump_processor::StackFrame;

    /// A stack frame in an inlined function.
    #[derive(Debug, Clone)]
    pub struct InlineFrame {
        /// The name of the function
        pub function_name: String,
        /// The file name of the stack frame
        pub source_file_name: Option<String>,
        /// The line number of the stack frame
        pub source_line: Option<u32>,
    }

    pub fn get_inline_frames(_frame: &StackFrame) -> &[InlineFrame] {
        &[]
    }
}

impl MyApp {
    pub fn ui_processed(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        if let Some(Err(e)) = &self.minidump {
            ui.label("Minidump couldn't be read!");
            ui.label(e.to_string());
            return;
        }
        if let Some(state) = &self.processed {
            match state {
                Ok(state) => {
                    self.ui_processed_good(ui, ctx, &state.clone());
                }
                Err(e) => {
                    ui.label("Minidump couldn't be processed!");
                    ui.label(e.to_string());
                }
            }
        }
    }

    fn ui_processed_good(&mut self, ui: &mut Ui, ctx: &Context, state: &ProcessState) {
        // let is_symbolicated = self.cur_status == ProcessingStatus::Done;
        StripBuilder::new(ui)
            .size(Size::relative(0.5))
            .size(Size::remainder())
            .size(Size::exact(18.0))
            .vertical(|mut strip| {
                strip.cell(|ui| {
                    self.ui_processed_data(ui, ctx, state);
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
                        self.ui_processed_backtrace(ui, ctx, stack);
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

    fn ui_processed_data(&mut self, ui: &mut Ui, ctx: &Context, state: &ProcessState) {
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
                        ctx,
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
                                    .map(|addr| self.format_addr(addr))
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
                            ctx,
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
                                .map(|(name, val)| (name.to_owned(), self.format_addr(val)));
                            crate::listing(ui, ctx, 3, regs);
                        }
                    }
                })
            });
    }

    fn ui_processed_backtrace(&mut self, ui: &mut Ui, ctx: &Context, stack: &CallStack) {
        let font = egui::style::TextStyle::Body.resolve(ui.style());
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right().with_cross_align(egui::Align::Center))
            .column(Size::initial(60.0).at_least(40.0))
            .column(Size::initial(80.0).at_least(40.0))
            .column(Size::initial(160.0).at_least(40.0))
            .column(Size::initial(160.0).at_least(40.0))
            .column(Size::remainder().at_least(60.0))
            .resizable(true)
            .clip(false)
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
                let mut widths = [0.0f32; 5];
                widths.clone_from_slice(body.widths());
                for (frame_idx, frame) in stack.frames.iter().enumerate() {
                    for inline in get_inline_frames(frame).iter().rev() {
                        let frame_num = frame_count;
                        frame_count += 1;
                        self.ui_inline_frame(
                            &mut body, ctx, &widths, &font, frame_num, frame, inline,
                        );
                    }

                    let frame_num = frame_count;
                    frame_count += 1;
                    self.ui_real_frame(&mut body, ctx, &widths, &font, frame_idx, frame_num, frame);
                }
            });
    }

    fn ui_real_frame(
        &mut self,
        body: &mut TableBody,
        ctx: &Context,
        widths: &[f32],
        font: &FontId,
        frame_idx: usize,
        frame_num: usize,
        frame: &StackFrame,
    ) {
        let col1_width = widths[0];
        let col2_width = widths[1];
        let col3_width = widths[2];
        let col4_width = widths[3];
        let col5_width = widths[4];

        let (col1, col2, col3, col4, col5, row_height) = {
            let fonts = ctx.fonts();
            let col1 = {
                fonts.layout(
                    frame_num.to_string(),
                    font.clone(),
                    Color32::BLACK,
                    col1_width,
                )
            };
            let col2 = {
                let trust = match frame.trust {
                    minidump_processor::FrameTrust::None => "none",
                    minidump_processor::FrameTrust::Scan => "scan",
                    minidump_processor::FrameTrust::CfiScan => "cfi scan",
                    minidump_processor::FrameTrust::FramePointer => "frame pointer",
                    minidump_processor::FrameTrust::CallFrameInfo => "cfi",
                    minidump_processor::FrameTrust::PreWalked => "prewalked",
                    minidump_processor::FrameTrust::Context => "context",
                };
                fonts.layout(trust.to_owned(), font.clone(), Color32::BLACK, col2_width)
            };
            let col3 = {
                let label = if let Some(module) = &frame.module {
                    basename(&module.name).to_string()
                } else {
                    String::new()
                };
                fonts.layout(label, font.clone(), Color32::BLACK, col3_width)
            };
            let col4 = {
                let mut label = String::new();
                crate::frame_source(&mut label, frame).unwrap();
                fonts.layout(label, font.clone(), Color32::BLACK, col4_width)
            };
            let col5 = {
                let mut label = String::new();
                crate::frame_signature(&mut label, frame).unwrap();
                let fname = &label[..];
                let parsed = parse_function_name(fname);
                let parts = [
                    (0..parsed.type_name.start, false),
                    (parsed.type_name.clone(), true),
                    (parsed.type_name.end..parsed.func_name.start, false),
                    (parsed.func_name.clone(), true),
                    (parsed.func_name.end..fname.len(), false),
                ];

                let mut job = LayoutJob::default();
                job.wrap.max_width = col5_width;
                for (range, is_bold) in parts {
                    job.append(
                        &fname[range],
                        0.0,
                        TextFormat {
                            font_id: font.clone(),
                            color: if is_bold {
                                Color32::BLACK
                            } else {
                                Color32::GRAY
                            },
                            ..Default::default()
                        },
                    );
                }
                fonts.layout_job(job)
            };

            let row_height = col1
                .rect
                .height()
                .max(col2.rect.height())
                .max(col3.rect.height())
                .max(col4.rect.height())
                .max(col5.rect.height())
                + 6.0;
            (col1, col2, col3, col4, col5, row_height)
        };

        body.row(row_height, |mut row| {
            row.col(|ui| {
                ui.centered_and_justified(|ui| {
                    if ui.link(col1).clicked() {
                        self.processed_ui_state.cur_frame = frame_idx;
                    }
                });
            });
            row.col(|ui| {
                ui.centered_and_justified(|ui| {
                    if ui.link(col2).clicked() {
                        self.tab = Tab::Logs;
                        self.log_ui_state.cur_thread = Some(self.processed_ui_state.cur_thread);
                        self.log_ui_state.cur_frame = Some(frame_idx);
                    }
                });
            });
            row.col(|ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(col3);
                });
            });
            row.col(|ui| {
                ui.label(col4);
            });
            row.col(|ui| {
                ui.label(col5);
            });
        });
    }

    fn ui_inline_frame(
        &mut self,
        body: &mut TableBody,
        ctx: &Context,
        widths: &[f32],
        font: &FontId,
        frame_num: usize,
        real_frame: &StackFrame,
        frame: &InlineFrame,
    ) {
        let col1_width = widths[0];
        let col2_width = widths[1];
        let col3_width = widths[2];
        let col4_width = widths[3];
        let col5_width = widths[4];
        let (col1, col2, col3, col4, col5, row_height) = {
            let fonts = ctx.fonts();
            let col1 = {
                fonts.layout(
                    frame_num.to_string(),
                    font.clone(),
                    Color32::BLACK,
                    col1_width,
                )
            };
            let col2 = {
                let trust = "inlined";
                fonts.layout(trust.to_owned(), font.clone(), Color32::BLACK, col2_width)
            };
            let col3 = {
                let label = if let Some(module) = &real_frame.module {
                    basename(&module.name).to_string()
                } else {
                    String::new()
                };
                fonts.layout(label, font.clone(), Color32::BLACK, col3_width)
            };
            let col4 = {
                let label = if let (Some(source_file), Some(line)) =
                    (frame.source_file_name.as_ref(), frame.source_line.as_ref())
                {
                    format!("{}: {}", basename(source_file).to_owned(), line)
                } else {
                    String::new()
                };
                fonts.layout(label, font.clone(), Color32::BLACK, col4_width)
            };
            let col5 = {
                let fname = &frame.function_name;
                let parsed = parse_function_name(fname);
                let parts = [
                    (0..parsed.type_name.start, false),
                    (parsed.type_name.clone(), true),
                    (parsed.type_name.end..parsed.func_name.start, false),
                    (parsed.func_name.clone(), true),
                    (parsed.func_name.end..fname.len(), false),
                ];

                let mut job = LayoutJob::default();
                job.wrap.max_width = col5_width;
                for (range, is_bold) in parts {
                    job.append(
                        &fname[range],
                        0.0,
                        TextFormat {
                            font_id: font.clone(),
                            color: if is_bold {
                                Color32::BLACK
                            } else {
                                Color32::GRAY
                            },
                            ..Default::default()
                        },
                    );
                }
                fonts.layout_job(job)
            };

            let row_height = col1
                .rect
                .height()
                .max(col2.rect.height())
                .max(col3.rect.height())
                .max(col4.rect.height())
                .max(col5.rect.height())
                + 6.0;
            (col1, col2, col3, col4, col5, row_height)
        };

        body.row(row_height, |mut row| {
            row.col(|ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(col1);
                });
            });
            row.col(|ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(col2);
                });
            });
            row.col(|ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(col3);
                });
            });
            row.col(|ui| {
                ui.label(col4);
            });
            row.col(|ui| {
                ui.label(col5);
            });
        });
    }
}

struct ParsedFuncName {
    type_name: Range<usize>,
    func_name: Range<usize>,
    _args: Vec<Range<usize>>,
}

fn parse_function_name(func: &str) -> ParsedFuncName {
    let mut gen_depth = 0isize;
    let mut paren_depth = 0isize;
    let mut last_saw_colon = false;
    let mut last_saw_double_colon = false;
    let mut last_saw_real_char = false;

    let mut cur_piece_start = 0usize;
    let mut cur_piece_end;

    let mut func_name_start = 0usize;
    let mut func_name_end = 0usize;
    let mut type_name_start = 0usize;
    let mut type_name_end = 0usize;

    for (idx, c) in func.char_indices() {
        let mut saw_colon = false;
        let mut saw_real_char = false;
        let mut gen_adjust = 0isize;
        let mut paren_adjust = 0isize;
        match c {
            '<' => {
                gen_adjust = 1;
            }
            '>' => {
                assert!(gen_depth > 0, "mismatched generic close!");
                gen_adjust = -1;
            }
            '(' => {
                paren_adjust = 1;
            }
            ')' => {
                assert!(paren_depth > 0, "mismatched generic close!");
                paren_adjust = -1;
            }
            ':' => {
                saw_colon = true;
            }
            ' ' => {}
            ',' => {}
            _ => {
                saw_real_char = true;
            }
        }
        if saw_real_char {
            if !last_saw_real_char {
                cur_piece_start = idx;
                last_saw_real_char = true;
            }
        } else {
            if last_saw_real_char {
                cur_piece_end = idx;
                if gen_depth == 0 && paren_depth == 0 {
                    func_name_start = cur_piece_start;
                    func_name_end = cur_piece_end;
                }
                if gen_depth == 1
                    && type_name_start == 0
                    && func_name_start == 0
                    && (c == ' ' || gen_adjust != 0)
                {
                    type_name_start = cur_piece_start;
                    type_name_end = cur_piece_end;
                }
                last_saw_real_char = false;
            }
        }
        if saw_colon {
            if !last_saw_colon {
                last_saw_colon = true;
            } else if !last_saw_double_colon {
                last_saw_double_colon = true;
            } else {
                unreachable!("triple colon???");
            }
        } else {
            last_saw_colon = false;
            last_saw_double_colon = false;
        }
        gen_depth += gen_adjust;
        paren_depth += paren_adjust;
    }

    if last_saw_real_char {
        cur_piece_end = func.len();
        if gen_depth == 0 && paren_depth == 0 {
            func_name_start = cur_piece_start;
            func_name_end = cur_piece_end;
        }
    }

    let type_name = type_name_start..type_name_end;
    let func_name = func_name_start..func_name_end;
    let args = vec![];
    ParsedFuncName {
        type_name,
        func_name,
        _args: args,
    }
}

#[test]
fn test_parse_function_name() {
    let input = r###"<alloc::vec::Vec<clap::builder::possible_value::PossibleValue> as core::iter::traits::collect::Extend<clap::builder::possible_value::PossibleValue>>::extend::<core::iter::adapters::map::Map<core::iter::adapters::filter_map::FilterMap<core::slice::iter::Iter<minidumper_test::Signal>, <minidumper_test::Signal as clap::derive::ValueEnum>::to_possible_value>, <clap::builder::arg::Arg>::possible_values<core::iter::adapters::filter_map::FilterMap<core::slice::iter::Iter<minidumper_test::Signal>, <minidumper_test::Signal as clap::derive::ValueEnum>::to_possible_value>, clap::builder::possible_value::PossibleValue>::{closure#0}>>"###;
    let parsed = parse_function_name(input);
    assert_eq!(&input[parsed.type_name], "Vec");
    assert_eq!(&input[parsed.func_name], "extend");
}
