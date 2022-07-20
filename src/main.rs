#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use eframe::egui;
use egui::{ComboBox, ScrollArea, Ui};
use egui_extras::{Size, TableBuilder};
use memmap2::Mmap;
use minidump::{format::MINIDUMP_STREAM_TYPE, Minidump, Module};
use minidump_common::utils::basename;
use minidump_processor::{
    http_symbol_supplier, CallStack, ProcessState, ProcessorOptions, StackFrame, Symbolizer,
};
use num_traits::FromPrimitive;
use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};

fn main() {
    let options = eframe::NativeOptions {
        drag_and_drop_support: true,
        ..Default::default()
    };
    let path_sender = Arc::new((Mutex::new(None::<PathBuf>), Condvar::new()));
    let path_receiver = path_sender.clone();
    let analysis_receiver = Arc::new(MinidumpAnalysis {
        minidump: Arc::new(Mutex::new(None)),
        processed: Arc::new(Mutex::new(None)),
        status: Arc::new(Mutex::new(ProcessingStatus::NoDump)),
    });
    let analysis_sender = analysis_receiver.clone();
    let _handle = std::thread::spawn(move || {
        let (lock, condvar) = &*path_receiver;
        let path = {
            let mut path = lock.lock().unwrap();
            if path.is_none() {
                path = condvar.wait(path).unwrap();
            }
            path.take().unwrap()
        };

        *analysis_sender.status.lock().unwrap() = ProcessingStatus::ReadingDump;

        let dump = Minidump::read_path(path).map(Arc::new);
        let ok_dump = dump.as_ref().ok().cloned();
        *analysis_sender.minidump.lock().unwrap() = Some(dump);
        if let Some(dump) = ok_dump {
            *analysis_sender.status.lock().unwrap() = ProcessingStatus::RawProcessing;
            let raw_processed = process_minidump(&dump, false).map(Arc::new);
            *analysis_sender.processed.lock().unwrap() = Some(raw_processed);

            *analysis_sender.status.lock().unwrap() = ProcessingStatus::Symbolicating;
            let symbolicated = process_minidump(&dump, true).map(Arc::new);
            *analysis_sender.processed.lock().unwrap() = Some(symbolicated);
        }
        *analysis_sender.status.lock().unwrap() = ProcessingStatus::Done;
    });
    eframe::run_native(
        "rust-minidump debugger",
        options,
        Box::new(|_cc| {
            Box::new(MyApp {
                tab: Tab::Settings,
                settings: Settings { picked_path: None },
                raw_dump_ui_state: RawDumpUiState { cur_stream: 0 },
                processed_ui_state: ProcessedUiState { cur_thread: 0 },

                cur_status: ProcessingStatus::NoDump,
                last_status: ProcessingStatus::NoDump,
                minidump: None,
                processed: None,

                path_sender,
                analysis_state: analysis_receiver,
            })
        }),
    );
}

struct MyApp {
    settings: Settings,
    tab: Tab,
    raw_dump_ui_state: RawDumpUiState,
    processed_ui_state: ProcessedUiState,

    cur_status: ProcessingStatus,
    last_status: ProcessingStatus,
    minidump: MaybeMinidump,
    processed: MaybeProcessed,

    path_sender: Arc<(Mutex<Option<PathBuf>>, Condvar)>,
    analysis_state: Arc<MinidumpAnalysis>,
}

struct RawDumpUiState {
    cur_stream: usize,
}

struct ProcessedUiState {
    cur_thread: usize,
}

