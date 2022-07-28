#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use clap::Parser;
use eframe::egui;
use egui::{Color32, Ui, Vec2};
use egui_extras::{Size, TableBuilder};
use logger::MapLogger;
use memmap2::Mmap;
use minidump::{format::MINIDUMP_STREAM_TYPE, system_info::PointerWidth, Minidump, Module};
use minidump_common::utils::basename;
use minidump_processor::{CallStack, ProcessState, StackFrame};
use processor::{
    MaybeMinidump, MaybeProcessed, MinidumpAnalysis, ProcessDump, ProcessingStatus, ProcessorTask,
};
use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};
use tracing_subscriber::prelude::*;
use ui_logs::LogUiState;
use ui_processed::ProcessedUiState;
use ui_raw_dump::RawDumpUiState;

pub mod logger;
pub mod processor;
mod ui_logs;
mod ui_processed;
mod ui_raw_dump;
mod ui_settings;

struct MyApp {
    logger: MapLogger,
    settings: Settings,
    tab: Tab,
    raw_dump_ui_state: RawDumpUiState,
    processed_ui_state: ProcessedUiState,
    log_ui_state: LogUiState,

    cur_status: ProcessingStatus,
    last_status: ProcessingStatus,
    minidump: MaybeMinidump,
    processed: MaybeProcessed,
    pointer_width: PointerWidth,

    task_sender: Arc<(Mutex<Option<ProcessorTask>>, Condvar)>,
    analysis_state: Arc<MinidumpAnalysis>,
}

struct Settings {
    available_paths: Vec<PathBuf>,
    picked_path: Option<String>,
    symbol_paths: Vec<(String, bool)>,
    symbol_urls: Vec<(String, bool)>,
    symbol_cache: (String, bool),
    http_timeout_secs: String,
    raw_dump_brief: bool,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Tab {
    Settings,
    Processed,
    RawDump,
    Logs,
}

#[derive(Parser)]
struct Cli {
    #[clap(action, long)]
    symbols_url: Vec<String>,
    #[clap(action, long)]
    symbols_path: Vec<String>,
    #[clap(action)]
    minidumps: Vec<PathBuf>,
}

const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 1000;

fn main() {
    let cli = Cli::parse();
    let available_paths = cli.minidumps;
    let symbol_paths = if cli.symbols_path.is_empty() {
        vec![(String::new(), true)]
    } else {
        cli.symbols_path.into_iter().map(|p| (p, true)).collect()
    };
    let symbol_urls = if cli.symbols_url.is_empty() {
        vec![
            ("https://symbols.mozilla.org/".to_string(), true),
            (
                "https://msdl.microsoft.com/download/symbols/".to_string(),
                false,
            ),
            (String::new(), true),
        ]
    } else {
        cli.symbols_url.into_iter().map(|p| (p, true)).collect()
    };

    let logger = MapLogger::new();

    tracing_subscriber::registry().with(logger.clone()).init();

    let options = eframe::NativeOptions {
        drag_and_drop_support: true,
        initial_window_size: Some(Vec2::new(1000.0, 800.0)),
        ..Default::default()
    };
    let task_sender = Arc::new((Mutex::new(None::<ProcessorTask>), Condvar::new()));
    let task_receiver = task_sender.clone();
    let analysis_receiver = Arc::new(MinidumpAnalysis::default());
    let analysis_sender = analysis_receiver.clone();
    let logger_handle = logger.clone();

    // Start the processor background thread
    let _handle = std::thread::spawn(move || {
        processor::run_processor(task_receiver, analysis_sender, logger_handle);
    });

    // Launch the app
    eframe::run_native(
        "rust-minidump debugger",
        options,
        Box::new(|_cc| {
            Box::new(MyApp {
                logger,
                tab: Tab::Settings,
                settings: Settings {
                    available_paths,
                    picked_path: None,
                    raw_dump_brief: true,
                    symbol_urls,
                    symbol_paths,
                    symbol_cache: (
                        std::env::temp_dir()
                            .join("minidump-cache")
                            .to_string_lossy()
                            .into_owned(),
                        true,
                    ),
                    http_timeout_secs: DEFAULT_HTTP_TIMEOUT_SECS.to_string(),
                },
                raw_dump_ui_state: RawDumpUiState { cur_stream: 0 },
                processed_ui_state: ProcessedUiState {
                    cur_thread: 0,
                    cur_frame: 0,
                },
                log_ui_state: LogUiState {
                    cur_thread: None,
                    cur_frame: None,
                },

                cur_status: ProcessingStatus::NoDump,
                last_status: ProcessingStatus::NoDump,
                minidump: None,
                processed: None,
                pointer_width: PointerWidth::Unknown,

                task_sender,
                analysis_state: analysis_receiver,
            })
        }),
    );
}

// The main even loop
impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_processor_state();
        self.update_ui(ctx);
        self.last_status = self.cur_status;
    }
}

