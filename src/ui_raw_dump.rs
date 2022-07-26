use crate::MyApp;
use eframe::egui;
use egui::{TextStyle, Ui};
use egui_extras::{Size, StripBuilder, TableBuilder};
use memmap2::Mmap;
use minidump::{format::MINIDUMP_STREAM_TYPE, Minidump};
use num_traits::FromPrimitive;

pub struct RawDumpUiState {
    pub cur_stream: usize,
}

impl MyApp {
    pub fn ui_raw_dump(&mut self, ui: &mut Ui, _ctx: &egui::Context) {
        if let Some(minidump) = &self.minidump {
            match minidump {
                Ok(dump) => {
                    self.ui_raw_dump_good(ui, &dump.clone());
                }
                Err(e) => {
                    ui.label("Minidump couldn't be read!");
                    ui.label(e.to_string());
                }
            }
        }
    }

    fn ui_raw_dump_good(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        StripBuilder::new(ui)
            .size(Size::exact(180.0))
            .size(Size::remainder())
            .horizontal(|mut strip| {
                strip.cell(|ui| {
                    self.ui_raw_dump_streams(ui, dump);
                });
                strip.cell(|ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        if self.raw_dump_ui_state.cur_stream == 0 {
                            self.ui_raw_dump_top_level(ui, dump);
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
                                Memory64ListStream => self.update_raw_dump_memory_64_list(ui, dump),
                                MemoryInfoListStream => {
                                    self.update_raw_dump_memory_info_list(ui, dump)
                                }
                                LinuxMaps => self.update_raw_dump_linux_maps(ui, dump),
                                LinuxCmdLine => self.update_raw_dump_linux_cmd_line(ui, dump),
                                LinuxCpuInfo => self.update_raw_dump_linux_cpu_info(ui, dump),
                                LinuxEnviron => self.update_raw_dump_linux_environ(ui, dump),
                                LinuxLsbRelease => self.update_raw_dump_linux_lsb_release(ui, dump),
                                LinuxProcStatus => self.update_raw_dump_linux_proc_status(ui, dump),
                                MozMacosCrashInfoStream => {
                                    self.update_raw_dump_moz_macos_crash_info(ui, dump)
                                }
                                _ => {}
                            }
                        }
                    });
                });
            });
    }

    fn ui_raw_dump_streams(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        ui.heading("Streams");
        ui.add_space(20.0);
        ui.selectable_value(&mut self.raw_dump_ui_state.cur_stream, 0, "<summary>");

        for (i, stream) in dump.all_streams().enumerate() {
            use MINIDUMP_STREAM_TYPE::*;
            let (supported, label) =
                if let Some(stream_type) = MINIDUMP_STREAM_TYPE::from_u32(stream.stream_type) {
                    let supported = matches!(
                        stream_type,
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
                            | Memory64ListStream
                            | MemoryInfoListStream
                            | MozMacosCrashInfoStream
                            | LinuxCmdLine
                            | LinuxMaps
                            | LinuxCpuInfo
                            | LinuxEnviron
                            | LinuxLsbRelease
                            | LinuxProcStatus
                    );

                    (supported, format!("{:?}", stream_type))
                } else {
                    (false, "<unknown>".to_string())
                };

            ui.add_enabled_ui(supported, |ui| {
                ui.selectable_value(&mut self.raw_dump_ui_state.cur_stream, i + 1, label);
            });
        }
    }

    fn ui_raw_dump_top_level(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
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
                                ui.label(crate::stream_vendor(stream.stream_type));
                            });
                        });
                        row.col(|ui| {
                            use MINIDUMP_STREAM_TYPE::*;
                            let (supported, label) = if let Some(stream_type) =
                                MINIDUMP_STREAM_TYPE::from_u32(stream.stream_type)
                            {
                                let supported = matches!(
                                    stream_type,
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
                                        | Memory64ListStream
                                        | MemoryInfoListStream
                                        | MozMacosCrashInfoStream
                                        | LinuxCmdLine
                                        | LinuxMaps
                                        | LinuxCpuInfo
                                        | LinuxEnviron
                                        | LinuxLsbRelease
                                        | LinuxProcStatus
                                );
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

    fn update_raw_dump_moz_macos_crash_info(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_stream::<minidump::MinidumpMacCrashInfo>();
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

        let mut bytes = Vec::new();
        stream.print(&mut bytes).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.add(
            egui::TextEdit::multiline(&mut &*text)
                .font(TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
    }

    fn update_raw_dump_unloaded_module_list(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_stream::<minidump::MinidumpUnloadedModuleList>();
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

    fn update_raw_dump_memory_list(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let brief = self.settings.raw_dump_brief;
        let stream = dump.get_stream::<minidump::MinidumpMemoryList>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();

        let mut bytes = Vec::new();
        stream.print(&mut bytes, brief).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.add(
            egui::TextEdit::multiline(&mut &*text)
                .font(TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
    }
    fn update_raw_dump_memory_64_list(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let brief = self.settings.raw_dump_brief;
        let stream = dump.get_stream::<minidump::MinidumpMemory64List>();
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();

        let mut bytes = Vec::new();
        stream.print(&mut bytes, brief).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.add(
            egui::TextEdit::multiline(&mut &*text)
                .font(TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
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

    fn update_raw_dump_linux_cpu_info(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_raw_stream(MINIDUMP_STREAM_TYPE::LinuxCpuInfo as u32);
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        let mut bytes = Vec::new();
        print_raw_stream("LinuxCpuInfo", stream, &mut bytes).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.monospace(text);
    }

    fn update_raw_dump_linux_proc_status(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_raw_stream(MINIDUMP_STREAM_TYPE::LinuxProcStatus as u32);
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        let mut bytes = Vec::new();
        print_raw_stream("LinuxProcStatus", stream, &mut bytes).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.monospace(text);
    }

    fn update_raw_dump_linux_maps(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_raw_stream(MINIDUMP_STREAM_TYPE::LinuxMaps as u32);
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        let mut bytes = Vec::new();
        print_raw_stream("LinuxMaps", stream, &mut bytes).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.monospace(text);
    }

    fn update_raw_dump_linux_cmd_line(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_raw_stream(MINIDUMP_STREAM_TYPE::LinuxCmdLine as u32);
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        let mut bytes = Vec::new();
        print_raw_stream("LinuxCmdLine", stream, &mut bytes).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.monospace(text);
    }

    fn update_raw_dump_linux_lsb_release(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_raw_stream(MINIDUMP_STREAM_TYPE::LinuxLsbRelease as u32);
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        let mut bytes = Vec::new();
        print_raw_stream("LinuxLsbRelease", stream, &mut bytes).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.monospace(text);
    }

    fn update_raw_dump_linux_environ(&mut self, ui: &mut Ui, dump: &Minidump<Mmap>) {
        let stream = dump.get_raw_stream(MINIDUMP_STREAM_TYPE::LinuxEnviron as u32);
        if let Err(e) = &stream {
            ui.label("Failed to read stream");
            ui.label(e.to_string());
            return;
        }
        let stream = stream.unwrap();
        let mut bytes = Vec::new();
        print_raw_stream("LinuxEnviron", stream, &mut bytes).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        ui.monospace(text);
    }
}

fn print_raw_stream<T: std::io::Write>(
    name: &str,
    contents: &[u8],
    out: &mut T,
) -> std::io::Result<()> {
    writeln!(out, "Stream {}:", name)?;
    let s = contents
        .split(|&v| v == 0)
        .map(String::from_utf8_lossy)
        .collect::<Vec<_>>()
        .join("\\0\n");
    write!(out, "{}\n\n", s)
}