struct Settings {
    picked_path: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ProcessingStatus {
    NoDump,
    ReadingDump,
    RawProcessing,
    Symbolicating,
    Done,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Tab {
    Settings,
    Processed,
    RawDump,
}

type MaybeMinidump = Option<Result<Arc<Minidump<'static, Mmap>>, minidump::Error>>;
type MaybeProcessed = Option<Result<Arc<ProcessState>, minidump_processor::ProcessError>>;

struct MinidumpAnalysis {
    minidump: Arc<Mutex<MaybeMinidump>>,
    processed: Arc<Mutex<MaybeProcessed>>,
    status: Arc<Mutex<ProcessingStatus>>,
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Fetch updates from processing thread
        let status = *self.analysis_state.status.lock().unwrap();
        self.cur_status = status;
        let new_minidump = self.analysis_state.minidump.lock().unwrap().take();
        if new_minidump.is_some() {
            self.minidump = new_minidump;
        }
        let new_processed = self.analysis_state.processed.lock().unwrap().take();
        if let Some(processed) = new_processed {
            if self.tab == Tab::Settings {
                self.tab = Tab::Processed;
            }
            if let Ok(state) = &processed {
                if let Some(crashed_thread) = state.requesting_thread {
                    self.processed_ui_state.cur_thread = crashed_thread;
                }
            }
            self.processed = Some(processed);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Settings, "settings");
                if status >= ProcessingStatus::RawProcessing {
                    ui.selectable_value(&mut self.tab, Tab::RawDump, "raw dump");
                }
                if status >= ProcessingStatus::Symbolicating {
                    ui.selectable_value(&mut self.tab, Tab::Processed, "processed");
                }
            });
            ui.separator();
            match self.tab {
                Tab::Settings => self.update_settings(ui, ctx),
                Tab::RawDump => self.update_raw_dump(ui, ctx),
                Tab::Processed => self.update_processed(ui, ctx),
            }
        });
        self.last_status = status;
    }
}

impl MyApp {
    fn set_path(&mut self, path: PathBuf) {
        self.settings.picked_path = Some(path.display().to_string());
        let (lock, condvar) = &*self.path_sender;
        let mut new_path = lock.lock().unwrap();
        *new_path = Some(path);
        condvar.notify_one();
    }

    fn update_settings(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let message = match self.cur_status {
            ProcessingStatus::NoDump => "Select or drop a minidump!",
            ProcessingStatus::ReadingDump => "Reading minidump...",
            ProcessingStatus::RawProcessing => "Processing minidump...",
            ProcessingStatus::Symbolicating => "Minidump processed!",
            ProcessingStatus::Done => "Minidump processed!",
        };
        ui.label(message);

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

        preview_files_being_dropped(ctx);

        // Collect dropped files:
        if let Some(dropped) = ctx.input().raw.dropped_files.get(0) {
            if let Some(path) = &dropped.path {
                self.set_path(path.clone());
            }
        }
    }

    fn update_raw_dump(&mut self, ui: &mut Ui, _ctx: &egui::Context) {
        if let Some(minidump) = &self.minidump {
            match minidump {
                Ok(dump) => {
                    self.update_raw_dump_good(ui, &dump.clone());
                }
                Err(e) => {
                    ui.label("Minidump couldn't be read!");
                    ui.label(e.to_string());
                }
            }
        }
    }

