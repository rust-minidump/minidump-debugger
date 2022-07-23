use linked_hash_map::LinkedHashMap;
use std::collections::{BTreeMap, HashMap};
use tracing::{Id, Level};
use tracing_subscriber::Layer;

use std::sync::{Arc, Mutex};

const TRACE_THREAD_SPAN: &str = "unwind_thread";
const TRACE_FRAME_SPAN: &str = "unwind_frame";
const TRACE_FRAME_SPAN_IDX: &str = "idx";

#[derive(Default, Debug, Clone)]
pub struct MapLogger {
    state: Arc<Mutex<MapLoggerInner>>,
}

#[derive(Default, Debug, Clone)]
struct MapLoggerInner {
    root_span: SpanEntry,
    sub_spans: LinkedHashMap<Id, SpanEntry>,

    last_query: Option<Id>,
    cur_string: Option<Arc<String>>,

    thread_spans: HashMap<usize, Id>,
    frame_spans: HashMap<(usize, usize), Id>,
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
    Span(Id),
    Message(MessageEntry),
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct MessageEntry {
    level: Level,
    fields: BTreeMap<String, String>,
}

impl MapLogger {
    pub fn new() -> Self {
        let this = Self::default();
        // this.state.lock().unwrap().modified = true;
        this
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

    pub fn id_for_thread(&self, thread_idx: usize) -> Option<Id> {
        self.state
            .lock()
            .unwrap()
            .thread_spans
            .get(&thread_idx)
            .cloned()
    }

    pub fn id_for_frame(&self, thread_idx: usize, frame_idx: usize) -> Option<Id> {
        self.state
            .lock()
            .unwrap()
            .frame_spans
            .get(&(thread_idx, frame_idx))
            .cloned()
    }

    pub fn string(&self, query_id: Option<Id>) -> Arc<String> {
        use std::fmt::Write;

        fn print_indent(output: &mut String, depth: usize) {
            write!(output, "{:indent$}", "", indent = depth * 4).unwrap();
        }
        fn print_span_recursive(
            output: &mut String,
            sub_spans: &LinkedHashMap<Id, SpanEntry>,
            depth: usize,
            span: &SpanEntry,
        ) {
            print_indent(output, depth);
            writeln!(output, "[{} {:?}]", span.name, span.fields).unwrap();

            for event in &span.events {
                match event {
                    EventEntry::Message(event) => {
                        if let Some(message) = event.fields.get("message") {
                            print_indent(output, depth + 1);
                            // writeln!(output, "[{:5}] {}", event.level, message).unwrap();
                            writeln!(output, "{}", message).unwrap();
                        }
                    }
                    EventEntry::Span(sub_span) => {
                        print_span_recursive(output, sub_spans, depth + 1, &sub_spans[sub_span]);
                    }
                }
            }
        }

        let mut state = self.state.lock().unwrap();
        if query_id == state.last_query {
            if let Some(string) = &state.cur_string {
                return string.clone();
            }
        }
        state.last_query = query_id.clone();

        let mut output = String::new();
        let span_to_print = query_id
            .and_then(|id| state.sub_spans.get(&id))
            .unwrap_or(&state.root_span);
        print_span_recursive(&mut output, &state.sub_spans, 0, &span_to_print);

        let result = Arc::new(output);
        state.cur_string = Some(result.clone());
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
        log.cur_string = None;

        let cur_span = if let Some(span) = ctx.event_span(event) {
            log.sub_spans.get_mut(&span.id()).unwrap()
        } else {
            &mut log.root_span
        };

        // The fields of the event
        let mut fields = BTreeMap::new();
        let mut visitor = MapVisitor(&mut fields);
        event.record(&mut visitor);

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
        log.cur_string = None;

        let span = ctx.span(id).unwrap();
        let parent_span = if let Some(parent) = span.parent() {
            log.sub_spans.get_mut(&parent.id()).unwrap()
        } else {
            &mut log.root_span
        };
        parent_span.events.push(EventEntry::Span(id.clone()));

        let mut new_entry = SpanEntry {
            destroyed: false,
            name: span.name().to_owned(),
            fields: BTreeMap::new(),
            events: Vec::new(),
            idx: None,
        };

        // Build our json object from the field values like we have been
        let mut visitor = SpanVisitor(&mut new_entry);
        attrs.record(&mut visitor);

        if let Some(idx) = new_entry.idx {
            if span.name() == TRACE_THREAD_SPAN {
                log.thread_spans.insert(idx, id.clone());
            } else if span.name() == TRACE_FRAME_SPAN {
                if let Some(thread_idx) = parent_span.idx {
                    log.frame_spans.insert((thread_idx, idx), id.clone());
                }
            }
        }

        log.sub_spans.insert(id.clone(), new_entry);
    }

    fn on_close(&self, id: Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut log = self.state.lock().unwrap();
        log.sub_spans.get_mut(&id).unwrap().destroyed = true;
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut log = self.state.lock().unwrap();

        // And add to using our old friend the visitor!
        let mut new_fields = BTreeMap::new();
        let mut visitor = MapVisitor(&mut new_fields);
        values.record(&mut visitor);

        log.sub_spans
            .get_mut(&id)
            .unwrap()
            .fields
            .append(&mut new_fields);
    }
}

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