// Core State Updating
impl MyApp {
    fn poll_processor_state(&mut self) {
        // Fetch updates from processing thread
        let new_minidump = self.analysis_state.minidump.lock().unwrap().take();
        if let Some(dump) = new_minidump {
            if let Ok(dump) = &dump {
                self.process_dump(dump.clone());
            }
            self.minidump = Some(dump);
        }

        if self.cur_status < ProcessingStatus::Done {
            let stats = self.analysis_state.stats.lock().unwrap();
            let partial = stats.processor_stats.take_unwalked_result();
            if let Some(state) = partial {
                self.pointer_width = state.system_info.cpu.pointer_width();
                if self.tab == Tab::Settings && self.cur_status <= ProcessingStatus::RawProcessing {
                    self.tab = Tab::Processed;
                }
                self.cur_status = ProcessingStatus::Symbolicating;

                if let Some(crashed_thread) = state.requesting_thread {
                    self.processed_ui_state.cur_thread = crashed_thread;
                }
                self.processed = Some(Ok(Arc::new(state)));
            }

            if let Some(partial) = self.processed.as_mut().and_then(|p| p.as_mut().ok()) {
                let partial = Arc::make_mut(partial);
                stats.processor_stats.drain_new_frames(|frame| {
                    let thread = &mut partial.threads[frame.thread_idx];
                    if thread.frames.len() > frame.frame_idx {
                        // Allows us to overwrite the old context frame
                        thread.frames[frame.frame_idx] = frame.frame;
                    } else if thread.frames.len() == frame.frame_idx {
                        thread.frames.push(frame.frame);
                    } else {
                        unreachable!("stack frames arrived in wrong order??");
                    }
                });
            }
        }

        let new_processed = self.analysis_state.processed.lock().unwrap().take();
        if let Some(processed) = new_processed {
            if self.tab == Tab::Settings && self.cur_status <= ProcessingStatus::RawProcessing {
                self.tab = Tab::Processed;
            }
            self.cur_status = ProcessingStatus::Done;
            if let Ok(state) = &processed {
                self.pointer_width = state.system_info.cpu.pointer_width();
                if let Some(crashed_thread) = state.requesting_thread {
                    self.processed_ui_state.cur_thread = crashed_thread;
                }
            }
            self.processed = Some(processed);
        }
    }

    fn set_path(&mut self, idx: usize) {
        let path = self.settings.available_paths[idx].clone();
        self.cur_status = ProcessingStatus::ReadingDump;
        self.settings.picked_path = Some(path.display().to_string());
        let (lock, condvar) = &*self.task_sender;
        let mut new_task = lock.lock().unwrap();
        *new_task = Some(ProcessorTask::ReadDump(path));
        self.minidump = None;
        self.processed = None;
        self.tab = Tab::Settings;
        condvar.notify_one();
    }

    fn process_dump(&mut self, dump: Arc<Minidump<'static, Mmap>>) {
        let (lock, condvar) = &*self.task_sender;
        let mut new_task = lock.lock().unwrap();
        self.cur_status = ProcessingStatus::RawProcessing;

        let symbol_paths = self
            .settings
            .symbol_paths
            .iter()
            .filter(|(path, enabled)| *enabled && !path.trim().is_empty())
            .map(|(path, _enabled)| PathBuf::from(path))
            .collect();
        let symbol_urls = self
            .settings
            .symbol_urls
            .iter()
            .filter(|(url, enabled)| *enabled && !url.trim().is_empty())
            .map(|(url, _enabled)| url.to_owned())
            .collect();
        let (raw_cache, cache_enabled) = &self.settings.symbol_cache;
        let clear_cache = !cache_enabled;
        let symbol_cache = PathBuf::from(raw_cache);
        let http_timeout_secs = self
            .settings
            .http_timeout_secs
            .parse::<u64>()
            .unwrap_or(DEFAULT_HTTP_TIMEOUT_SECS);
        *new_task = Some(ProcessorTask::ProcessDump(ProcessDump {
            dump,
            symbol_paths,
            symbol_urls,
            symbol_cache,
            clear_cache,
            http_timeout_secs,
        }));
        condvar.notify_one();
    }