    fn update_raw_dump_good(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right().with_cross_align(egui::Align::Center))
            .column(Size::initial(40.0).at_least(40.0))
            .column(Size::initial(80.0).at_least(40.0))
            .column(Size::initial(80.0).at_least(40.0))
            .column(Size::remainder())
            .resizable(true)
            .header(20.0, |mut header| {
                header.col(|ui| {
                    ui.heading("Idx");
                });
                header.col(|ui| {
                    ui.heading("Type");
                });
                header.col(|ui| {
                    ui.heading("Vendor");
                });
                header.col(|ui| {
                    ui.heading("Name");
                });
            })
            .body(|mut body| {
                for (i, stream) in dump.all_streams().enumerate() {
                    let row_height = 18.0;
                    body.row(row_height, |mut row| {
                        row.col(|ui| {
                            ui.centered_and_justified(|ui| {
                                ui.label(i.to_string());
                            });
                        });
                        row.col(|ui| {
                            ui.centered_and_justified(|ui| {
                                ui.label(format!("0x{:08x}", stream.stream_type));
                            });
                        });
                        row.col(|ui| {
                            ui.centered_and_justified(|ui| {
                                ui.label(stream_vendor(stream.stream_type));
                            });
                        });
                        row.col(|ui| {
                            let label = if let Some(stream_type) =
                                MINIDUMP_STREAM_TYPE::from_u32(stream.stream_type)
                            {
                                format!("{:?}", stream_type)
                            } else {
                                "<unknown>".to_string()
                            };
                            ui.label(label);
                        });
                    })
                }
            })
    }

    fn update_processed(&mut self, ui: &mut Ui, _ctx: &egui::Context) {
        if let Some(Err(e)) = &self.minidump {
            ui.label("Minidump couldn't be read!");
            ui.label(e.to_string());
            return;
        }
        if let Some(state) = &self.processed {
            match state {
                Ok(state) => {
                    self.update_processed_good(ui, &state.clone());
                }
                Err(e) => {
                    ui.label("Minidump couldn't be processed!");
                    ui.label(e.to_string());
                }
            }
        }
    }

    fn update_processed_good(&mut self, ui: &mut Ui, state: &ProcessState) {
        let is_symbolicated = self.cur_status == ProcessingStatus::Done;

        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if let Some(reason) = state.crash_reason {
                    ui.horizontal(|ui| {
                        ui.label("Crash Reason:");
                        ui.monospace(reason.to_string());
                    });
                }
                if let Some(addr) = state.crash_address {
                    ui.horizontal(|ui| {
                        ui.label("Crash Address:");
                        ui.monospace(format!("0x{:08x}", addr));
                    });
                }
                if let Some(crashing_thread) = state.requesting_thread {
                    if let Some(stack) = state.threads.get(crashing_thread) {
                        ui.horizontal(|ui| {
                            ui.label("Crashing Thread: ");
                            ui.label(threadname(stack));
                        });
                    }
                }
                ComboBox::from_label("Thread")
                    .width(200.0)
                    .selected_text(
                        state
                            .threads
                            .get(self.processed_ui_state.cur_thread)
                            .map(threadname)
                            .unwrap_or_default(),
                    )
                    .show_ui(ui, |ui| {
                        for (idx, stack) in state.threads.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.processed_ui_state.cur_thread,
                                idx,
                                threadname(stack),
                            );
                        }
                    });

                if let Some(stack) = state.threads.get(self.processed_ui_state.cur_thread) {
                    if is_symbolicated {
                        TableBuilder::new(ui)
                            .striped(true)
                            .cell_layout(
                                egui::Layout::left_to_right().with_cross_align(egui::Align::Center),
                            )
                            .column(Size::initial(60.0).at_least(40.0))
                            .column(Size::initial(80.0).at_least(40.0))
                            .column(Size::remainder())
                            .column(Size::initial(80.0).at_least(40.0))
                            .column(Size::initial(60.0).at_least(40.0))
                            .resizable(true)
                            .header(20.0, |mut header| {
                                header.col(|ui| {
                                    ui.heading("Frame");
                                });
                                header.col(|ui| {
                                    ui.heading("Module");
                                });
                                header.col(|ui| {
                                    ui.heading("Signature");
                                });
                                header.col(|ui| {
                                    ui.heading("Source");
                                });
                                header.col(|ui| {
                                    ui.heading("Trust");
                                });
                            })
                            .body(|mut body| {
                                for (i, frame) in stack.frames.iter().enumerate() {
                                    let is_thick = false; // thick_row(row_index);
                                    let row_height = if is_thick { 30.0 } else { 18.0 };

                                    body.row(row_height, |mut row| {
                                        row.col(|ui| {
                                            ui.centered_and_justified(|ui| {
                                                ui.label(i.to_string());
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
                                            frame_signature(&mut label, frame).unwrap();
                                            // ui.style_mut().wrap = Some(false);
                                            ui.label(label);
                                        });
                                        row.col(|ui| {
                                            let mut label = String::new();
                                            frame_source(&mut label, frame).unwrap();
                                            // ui.style_mut().wrap = Some(false);
                                            ui.label(label);
                                        });
                                        row.col(|ui| {
                                            let trust = match frame.trust {
                                                minidump_processor::FrameTrust::None => "none",
                                                minidump_processor::FrameTrust::Scan => "scan",
                                                minidump_processor::FrameTrust::CfiScan => {
                                                    "cfi scan"
                                                }
                                                minidump_processor::FrameTrust::FramePointer => {
                                                    "frame pointer"
                                                }
                                                minidump_processor::FrameTrust::CallFrameInfo => {
                                                    "cfi"
                                                }
                                                minidump_processor::FrameTrust::PreWalked => {
                                                    "prewalked"
                                                }
                                                minidump_processor::FrameTrust::Context => {
                                                    "context"
                                                }
                                            };
                                            ui.centered_and_justified(|ui| {
                                                ui.label(trust);
                                            });
                                        });
                                    });
                                }
                            });
                    } else {
                        ui.label("stackwalking in progress...");
                    }
                }
            });
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

