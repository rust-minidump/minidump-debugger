#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use eframe::egui;
use egui::{ComboBox, TextStyle, Ui, Vec2};
use egui_extras::{Size, StripBuilder, TableBuilder};
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

use std::io;

struct MySink(Mutex<io::BufWriter<Vec<u8>>>);
impl io::Write for &MySink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}
impl MySink {
    fn clear(&self) {
        self.0.lock().unwrap().get_mut().clear()
    }
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().get_ref().clone()
    }
}

fn main() {
    let logger = Arc::new(MySink(Mutex::new(io::BufWriter::new(Vec::<u8>::new()))));

    tracing_subscriber::fmt::fmt()
        .with_max_level(tracing::level_filters::LevelFilter::TRACE)
        .with_target(false)
        .without_time()
        .with_ansi(false)
        .with_writer(logger.clone())
        .init();

    let options = eframe::NativeOptions {
        drag_and_drop_support: true,
        initial_window_size: Some(Vec2::new(1000.0, 800.0)),
        ..Default::default()
    };
    let task_sender = Arc::new((Mutex::new(None::<ProcessorTask>), Condvar::new()));
    let task_receiver = task_sender.clone();
    let analysis_receiver = Arc::new(MinidumpAnalysis {
        minidump: Arc::new(Mutex::new(None)),
        processed: Arc::new(Mutex::new(None)),
        status: Arc::new(Mutex::new(ProcessingStatus::NoDump)),
    });
    let analysis_sender = analysis_receiver.clone();
    let _handle = std::thread::spawn(move || loop {
        let (lock, condvar) = &*task_receiver;
        let task = {
            let mut task = lock.lock().unwrap();
            if task.is_none() {
                task = condvar.wait(task).unwrap();
            }
            task.take().unwrap()
        };

        match task {
            ProcessorTask::Cancel => {
                // Do nothing, this is only relevant within the other tasks, now we're just clearing it out
            }
            ProcessorTask::ReadDump(path) => {
                *analysis_sender.status.lock().unwrap() = ProcessingStatus::ReadingDump;
                let dump = Minidump::read_path(path).map(Arc::new);
                *analysis_sender.minidump.lock().unwrap() = Some(dump);
            }
            ProcessorTask::ProcessDump(settings) => {
                *analysis_sender.status.lock().unwrap() = ProcessingStatus::RawProcessing;
                let raw_processed =
                    process_minidump(&task_receiver, &settings, false).map(Arc::new);
                *analysis_sender.processed.lock().unwrap() = Some(raw_processed);

                *analysis_sender.status.lock().unwrap() = ProcessingStatus::Symbolicating;
                let symbolicated = process_minidump(&task_receiver, &settings, true).map(Arc::new);
                *analysis_sender.processed.lock().unwrap() = Some(symbolicated);
                *analysis_sender.status.lock().unwrap() = ProcessingStatus::Done;
            }
        }
    });
    eframe::run_native(
        "rust-minidump debugger",
        options,
        Box::new(|_cc| {
            Box::new(MyApp {
                logger,
                tab: Tab::Settings,
                settings: Settings {
                    picked_path: None,
                    raw_dump_brief: true,
                    symbol_urls: vec![
                        ("https://symbols.mozilla.org/".to_string(), true),
                        (
                            "https://msdl.microsoft.com/download/symbols/".to_string(),
                            false,
                        ),
                        (String::new(), true),
                    ],
                    symbol_paths: vec![(String::new(), true)],
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

                cur_status: ProcessingStatus::NoDump,
                last_status: ProcessingStatus::NoDump,
                minidump: None,
                processed: None,

                task_sender,
                analysis_state: analysis_receiver,
            })
        }),
    );
}

struct MyApp {
    logger: Arc<MySink>,
    settings: Settings,
    tab: Tab,
    raw_dump_ui_state: RawDumpUiState,
    processed_ui_state: ProcessedUiState,

    cur_status: ProcessingStatus,
    last_status: ProcessingStatus,
    minidump: MaybeMinidump,
    processed: MaybeProcessed,

    task_sender: Arc<(Mutex<Option<ProcessorTask>>, Condvar)>,
    analysis_state: Arc<MinidumpAnalysis>,
}

struct RawDumpUiState {
    cur_stream: usize,
}

struct ProcessedUiState {
    cur_thread: usize,
    cur_frame: usize,
}

struct Settings {
    picked_path: Option<String>,
    symbol_paths: Vec<(String, bool)>,
    symbol_urls: Vec<(String, bool)>,
    symbol_cache: (String, bool),
    http_timeout_secs: String,
    raw_dump_brief: bool,
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
    Logs,
}

enum ProcessorTask {
    Cancel,
    ReadDump(PathBuf),
    ProcessDump(ProcessDump),
}

struct ProcessDump {
    dump: Arc<Minidump<'static, Mmap>>,
    symbol_paths: Vec<PathBuf>,
    symbol_urls: Vec<String>,
    symbol_cache: PathBuf,
    clear_cache: bool,
    http_timeout_secs: u64,
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
        if let Some(dump) = new_minidump {
            if let Ok(dump) = &dump {
                self.process_dump(dump.clone());
            }
            self.minidump = Some(dump);
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
                if status >= ProcessingStatus::RawProcessing {
                    ui.selectable_value(&mut self.tab, Tab::Logs, "logs");
                }
            });
            ui.separator();
            match self.tab {
                Tab::Settings => self.update_settings(ui, ctx),
                Tab::RawDump => self.update_raw_dump(ui, ctx),
                Tab::Processed => self.update_processed(ui, ctx),
                Tab::Logs => self.update_logs(ui, ctx),
            }
        });
        self.last_status = status;
    }
}