    fn cancel_processing(&mut self) {
        let (lock, condvar) = &*self.task_sender;
        let mut new_task = lock.lock().unwrap();
        *new_task = Some(ProcessorTask::Cancel);
        condvar.notify_one();
    }
}

// Main UI: sets up tabs and then shells out to the current view
//
// All the different views have been split off into different files
// because they don't care about eachother and things were getting way
// out of control with all these unrelated UIs together!
impl MyApp {
    fn update_ui(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Settings, "settings");
                if self.cur_status >= ProcessingStatus::RawProcessing {
                    ui.selectable_value(&mut self.tab, Tab::RawDump, "raw dump");
                }
                if self.cur_status >= ProcessingStatus::Symbolicating {
                    ui.selectable_value(&mut self.tab, Tab::Processed, "processed");
                }
                if self.cur_status >= ProcessingStatus::RawProcessing {
                    ui.selectable_value(&mut self.tab, Tab::Logs, "logs");
                }
            });
            ui.separator();
            match self.tab {
                Tab::Settings => self.ui_settings(ui, ctx),
                Tab::RawDump => self.ui_raw_dump(ui, ctx),
                Tab::Processed => self.ui_processed(ui, ctx),
                Tab::Logs => self.ui_logs(ui, ctx),
            }
        });
    }

    fn format_addr(&self, addr: u64) -> String {
        match self.pointer_width {
            minidump::system_info::PointerWidth::Bits32 => format!("0x{:08x}", addr),
            minidump::system_info::PointerWidth::Bits64 => format!("0x{:016x}", addr),
            minidump::system_info::PointerWidth::Unknown => format!("0x{:08x}", addr),
        }
    }
}

fn listing(
    ui: &mut Ui,
    ctx: &egui::Context,
    id: u64,
    items: impl IntoIterator<Item = (String, String)>,
) {
    ui.push_id(id, |ui| {
        let mono_font = egui::style::TextStyle::Monospace.resolve(ui.style());
        let body_font = egui::style::TextStyle::Body.resolve(ui.style());
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right().with_cross_align(egui::Align::Center))
            .column(Size::initial(120.0).at_least(40.0))
            .column(Size::remainder().at_least(60.0))
            .clip(false)
            .resizable(true)
            .body(|mut body| {
                let widths = body.widths();
                let col1_width = widths[0];
                let col2_width = widths[1];
                for (lhs, rhs) in items {
                    let (col1, col2, row_height) = {
                        let fonts = ctx.fonts();
                        let col1 = fonts.layout(lhs, body_font.clone(), Color32::BLACK, col1_width);
                        let col2 = fonts.layout(rhs, mono_font.clone(), Color32::BLACK, col2_width);
                        let row_height = col1.rect.height().max(col2.rect.height()) + 6.0;
                        (col1, col2, row_height)
                    };
                    body.row(row_height, |mut row| {
                        row.col(|ui| {
                            ui.label(col1);
                        });
                        row.col(|ui| {
                            ui.label(col2);
                        });
                    });
                }
            });
    });
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
            write!(f, "{}: {}", sourcename(source_file), source_line,)?;
        } else if let Some(function_base) = frame.function_base {
            write!(
                f,
                "{} + {:#x}",
                basename(&module.code_file()),
                addr - function_base
            )?;
        }
    }
    Ok(())
}

fn frame_signature_from_indices(
    state: &ProcessState,
    thread_idx: Option<usize>,
    frame_idx: Option<usize>,
) -> String {
    use std::fmt::Write;
    fn frame_signature_from_indices_inner(
        buf: &mut String,
        state: &ProcessState,
        thread_idx: Option<usize>,
        frame_idx: Option<usize>,
    ) -> Option<()> {
        let thread_idx = thread_idx?;
        let frame_idx = frame_idx?;
        let thread = state.threads.get(thread_idx)?;
        let frame = thread.frames.get(frame_idx)?;
        frame_signature(buf, frame).ok()?;
        Some(())
    }

    if frame_idx.is_none() {
        return "<no frame>".to_owned();
    }
    let mut buf = String::new();
    write!(&mut buf, "{}: ", frame_idx.unwrap()).unwrap();
    let _ = frame_signature_from_indices_inner(&mut buf, state, thread_idx, frame_idx);
    buf
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
                basename(&module.code_file()),
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
