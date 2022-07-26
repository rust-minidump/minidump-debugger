use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};

use memmap2::Mmap;
use minidump::Minidump;
use minidump_processor::{
    http_symbol_supplier, PendingProcessorStatSubscriptions, PendingProcessorStats,
    PendingSymbolStats, ProcessState, ProcessorOptions, Symbolizer,
};

#[derive(Default, Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProcessingStatus {
    #[default]
    NoDump,
    ReadingDump,
    RawProcessing,
    Symbolicating,
    Done,
}

pub enum ProcessorTask {
    Cancel,
    ReadDump(PathBuf),
    ProcessDump(ProcessDump),
}

pub type MaybeMinidump = Option<Result<Arc<Minidump<'static, Mmap>>, minidump::Error>>;
pub type MaybeProcessed = Option<Result<Arc<ProcessState>, minidump_processor::ProcessError>>;

#[derive(Default, Clone)]
pub struct MinidumpAnalysis {
    pub minidump: Arc<Mutex<MaybeMinidump>>,
    pub processed: Arc<Mutex<MaybeProcessed>>,
    pub stats: Arc<Mutex<ProcessingStats>>,
}

#[derive(Clone)]
pub struct ProcessingStats {
    pub processor_stats: Arc<PendingProcessorStats>,
    pub pending_symbols: Arc<Mutex<PendingSymbolStats>>,
}

impl Default for ProcessingStats {
    fn default() -> Self {
        let mut subscriptions = PendingProcessorStatSubscriptions::default();
        subscriptions.thread_count = true;
        subscriptions.frame_count = true;
        subscriptions.unwalked_result = true;
        subscriptions.live_frames = true;

        Self {
            processor_stats: Arc::new(PendingProcessorStats::new(subscriptions)),
            pending_symbols: Default::default(),
        }
    }
}

pub struct ProcessDump {
    pub dump: Arc<Minidump<'static, Mmap>>,
    pub symbol_paths: Vec<PathBuf>,
    pub symbol_urls: Vec<String>,
    pub symbol_cache: PathBuf,
    pub clear_cache: bool,
    pub http_timeout_secs: u64,
}

pub fn run_processor(
    task_receiver: std::sync::Arc<(std::sync::Mutex<Option<ProcessorTask>>, std::sync::Condvar)>,
    analysis_sender: std::sync::Arc<MinidumpAnalysis>,
    logger: crate::logger::MapLogger,
) {
    loop {
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
                // Read the dump
                let dump = Minidump::read_path(path).map(Arc::new);
                *analysis_sender.minidump.lock().unwrap() = Some(dump);
            }
            ProcessorTask::ProcessDump(settings) => {
                // Reset all stats
                *analysis_sender.stats.lock().unwrap() = Default::default();
                logger.clear();

                // Do the processing
                let processed = process_minidump(&task_receiver, &analysis_sender, &settings, true);
                *analysis_sender.processed.lock().unwrap() = processed.map(|p| p.map(Arc::new));
            }
        }
    }
}

fn process_minidump(
    task_receiver: &Arc<(Mutex<Option<ProcessorTask>>, Condvar)>,
    analysis_sender: &Arc<MinidumpAnalysis>,
    settings: &ProcessDump,
    symbolicate: bool,
) -> Option<Result<ProcessState, minidump_processor::ProcessError>> {
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
    let mut options = ProcessorOptions::default();
    let stat_reporter = analysis_sender
        .stats
        .lock()
        .unwrap()
        .processor_stats
        .clone();
    options.stat_reporter = Some(&stat_reporter);

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

    let process = || async {
        minidump_processor::process_minidump_with_options(&settings.dump, &provider, options).await
    };
    let check_status = || async {
        loop {
            if task_receiver.0.lock().unwrap().is_some() {
                // Cancel processing, controller wants us doing something else
                return;
            }
            // Update stats
            *analysis_sender
                .stats
                .lock()
                .unwrap()
                .pending_symbols
                .lock()
                .unwrap() = provider.pending_stats();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    };

    let state = runtime.block_on(async {
        tokio::select! {
            state = process() => Some(state),
            _ = check_status() => None,
        }
    });

    *analysis_sender
        .stats
        .lock()
        .unwrap()
        .pending_symbols
        .lock()
        .unwrap() = provider.pending_stats();

    state
}
