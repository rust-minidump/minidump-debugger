use linked_hash_map::LinkedHashMap;
use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
};
use tracing::{Id, Level};
use tracing_subscriber::Layer;

use std::sync::{Arc, Mutex};

const TRACE_THREAD_SPAN: &str = "unwind_thread";
const TRACE_FRAME_SPAN: &str = "unwind_frame";
const TRACE_FRAME_SPAN_IDX: &str = "idx";

/// An in-memory logger that lets us view particular
/// spans of the logs, and understands minidump-stackwalk's
/// span format for threads/frames during stackwalking.
#[derive(Default, Debug, Clone)]
pub struct MapLogger {
    state: Arc<Mutex<MapLoggerInner>>,
}

type SpanId = u64;

#[derive(Default, Debug, Clone)]
struct MapLoggerInner {
    root_span: SpanEntry,
    sub_spans: LinkedHashMap<SpanId, SpanEntry>,

    last_query: Option<Query>,
    cur_string: Option<Arc<String>>,

    thread_spans: HashMap<usize, SpanId>,
    frame_spans: HashMap<(usize, usize), SpanId>,
    live_spans: HashMap<Id, SpanId>,
    next_span_id: SpanId,
}

#[derive(Default, Debug, Clone)]
struct SpanEntry {
    destroyed: bool,
    name: String,
    fields: BTreeMap<String, String>,
    events: Vec<EventEntry>,
    idx: Option<usize>,
}

#[derive(Debug, Clone)]
enum EventEntry {
    Span(SpanId),
    Message(MessageEntry),
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct MessageEntry {
    level: Level,
    fields: BTreeMap<String, String>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Query {
    All,
    Thread(SpanId),
    Frame(SpanId, SpanId),
}

impl MapLogger {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn clear(&self) {
        let mut log = self.state.lock().unwrap();
        let ids = log.sub_spans.keys().cloned().collect::<Vec<_>>();
        for id in ids {
            let span = log.sub_spans.get_mut(&id).unwrap();
            if !span.destroyed {
                span.events.clear();
                continue;
            }
            log.sub_spans.remove(&id);
        }
        log.root_span.events.clear();
        log.cur_string = None;
    }

    pub fn string_for_all(&self) -> Arc<String> {
        self.string_query(Query::All)
    }

    pub fn string_for_thread(&self, thread_idx: usize) -> Arc<String> {
        let thread = self
            .state
            .lock()
            .unwrap()
            .thread_spans
            .get(&thread_idx)
            .cloned();

        if let Some(thread) = thread {
            self.string_query(Query::Thread(thread))
        } else {
            self.string_query(Query::All)
        }
    }

    pub fn string_for_frame(&self, thread_idx: usize, frame_idx: usize) -> Arc<String> {
        let thread = self
            .state
            .lock()
            .unwrap()
            .thread_spans
            .get(&thread_idx)
            .cloned();

        let frame = self
            .state
            .lock()
            .unwrap()
            .frame_spans
            .get(&(thread_idx, frame_idx))
            .cloned();

        if let (Some(thread), Some(frame)) = (thread, frame) {
            self.string_query(Query::Frame(thread, frame))
        } else {
            self.string_query(Query::All)
        }
    }

    fn string_query(&self, query: Query) -> Arc<String> {
        use std::fmt::Write;

        fn print_indent(output: &mut String, depth: usize) {
            write!(output, "{:indent$}", "", indent = depth * 4).unwrap();
        }
        fn print_span_recursive(
            output: &mut String,
            sub_spans: &LinkedHashMap<SpanId, SpanEntry>,
            depth: usize,
            span: &SpanEntry,
            range: Option<Range<usize>>,
        ) {
            if !span.name.is_empty() {
                print_indent(output, depth);
                writeln!(output, "[{} {:?}]", span.name, span.fields).unwrap();
            }

            let event_range = if let Some(range) = range {
                &span.events[range]
            } else {
                &span.events[..]
            };
            for event in event_range {
                match event {
                    EventEntry::Message(event) => {
                        if let Some(message) = event.fields.get("message") {
                            print_indent(output, depth + 1);
                            // writeln!(output, "[{:5}] {}", event.level, message).unwrap();
                            writeln!(output, "{}", message).unwrap();
                        }
                    }
                    EventEntry::Span(sub_span) => {
                        print_span_recursive(
                            output,
                            sub_spans,
                            depth + 1,
                            &sub_spans[sub_span],
                            None,
                        );
                    }
                }
            }
        }

        let mut log = self.state.lock().unwrap();
        if Some(query) == log.last_query {
            if let Some(string) = &log.cur_string {
                return string.clone();
            }
        }
        log.last_query = Some(query.clone());

        let mut output = String::new();

        let (span_to_print, range) = match query {
            Query::All => (&log.root_span, None),
            Query::Thread(thread) => (&log.sub_spans[&thread], None),
            Query::Frame(thread, frame) => {
                // So if you care about frame X, you might care about how it's produced
                // and how it was walked, so we want to grab both. We accomplish this by
                // scrubbing through all the events and keeping a sliding window of the
                // last few spans seen.
                //
                // Once we reach the target span, we keep seeking until the next span.
                // We want to print out info about prev_frame and this_frame, but there
                // might be some extra little tidbits before and after those points,
                // so print out `grand_prev_frame+1 .. next_frame`.
                let thread_span = &log.sub_spans[&thread];
                let mut grand_prev_frame = None;
                let mut prev_frame = None;
                let mut this_frame = None;
                let mut next_frame = None;

                for (idx, event) in thread_span.events.iter().enumerate() {
                    if let EventEntry::Span(span_event) = event {
                        if span_event == &frame {
                            this_frame = Some(idx);
                        } else if this_frame.is_none() {
                            grand_prev_frame = prev_frame;
                            prev_frame = Some(idx);
                        } else {
                            next_frame = Some(idx);
                            break;
                        }
                    }
                }

                // Now get the ranges, snapping to start/end if missing the boundary points
                assert!(this_frame.is_some(), "couldn't find frame in logs!?");
                let range_start = if let Some(grand_prev_frame) = grand_prev_frame {
                    grand_prev_frame + 1
                } else {
                    0
                };
                let range_end = if let Some(next_frame) = next_frame {
                    next_frame
                } else {
                    thread_span.events.len()
                };

                // Add a message indicating how to read this special snapshot
                writeln!(
                    &mut output,
                    "Viewing logs for a frame's stackwalk, which has two parts"
                )
                .unwrap();
                writeln!(
                    &mut output,
                    "  1. How the frame was computed (the stackwalk of its callee)"
                )
                .unwrap();
                writeln!(
                    &mut output,
                    "  2. How the frame itself was walked (producing its caller)"
                )
                .unwrap();
                writeln!(&mut output).unwrap();

                (thread_span, Some(range_start..range_end))
            }
        };

        print_span_recursive(&mut output, &log.sub_spans, 0, &span_to_print, range);

        let result = Arc::new(output);
        log.cur_string = Some(result.clone());
        result
    }
}

impl<S> Layer<S> for MapLogger
where
    S: tracing::Subscriber,
    S: for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut log = self.state.lock().unwrap();
        // Invalidate any cached log printout
        log.cur_string = None;

        // Grab the parent span (or the dummy root span)
        let cur_span = if let Some(span) = ctx.event_span(event) {
            let span_id = log.live_spans[&span.id()];
            log.sub_spans.get_mut(&span_id).unwrap()
        } else {
            &mut log.root_span
        };

        // Grab the fields
        let mut fields = BTreeMap::new();
        let mut visitor = MapVisitor(&mut fields);
        event.record(&mut visitor);

        // Store the message in the span
        cur_span.events.push(EventEntry::Message(MessageEntry {
            level: event.metadata().level().clone(),
            fields: fields,
        }));
    }

    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut log = self.state.lock().unwrap();
        // Invalidate any cache log printout
        log.cur_string = None;