fn process_minidump(
    minidump: &Minidump<Mmap>,
    symbolicate: bool,
) -> Result<ProcessState, minidump_processor::ProcessError> {
    // Configure the symbolizer and processor
    let symbols_urls = if symbolicate {
        vec!["https://symbols.mozilla.org".to_string()]
    } else {
        vec![]
    };
    let symbols_paths = vec![];
    let mut symbols_cache = std::env::temp_dir();
    symbols_cache.push("minidump-cache");
    let symbols_tmp = std::env::temp_dir();
    let timeout = std::time::Duration::from_secs(1000);

    // Use ProcessorOptions for detailed configuration
    let options = ProcessorOptions::default();

    // Specify a symbol supplier (here we're using the most powerful one, the http supplier)
    let provider = Symbolizer::new(http_symbol_supplier(
        symbols_paths,
        symbols_urls,
        symbols_cache,
        symbols_tmp,
        timeout,
    ));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let state = runtime.block_on(async {
        let state =
            minidump_processor::process_minidump_with_options(&minidump, &provider, options).await;
        state
    })?;

    Ok(state)
}

fn threadname(stack: &CallStack) -> String {
    if let Some(name) = &stack.thread_name {
        format!("{} ({})", name, stack.thread_id)
    } else {
        format!("({})", stack.thread_id)
    }
}

fn sourcename(file: &str) -> &str {
    let base = basename(file);
    match base.rsplit_once(':') {
        Some((lhs, _rhs)) => lhs,
        None => base,
    }
}

fn stream_vendor(stream_type: u32) -> &'static str {
    if stream_type <= MINIDUMP_STREAM_TYPE::LastReservedStream as u32 {
        "Official"
    } else {
        match stream_type & 0xFFFF0000 {
            0x4767_0000 => "Google",
            0x4d7a_0000 => "Mozilla",
            _ => "Unknown",
        }
    }
}

fn frame_source(f: &mut impl std::fmt::Write, frame: &StackFrame) -> Result<(), std::fmt::Error> {
    let addr = frame.instruction;
    if let Some(ref module) = frame.module {
        if let (&Some(ref source_file), &Some(ref source_line), &Some(ref _source_line_base)) = (
            &frame.source_file_name,
            &frame.source_line,
            &frame.source_line_base,
        ) {
            write!(f, "{} : {}", sourcename(source_file), source_line,)?;
        } else if let Some(function_base) = frame.function_base {
            write!(
                f,
                "{} + {:#x}",
                basename(&*module.code_file()),
                addr - function_base
            )?;
        }
    }
    Ok(())
}

fn frame_signature(
    f: &mut impl std::fmt::Write,
    frame: &StackFrame,
) -> Result<(), std::fmt::Error> {
    let addr = frame.instruction;
    if let Some(ref module) = frame.module {
        if let (&Some(ref function), &Some(ref _function_base)) =
            (&frame.function_name, &frame.function_base)
        {
            write!(f, "{}", function)?;
        } else {
            write!(
                f,
                "{} + {:#x}",
                basename(&*module.code_file()),
                addr - module.base_address()
            )?;
        }
    } else {
        write!(f, "{:#x}", addr)?;

        // List off overlapping unloaded modules.

        // First we need to collect them up by name so that we can print
        // all the overlaps from one module together and dedupe them.

        for (name, offsets) in &frame.unloaded_modules {
            write!(f, " (unloaded {}@", name)?;
            let mut first = true;
            for offset in offsets {
                if first {
                    write!(f, "{:#x}", offset)?;
                } else {
                    // `|` is our separator for multiple entries
                    write!(f, "|{:#x}", offset)?;
                }
                first = false;
            }
            write!(f, ")")?;
        }
    }

    Ok(())
}