const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 1000;

impl MyApp {
    fn set_path(&mut self, path: PathBuf) {
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
        if self.cur_status >= ProcessingStatus::Done {
            self.logger.clear();
        }
        let (lock, condvar) = &*self.task_sender;
        let mut new_task = lock.lock().unwrap();

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

    fn update_settings(&mut self, ui: &mut Ui, ctx: &egui::Context) {
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
        StripBuilder::new(ui)
            .size(Size::exact(180.0))
            .size(Size::remainder())
            .horizontal(|mut strip| {
                strip.cell(|ui| {
                    self.update_raw_dump_streams(ui, dump);
                });
                strip.cell(|ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        if self.raw_dump_ui_state.cur_stream == 0 {
                            self.update_raw_dump_top_level(ui, dump);
                            return;
                        }
                        let stream = dump
                            .all_streams()
                            .nth(self.raw_dump_ui_state.cur_stream - 1)
                            .and_then(|entry| MINIDUMP_STREAM_TYPE::from_u32(entry.stream_type));
                        if let Some(stream) = stream {
                            use MINIDUMP_STREAM_TYPE::*;
                            match stream {
                                SystemInfoStream => self.update_raw_dump_system_info(ui, dump),
                                ThreadNamesStream => self.update_raw_dump_thread_names(ui, dump),
                                MiscInfoStream => self.update_raw_dump_misc_info(ui, dump),
                                ThreadListStream => self.update_raw_dump_thread_list(ui, dump),
                                AssertionInfoStream => {
                                    self.update_raw_dump_assertion_info(ui, dump)
                                }
                                BreakpadInfoStream => self.update_raw_dump_breakpad_info(ui, dump),
                                CrashpadInfoStream => self.update_raw_dump_crashpad_info(ui, dump),
                                ExceptionStream => self.update_raw_dump_exception(ui, dump),
                                ModuleListStream => self.update_raw_dump_module_list(ui, dump),
                                UnloadedModuleListStream => {
                                    self.update_raw_dump_unloaded_module_list(ui, dump)
                                }
                                MemoryListStream => self.update_raw_dump_memory_list(ui, dump),
                                MemoryInfoListStream => {
                                    self.update_raw_dump_memory_info_list(ui, dump)
                                }
                                /*
                                LinuxCpuInfo => self.update_raw_dump_linux_cpu_info(ui, dump),
                                LinuxEnviron => self.update_raw_dump_linux_environ(ui, dump),
                                LinuxLsbRelease => self.update_raw_dump_linux_lsb_release(ui, dump),
                                MozMacosCrashInfoStream => self.update_raw_dump_moz_macos_crash_info(ui, dump),
                                */
                                _ => {}
                            }
                        }
                    });
                });
            });
    }

    fn update_raw_dump_streams(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        ui.heading("Streams");
        ui.add_space(20.0);
        ui.selectable_value(&mut self.raw_dump_ui_state.cur_stream, 0, "<summary>");

        for (i, stream) in dump.all_streams().enumerate() {
            use MINIDUMP_STREAM_TYPE::*;
            let (supported, label) =
                if let Some(stream_type) = MINIDUMP_STREAM_TYPE::from_u32(stream.stream_type) {
                    let supported = match stream_type {
                        SystemInfoStream
                        | MiscInfoStream
                        | ThreadNamesStream
                        | ThreadListStream
                        | AssertionInfoStream
                        | BreakpadInfoStream
                        | CrashpadInfoStream
                        | ExceptionStream
                        | ModuleListStream
                        | UnloadedModuleListStream
                        | MemoryListStream
                        | MemoryInfoListStream => true,
                        _ => false,
                    };
                    (supported, format!("{:?}", stream_type))
                } else {
                    (false, "<unknown>".to_string())
                };

            ui.add_enabled_ui(supported, |ui| {
                ui.selectable_value(&mut self.raw_dump_ui_state.cur_stream, i + 1, label);
            });
        }
    }

    fn update_raw_dump_top_level(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        ui.heading("Minidump Streams");
        ui.add_space(20.0);

        let row_height = 18.0;
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right().with_cross_align(egui::Align::Center))
            .column(Size::initial(40.0).at_least(40.0))
            .column(Size::initial(80.0).at_least(40.0))
            .column(Size::initial(80.0).at_least(40.0))
            .column(Size::remainder().at_least(60.0))
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
                            use MINIDUMP_STREAM_TYPE::*;
                            let (supported, label) = if let Some(stream_type) =
                                MINIDUMP_STREAM_TYPE::from_u32(stream.stream_type)
                            {
                                let supported = match stream_type {
                                    SystemInfoStream
                                    | MiscInfoStream
                                    | ThreadNamesStream
                                    | ThreadListStream
                                    | AssertionInfoStream
                                    | BreakpadInfoStream
                                    | CrashpadInfoStream
                                    | ExceptionStream
                                    | ModuleListStream
                                    | UnloadedModuleListStream
                                    | MemoryListStream
                                    | MemoryInfoListStream => true,
                                    _ => false,
                                };
                                (supported, format!("{:?}", stream_type))
                            } else {
                                (false, "<unknown>".to_string())
                            };

                            if supported {
                                if ui.link(label).clicked() {
                                    self.raw_dump_ui_state.cur_stream = i + 1;
                                }
                            } else {
                                ui.label(label);
                            }
                        });
                    })
                }
            });

        ui.add_space(20.0);
        ui.separator();
        ui.heading("Minidump Metadata");
        ui.add_space(10.0);
        let mut bytes = Vec::new();
        dump.print(&mut bytes).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.add(
            egui::TextEdit::multiline(&mut &*text)
                .font(TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
    }

    fn update_raw_dump_misc_info(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_stream::<minidump::MinidumpMiscInfo>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        let mut bytes = Vec::new();
        stream.print(&mut bytes).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.add(
            egui::TextEdit::multiline(&mut &*text)
                .font(TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
    }

    fn update_raw_dump_thread_names(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_stream::<minidump::MinidumpThreadNames>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        let mut bytes = Vec::new();
        stream.print(&mut bytes).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.add(
            egui::TextEdit::multiline(&mut &*text)
                .font(TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
    }

    fn update_raw_dump_system_info(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_stream::<minidump::MinidumpSystemInfo>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        let mut bytes = Vec::new();
        stream.print(&mut bytes).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.add(
            egui::TextEdit::multiline(&mut &*text)
                .font(TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
    }

    fn update_raw_dump_thread_list(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let brief = self.settings.raw_dump_brief;
        let stream = dump.get_stream::<minidump::MinidumpThreadList>();
        let memory = dump.get_stream::<minidump::MinidumpMemoryList>();
        let system = dump.get_stream::<minidump::MinidumpSystemInfo>();
        let misc = dump.get_stream::<minidump::MinidumpMiscInfo>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        let mut bytes = Vec::new();
        stream
            .print(
                &mut bytes,
                memory.as_ref().ok(),
                system.as_ref().ok(),
                misc.as_ref().ok(),
                brief,
            )
            .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.add(
            egui::TextEdit::multiline(&mut &*text)
                .font(TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
    }

    fn update_raw_dump_assertion_info(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_stream::<minidump::MinidumpAssertion>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        ui.horizontal_wrapped(|ui| {
            let mut bytes = Vec::new();
            stream.print(&mut bytes).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            ui.add(
                egui::TextEdit::multiline(&mut &*text)
                    .font(TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
    }

    fn update_raw_dump_crashpad_info(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_stream::<minidump::MinidumpCrashpadInfo>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        ui.horizontal_wrapped(|ui| {
            let mut bytes = Vec::new();
            stream.print(&mut bytes).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            ui.add(
                egui::TextEdit::multiline(&mut &*text)
                    .font(TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
    }

    fn update_raw_dump_breakpad_info(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_stream::<minidump::MinidumpBreakpadInfo>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        ui.horizontal_wrapped(|ui| {
            let mut bytes = Vec::new();
            stream.print(&mut bytes).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            ui.add(
                egui::TextEdit::multiline(&mut &*text)
                    .font(TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
    }

    fn update_raw_dump_exception(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let system_info = dump.get_stream::<minidump::MinidumpSystemInfo>();
        let misc_info = dump.get_stream::<minidump::MinidumpMiscInfo>();
        let stream = dump.get_stream::<minidump::MinidumpException>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        ui.horizontal_wrapped(|ui| {
            let mut bytes = Vec::new();
            stream
                .print(
                    &mut bytes,
                    system_info.as_ref().ok(),
                    misc_info.as_ref().ok(),
                )
                .unwrap();
            let text = String::from_utf8(bytes).unwrap();
            ui.add(
                egui::TextEdit::multiline(&mut &*text)
                    .font(TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
    }

    fn update_raw_dump_module_list(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_stream::<minidump::MinidumpModuleList>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        ui.horizontal_wrapped(|ui| {
            let mut bytes = Vec::new();
            stream.print(&mut bytes).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            ui.add(
                egui::TextEdit::multiline(&mut &*text)
                    .font(TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
    }

    fn update_raw_dump_unloaded_module_list(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_stream::<minidump::MinidumpUnloadedModuleList>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        ui.horizontal_wrapped(|ui| {
            let mut bytes = Vec::new();
            stream.print(&mut bytes).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            ui.add(
                egui::TextEdit::multiline(&mut &*text)
                    .font(TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
    }

    fn update_raw_dump_memory_list(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let brief = self.settings.raw_dump_brief;
        let stream = dump.get_stream::<minidump::MinidumpMemoryList>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        ui.horizontal_wrapped(|ui| {
            let mut bytes = Vec::new();
            stream.print(&mut bytes, brief).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            ui.add(
                egui::TextEdit::multiline(&mut &*text)
                    .font(TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
    }

    fn update_raw_dump_memory_info_list(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_stream::<minidump::MinidumpMemoryInfoList>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        ui.horizontal_wrapped(|ui| {
            let mut bytes = Vec::new();
            stream.print(&mut bytes).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            ui.add(
                egui::TextEdit::multiline(&mut &*text)
                    .font(TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
    }
    /*
       fn update_raw_dump_linux_cpu_info(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
           let stream = dump.get_stream::<minidump::MinidumpLinuxCpuInfo>();
           if let Err(e) = &stream {
               ui.label("Failed to read stream");
               ui.label(e.to_string());
               return;
           }
           let stream = stream.unwrap();
           ui.horizontal_wrapped(|ui| {
               let mut bytes = Vec::new();
               for (k, v) in stream.iter() {

               }
               stream.print(&mut bytes).unwrap();
               let text = String::from_utf8(bytes).unwrap();
               ui.monospace(text);
           });
       }

       fn update_raw_dump_linux_environ(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
           let stream = dump.get_stream::<minidump::MinidumpLinuxEnviron>();
           if let Err(e) = &stream {
               ui.label("Failed to read stream");
               ui.label(e.to_string());
               return;
           }
           let stream = stream.unwrap();
           ui.horizontal_wrapped(|ui| {
               let mut bytes = Vec::new();
               stream.print(&mut bytes).unwrap();
               let text = String::from_utf8(bytes).unwrap();
               ui.monospace(text);
           });
       }

       fn update_raw_dump_linux_lsb_release(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
           let stream = dump.get_stream::<minidump::MinidumpLinuxLsbRelease>();
           if let Err(e) = &stream {
               ui.label("Failed to read stream");
               ui.label(e.to_string());
               return;
           }
           let stream = stream.unwrap();
           ui.horizontal_wrapped(|ui| {
               let mut bytes = Vec::new();
               stream.print(&mut bytes).unwrap();
               let text = String::from_utf8(bytes).unwrap();
               ui.monospace(text);
           });
       }

       fn update_raw_dump_moz_macos_crash_info(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
           let stream = dump.get_stream::<minidump::MinidumpMacCrashInfo>();
           if let Err(e) = &stream {
               ui.label("Failed to read stream");
               ui.label(e.to_string());
               return;
           }
           let stream = stream.unwrap();
           ui.horizontal_wrapped(|ui| {
               let mut bytes = Vec::new();
               stream.print(&mut bytes).unwrap();
               let text = String::from_utf8(bytes).unwrap();
               ui.monospace(text);
           });
       }
    */

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
        StripBuilder::new(ui)
            .size(Size::relative(0.5))
            .size(Size::relative(0.5))
            .vertical(|mut strip| {
                strip.cell(|ui| {
                    self.update_processed_data(ui, state);
                });
                strip.cell(|ui| {
                    ComboBox::from_label("Thread")
                        .width(400.0)
                        .selected_text(
                            state
                                .threads
                                .get(self.processed_ui_state.cur_thread)
                                .map(threadname)
                                .unwrap_or_default(),
                        )
                        .show_ui(ui, |ui| {
                            for (idx, stack) in state.threads.iter().enumerate() {
                                if ui
                                    .selectable_value(
                                        &mut self.processed_ui_state.cur_thread,
                                        idx,
                                        threadname(stack),
                                    )
                                    .changed()
                                {
                                    self.processed_ui_state.cur_frame = 0;
                                };
                            }
                        });

                    ui.separator();

                    if let Some(stack) = state.threads.get(self.processed_ui_state.cur_thread) {
                        if is_symbolicated {
                            self.update_processed_backtrace(ui, stack);
                        } else {
                            ui.label("stackwalking in progress...");
                        }
                    }
                });
            });
    }

    fn update_processed_data(&mut self, ui: &mut Ui, state: &ProcessState) {
        let cur_threadname = state
            .threads
            .get(self.processed_ui_state.cur_thread)
            .map(threadname)
            .unwrap_or_default();

        StripBuilder::new(ui)
            .size(Size::relative(0.5))
            .size(Size::relative(0.5))
            .horizontal(|mut strip| {
                strip.cell(|ui| {
                    listing(
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
                        listing(
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
                            listing(ui, 3, regs);
                        }
                    }
                })
            });
    }

    fn update_processed_backtrace(&mut self, ui: &mut Ui, stack: &CallStack) {
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
                for (i, frame) in stack.frames.iter().enumerate() {
                    let is_thick = false; // thick_row(row_index);
                    let row_height = if is_thick { 30.0 } else { 18.0 };

                    body.row(row_height, |mut row| {
                        row.col(|ui| {
                            ui.centered_and_justified(|ui| {
                                if ui.link(i.to_string()).clicked() {
                                    self.processed_ui_state.cur_frame = i;
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
                                ui.label(trust);
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
                            frame_source(&mut label, frame).unwrap();
                            // ui.style_mut().wrap = Some(false);
                            ui.label(label);
                        });
                        row.col(|ui| {
                            let mut label = String::new();
                            frame_signature(&mut label, frame).unwrap();
                            // ui.style_mut().wrap = Some(false);
                            ui.label(label);
                        });
                    });
                }
            });
    }

    fn update_logs(&self, ui: &mut Ui, _ctx: &egui::Context) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let bytes = self.logger.bytes();
            let text = String::from_utf8(bytes).expect("logs weren't utf8");
            ui.add(
                egui::TextEdit::multiline(&mut &*text)
                    .font(TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
    }
}

fn listing(ui: &mut Ui, id: u64, items: impl IntoIterator<Item = (String, String)>) {
    ui.push_id(id, |ui| {
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right().with_cross_align(egui::Align::Center))
            .column(Size::initial(120.0).at_least(40.0))
            .column(Size::remainder().at_least(60.0))
            .resizable(true)
            .body(|mut body| {
                for (lhs, rhs) in items {
                    let row_height = 18.0;
                    body.row(row_height, |mut row| {
                        row.col(|ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut &*lhs).desired_width(f32::INFINITY),
                            );
                        });
                        row.col(|ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut &*rhs).desired_width(f32::INFINITY),
                            );
                        });
                    });
                }
            });
    });
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
    _task_receiver: &Arc<(Mutex<Option<ProcessorTask>>, Condvar)>,
    settings: &ProcessDump,
    symbolicate: bool,
) -> Result<ProcessState, minidump_processor::ProcessError> {
    let (symbol_paths, symbol_urls) = if symbolicate {
        (settings.symbol_paths.clone(), settings.symbol_urls.clone())
    } else {
        (vec![], vec![])
    };

    // Configure the symbolizer and processor
    let symbols_cache = settings.symbol_cache.clone();
    if settings.clear_cache {
        let _ = std::fs::remove_dir_all(&symbols_cache);
    }
    let _ = std::fs::create_dir_all(&symbols_cache);
    let symbols_tmp = std::env::temp_dir();
    let timeout = std::time::Duration::from_secs(settings.http_timeout_secs);

    // Use ProcessorOptions for detailed configuration
    let options = ProcessorOptions::default();

    // Specify a symbol supplier (here we're using the most powerful one, the http supplier)
    let provider = Symbolizer::new(http_symbol_supplier(
        symbol_paths,
        symbol_urls,
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
            minidump_processor::process_minidump_with_options(&settings.dump, &provider, options)
                .await;
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