        // Create a new persistent id for this span, `tracing` may recycle its ids
        let new_span_id = log.next_span_id;
        log.next_span_id += 1;
        log.live_spans.insert(id.clone(), new_span_id);

        // Get the parent span (or dummy root span)
        let span = ctx.span(id).unwrap();
        let parent_span = if let Some(parent) = span.parent() {
            let parent_span_id = log.live_spans[&parent.id()];
            log.sub_spans.get_mut(&parent_span_id).unwrap()
        } else {
            &mut log.root_span
        };

        // Store the span at this point in the parent spans' messages,
        // so when we print out the parent span, this whole span will
        // print out "atomically" at this precise point in the log stream
        // which basically reconstitutes the logs of a sequential execution!
        parent_span.events.push(EventEntry::Span(new_span_id));

        // The actual span, with some info TBD
        let mut new_entry = SpanEntry {
            destroyed: false,
            name: span.name().to_owned(),
            fields: BTreeMap::new(),
            events: Vec::new(),
            idx: None,
        };

        // Collect up fields for the span, and detect if it's a thread/frame span
        let mut visitor = SpanVisitor(&mut new_entry);
        attrs.record(&mut visitor);

        if let Some(idx) = new_entry.idx {
            if span.name() == TRACE_THREAD_SPAN {
                log.thread_spans.insert(idx, new_span_id);
            } else if span.name() == TRACE_FRAME_SPAN {
                if let Some(thread_idx) = parent_span.idx {
                    log.frame_spans.insert((thread_idx, idx), new_span_id);
                }
            }
        }

        log.sub_spans.insert(new_span_id, new_entry);
    }

    fn on_close(&self, id: Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        // Mark the span as GC-able and remove it from the live mappings,
        // as tracing may now recycle the id for future spans!
        let mut log = self.state.lock().unwrap();
        let span_id = log.live_spans[&id];
        log.sub_spans.get_mut(&span_id).unwrap().destroyed = true;
        log.live_spans.remove(&id);
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut log = self.state.lock().unwrap();

        // Update fields... idk we don't really need/use this but sure whatever
        let mut new_fields = BTreeMap::new();
        let mut visitor = MapVisitor(&mut new_fields);
        values.record(&mut visitor);

        let span_id = log.live_spans[&id];
        log.sub_spans
            .get_mut(&span_id)
            .unwrap()
            .fields
            .append(&mut new_fields);
    }
}

/// Same as MapVisitor but grabs the special `idx: u64` field
struct SpanVisitor<'a>(&'a mut SpanEntry);

impl<'a> tracing::field::Visit for SpanVisitor<'a> {
    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.0.fields.insert(field.to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.fields.insert(field.to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == TRACE_FRAME_SPAN_IDX {
            self.0.idx = Some(value as usize);
        }
        self.0.fields.insert(field.to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0.fields.insert(field.to_string(), value.to_string());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.fields.insert(field.to_string(), value.to_string());
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        self.0.fields.insert(field.to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0
            .fields
            .insert(field.to_string(), format!("{:?}", value));
    }
}

/// Super boring generic field slurping
struct MapVisitor<'a>(&'a mut BTreeMap<String, String>);

impl<'a> tracing::field::Visit for MapVisitor<'a> {
    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.0.insert(field.to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.insert(field.to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.insert(field.to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0.insert(field.to_string(), value.to_string());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.to_string(), value.to_string());
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        self.0.insert(field.to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.to_string(), format!("{:?}", value));
    }
}
