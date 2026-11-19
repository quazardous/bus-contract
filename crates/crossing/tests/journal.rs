//! The `Journal` and `Relay` capabilities over a reference in-memory
//! implementation written against the contract alone: shared listeners
//! in registration order, no deadlines, no local listeners; a journal
//! with a window and a pump thread that delivers its entries in order;
//! asks answered for another process, behind authorities.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use std::panic::{AssertUnwindSafe, catch_unwind};

use std::any::{Any, TypeId};

use bus_contract::{
    Altered, AnyInput, AnyListener, AnyLocalListener, AnyValue, Bus, Cast, CloneFn, Collected,
    Facet, Failure, FailureKind, FailureSink, FreshFn, HookError, ListenOpts, ListenerInfo,
    PendingRun, PendingWaker, Point, PointDecl, PointId, PointInfo, PointKey, PointKind, PointSpec,
    Provided, Refusal, Refused, declare_all, offers,
};
use bus_contract_crossing::far_listener;
use bus_contract_crossing::{
    Answer, Authority, BusCrossing, BusJournal, Codec, Crossing, Cursor, IngestError, JOURNALED,
    Journal, JournalEntry, Lost, Origin, Relay, Relayed, RelayedAnswer, RelayedAsk, RelayedEntry,
    RelayedRequest, Resumed, Since, codec, is_journaled, relayable, relayable_event,
    relayable_request,
};
use bus_contract_journal::kind_matches;

/// Each bus a run of its own, as a process restart would give.
static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

/// A delivery the pump makes: one entry, to its point when it has one.
struct Queued {
    seq: u64,
    deliver: Option<(PointKey, AnyInput, FreshFn)>,
}

#[derive(Default)]
struct Log {
    /// The window, oldest first.
    entries: VecDeque<JournalEntry>,
    last: u64,
    delivered: u64,
    queue: VecDeque<Queued>,
}

/// An ask answered: who asked (process, run, number).
type AskKey = (String, u64, u64);

struct Inner {
    points: Mutex<HashMap<&'static str, (PointKey, PointSpec)>>,
    listeners: Mutex<Vec<(&'static str, String, ListenOpts, AnyListener)>>,
    sinks: Mutex<Vec<FailureSink>>,
    authorities: Mutex<Vec<Authority>>,
    answered: Mutex<HashMap<AskKey, RelayedAnswer>>,
    log: Mutex<Log>,
    changed: Condvar,
    run: u64,
    window: usize,
}

impl Inner {
    fn of_point(&self, point: &str) -> Vec<(String, Option<String>, AnyListener)> {
        self.listeners
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.0 == point)
            .map(|l| (l.1.clone(), l.2.owner.clone(), l.3.clone()))
            .collect()
    }

    fn fail(&self, f: &Failure) {
        for sink in self.sinks.lock().unwrap().iter() {
            sink(f);
        }
    }

    /// An alter emit with refusals and fail-closed requests.
    fn alter(
        &self,
        point: PointKey,
        input: AnyInput,
        value: AnyValue,
        clone: CloneFn,
    ) -> Altered<AnyValue> {
        let request = point.kind == PointKind::Request;
        let fail_closed = request
            && self
                .spec(point.id)
                .is_some_and(|(_, spec)| spec.fail_closed);
        let mut out = Altered::new(value);
        for (id, owner, run) in self.of_point(point.id) {
            let before = clone(&*out.value);
            match run(&*input, &mut *out.value) {
                Ok(()) => {}
                Err(HookError::Refuse(reason)) if request => {
                    out.value = before;
                    out.refused = Some(Refusal::new(point.id, id, owner, reason));
                    break;
                }
                Err(e) => {
                    out.value = before;
                    let f = Failure::new(point.id, id, owner, FailureKind::Error(e.to_string()));
                    self.fail(&f);
                    if fail_closed {
                        out.refused = Some(Refusal::from_failure(&f));
                        out.failures.push(f);
                        break;
                    }
                    out.failures.push(f);
                }
            }
        }
        out
    }

    /// Every authority, in order; a panic refuses.
    fn authorize(&self, crossing: &Crossing<'_>) -> Result<(), String> {
        let all = self.authorities.lock().unwrap().clone();
        for authority in all {
            match catch_unwind(AssertUnwindSafe(|| authority(crossing))) {
                Ok(Ok(())) => {}
                Ok(Err(reason)) => return Err(reason),
                Err(_) => return Err("an authority panicked".into()),
            }
        }
        Ok(())
    }

    fn collect(&self, point: PointKey, input: AnyInput, fresh: FreshFn) -> Collected<AnyValue> {
        let listeners: Vec<(String, Option<String>, AnyListener)> = self
            .listeners
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.0 == point.id)
            .map(|l| (l.1.clone(), l.2.owner.clone(), l.3.clone()))
            .collect();
        let mut out = Collected::new();
        for (id, owner, run) in listeners {
            let mut v = fresh();
            match run(&*input, &mut *v) {
                Ok(()) => out.parts.push((id, v)),
                Err(e) => {
                    let f = Failure::new(point.id, id, owner, FailureKind::Error(e.to_string()));
                    for sink in self.sinks.lock().unwrap().iter() {
                        sink(&f);
                    }
                    out.failures.push(f);
                }
            }
        }
        out
    }

    /// Records an entry and queues its delivery; its number.
    fn record(
        &self,
        mut entry: JournalEntry,
        deliver: Option<(PointKey, AnyInput, FreshFn)>,
    ) -> u64 {
        let mut log = self.log.lock().unwrap();
        log.last += 1;
        entry.seq = log.last;
        let seq = entry.seq;
        log.entries.push_back(entry);
        while log.entries.len() > self.window {
            log.entries.pop_front();
        }
        log.queue.push_back(Queued { seq, deliver });
        self.changed.notify_all();
        seq
    }

    /// The pump: delivers each entry in order, off the publisher's
    /// thread, until the bus is gone.
    fn pump(inner: Weak<Inner>) {
        loop {
            let Some(me) = inner.upgrade() else { return };
            let next = {
                let log = me.log.lock().unwrap();
                let (mut log, _) = me
                    .changed
                    .wait_timeout_while(log, Duration::from_millis(50), |l| l.queue.is_empty())
                    .unwrap();
                log.queue.pop_front()
            };
            let Some(q) = next else { continue };
            if let Some((point, input, fresh)) = q.deliver {
                me.collect(point, input, fresh);
            }
            me.log.lock().unwrap().delivered = q.seq;
            me.changed.notify_all();
        }
    }

    fn spec(&self, kind: &str) -> Option<(PointKey, PointSpec)> {
        self.points.lock().unwrap().get(kind).cloned()
    }
}

/// The reference bus; `journal` / `relay` say which capabilities it
/// offers.
struct Memory {
    inner: Arc<Inner>,
    journal: bool,
    relay: bool,
}

impl Memory {
    fn new(journal: bool, relay: bool, window: usize) -> Memory {
        let inner = Arc::new(Inner {
            points: Mutex::default(),
            listeners: Mutex::default(),
            sinks: Mutex::default(),
            authorities: Mutex::default(),
            answered: Mutex::default(),
            log: Mutex::default(),
            changed: Condvar::new(),
            run: NEXT_RUN.fetch_add(1, Ordering::Relaxed),
            window,
        });
        if journal {
            let weak = Arc::downgrade(&inner);
            thread::spawn(move || Inner::pump(weak));
        }
        Memory {
            inner,
            journal,
            relay,
        }
    }

    fn full() -> Arc<dyn Bus> {
        Arc::new(Memory::new(true, true, 1024))
    }
}

impl Bus for Memory {
    fn declare_point(&self, point: PointKey, spec: PointSpec) -> Result<(), Refused> {
        bus_contract_journal::check(&point, &spec)?;
        bus_contract_crossing::check(&point, &spec)?;
        let spec = bus_contract_crossing::applied(&point, spec);
        let mut points = self.inner.points.lock().unwrap();
        if points.contains_key(point.id) {
            return Err(Refused::AlreadyDeclared { point: point.id });
        }
        points.insert(point.id, (point, spec));
        Ok(())
    }

    fn listen_any(
        &self,
        point: PointKey,
        id: String,
        opts: ListenOpts,
        run: AnyListener,
    ) -> Result<(), Refused> {
        self.inner
            .listeners
            .lock()
            .unwrap()
            .push((point.id, id, opts, run));
        Ok(())
    }

    fn listen_local_any(
        &self,
        _: PointKey,
        _: String,
        _: ListenOpts,
        _: AnyLocalListener,
    ) -> Result<(), Refused> {
        unimplemented!("the reference journal bus has no local listeners")
    }

    fn run_pending(&self) -> Vec<PendingRun> {
        Vec::new()
    }

    fn add_pending_waker(&self, _: PendingWaker) {}

    fn unlisten(&self, point: &str, id: &str) -> bool {
        let mut l = self.inner.listeners.lock().unwrap();
        let before = l.len();
        l.retain(|l| !(l.0 == point && l.1 == id));
        before != l.len()
    }

    fn forget(&self, owner: &str) -> usize {
        let mut l = self.inner.listeners.lock().unwrap();
        let before = l.len();
        l.retain(|l| l.2.owner.as_deref() != Some(owner));
        before - l.len()
    }

    fn alter_any(
        &self,
        point: PointKey,
        input: AnyInput,
        value: AnyValue,
        clone: CloneFn,
    ) -> Altered<AnyValue> {
        self.inner.alter(point, input, value, clone)
    }

    fn collect_any(&self, point: PointKey, input: AnyInput, fresh: FreshFn) -> Collected<AnyValue> {
        self.inner.collect(point, input, fresh)
    }

    fn add_failure_sink(&self, sink: FailureSink) {
        self.inner.sinks.lock().unwrap().push(sink);
    }

    fn points(&self) -> Vec<PointInfo> {
        let mut all: Vec<PointInfo> = self
            .inner
            .points
            .lock()
            .unwrap()
            .values()
            .map(|(k, s)| {
                let mut info = PointInfo::new(k.id, k.kind);
                info.description = s.description.clone();
                info.declared = true;
                info.facets = s.facets.clone();
                info.version = s.version;
                info.fail_closed = s.fail_closed;
                info.listeners = self.listeners(k.id).unwrap_or_default();
                info
            })
            .collect();
        all.sort_by_key(|p| p.id);
        all
    }

    fn listeners(&self, point: &str) -> Option<Vec<ListenerInfo>> {
        Some(
            self.inner
                .listeners
                .lock()
                .unwrap()
                .iter()
                .filter(|l| l.0 == point)
                .map(|l| ListenerInfo::new(l.1.clone()))
                .collect(),
        )
    }

    fn capability(&self, contract: TypeId) -> Option<Provided<'_>> {
        if self.journal && contract == TypeId::of::<dyn Journal>() {
            return Some(Provided::new::<dyn Journal>(self, &AS_JOURNAL));
        }
        if self.journal && self.relay && contract == TypeId::of::<dyn Relay>() {
            return Some(Provided::new::<dyn Relay>(self, &AS_RELAY));
        }
        None
    }
}

fn as_journal(bus: &dyn Any) -> Option<&(dyn Journal + 'static)> {
    bus.downcast_ref::<Memory>().map(|b| b as _)
}
static AS_JOURNAL: Cast<dyn Journal> = as_journal;

fn as_relay(bus: &dyn Any) -> Option<&(dyn Relay + 'static)> {
    bus.downcast_ref::<Memory>().map(|b| b as _)
}
static AS_RELAY: Cast<dyn Relay> = as_relay;

impl Journal for Memory {
    fn last_seq(&self) -> u64 {
        self.inner.log.lock().unwrap().last
    }

    fn since(&self, seq: u64, limit: usize) -> Since {
        let log = self.inner.log.lock().unwrap();
        let oldest = log.entries.front().map_or(log.last + 1, |e| e.seq);
        let entries = log
            .entries
            .iter()
            .filter(|e| e.seq > seq)
            .take(limit)
            .cloned()
            .collect();
        let mut out = Since::new(entries, log.last);
        out.gap = seq < log.last && oldest > seq + 1;
        out
    }

    fn wait(&self, patterns: &[&str], seq: u64, timeout: Duration) -> Option<JournalEntry> {
        let until = Instant::now() + timeout;
        let mut log = self.inner.log.lock().unwrap();
        loop {
            let found = log.entries.iter().find(|e| {
                e.seq > seq
                    && (patterns.is_empty() || patterns.iter().any(|p| kind_matches(p, &e.kind)))
            });
            if let Some(e) = found {
                return Some(e.clone());
            }
            let left = until.checked_duration_since(Instant::now())?;
            log = self.inner.changed.wait_timeout(log, left).unwrap().0;
        }
    }

    fn publish_any(
        &self,
        point: PointKey,
        input: AnyInput,
        fresh: FreshFn,
        cause: Option<u64>,
    ) -> Result<u64, Refused> {
        let Some((key, spec)) = self.inner.spec(point.id) else {
            return Err(bus_contract_journal::not_journaled(point.id));
        };
        if key.type_id != point.type_id {
            return Err(Refused::TypeMismatch { point: point.id });
        }
        if !is_journaled(&spec.facets) {
            return Err(bus_contract_journal::not_journaled(point.id));
        }
        let mut entry = JournalEntry::new(0, point.id);
        entry.cause = cause;
        entry.input = Some(input.clone());
        Ok(self.inner.record(entry, Some((point, input, fresh))))
    }

    fn flush(&self, timeout: Duration) -> bool {
        let log = self.inner.log.lock().unwrap();
        let target = log.last;
        let (log, _) = self
            .inner
            .changed
            .wait_timeout_while(log, timeout, |l| l.delivered < target)
            .unwrap();
        log.delivered >= target
    }
}

impl Relay for Memory {
    fn cursor(&self) -> Cursor {
        Cursor {
            run: self.inner.run,
            seq: self.last_seq(),
        }
    }

    fn resume(&self, from: Cursor, limit: usize) -> Resumed {
        let (lost, after) = if from == Cursor::START {
            (None, 0)
        } else if from.run != self.inner.run {
            (Some(Lost::Restarted), 0)
        } else {
            (self.since(from.seq, 0).gap.then_some(Lost::Gap), from.seq)
        };
        let read = self.since(after, usize::MAX);
        let mut entries = Vec::new();
        let mut next = read.last_seq;
        for e in read.entries {
            if entries.len() == limit {
                next = e.seq - 1;
                break;
            }
            let Some((_, spec)) = self.inner.spec(&e.kind) else {
                continue;
            };
            let (Some(codec), None, Some(input)) = (
                bus_contract_crossing::codec(&spec.facets),
                &e.origin,
                &e.input,
            ) else {
                continue;
            };
            let Some(text) = (codec.encode)(&**input) else {
                continue;
            };
            let mut out = RelayedEntry::new(e.seq, e.kind.clone(), text);
            out.cause = e.cause;
            out.version = spec.version;
            entries.push(out);
        }
        let mut out = Resumed::new(
            entries,
            Cursor {
                run: self.inner.run,
                seq: next,
            },
        );
        out.lost = lost;
        out
    }

    fn ingest(&self, origin: &Origin, relayed: RelayedEntry) -> Result<u64, IngestError> {
        let crossing = Crossing::new(origin, &relayed.kind, relayed.version, false);
        if let Err(reason) = self.inner.authorize(&crossing) {
            return Err(IngestError::Refused {
                kind: relayed.kind,
                reason,
            });
        }
        let mut entry = JournalEntry::new(0, relayed.kind.clone());
        entry.cause = relayed.cause;
        entry.origin = Some(origin.clone());
        let Some((key, spec)) = self.inner.spec(&relayed.kind) else {
            entry.text = Some(relayed.text);
            return Ok(self.inner.record(entry, None));
        };
        let Some(codec) =
            bus_contract_crossing::codec(&spec.facets).filter(|_| key.kind == PointKind::Event)
        else {
            return Err(IngestError::NotRelayable { kind: relayed.kind });
        };
        if spec.version != relayed.version {
            return Err(IngestError::Version {
                kind: relayed.kind,
                sent: relayed.version,
                here: spec.version,
            });
        }
        let input = (codec.decode)(&relayed.text).map_err(|reason| IngestError::Decode {
            kind: relayed.kind.clone(),
            reason,
        })?;
        entry.input = Some(input.clone());
        entry.text = Some(relayed.text);
        Ok(self.inner.record(entry, Some((key, input, codec.fresh))))
    }

    fn answer(&self, origin: &Origin, ask: RelayedAsk) -> RelayedAnswer {
        let got = Instant::now();
        let key = (origin.process.clone(), ask.run, ask.id);
        if let Some(first) = self.inner.answered.lock().unwrap().get(&key) {
            return first.clone();
        }
        let answer = RelayedAnswer::to(&ask, self.answer_once(origin, &ask, got));
        self.inner
            .answered
            .lock()
            .unwrap()
            .insert(key, answer.clone());
        answer
    }

    fn add_authority(&self, authority: Authority) {
        self.inner.authorities.lock().unwrap().push(authority);
    }
}

impl Memory {
    fn answer_once(&self, origin: &Origin, ask: &RelayedAsk, got: Instant) -> Answer {
        let crossing = Crossing::new(origin, &ask.kind, ask.version, true);
        if let Err(reason) = self.inner.authorize(&crossing) {
            return Answer::refused(reason);
        }
        let Some((key, spec)) = self.inner.spec(&ask.kind) else {
            return Answer::refused(format!("`{}` is not declared here", ask.kind));
        };
        let Some(codec) =
            codec(&spec.facets).filter(|c| key.kind == PointKind::Request && c.answers())
        else {
            return Answer::refused(format!("`{}` is not a relayable request here", ask.kind));
        };
        if spec.version != ask.version {
            return Answer::refused(format!(
                "`{}`: asked at version {}, declared here at version {}",
                ask.kind, ask.version, spec.version
            ));
        }
        let input = match (codec.decode)(&ask.input) {
            Ok(input) => input,
            Err(reason) => return Answer::refused(format!("cannot read the input: {reason}")),
        };
        let mut value = (codec.fresh)();
        if let Err(reason) = (codec.decode_value.unwrap())(&ask.value, &mut *value) {
            return Answer::refused(format!("cannot read the value: {reason}"));
        }
        if got.elapsed() > ask.deadline {
            return Answer::refused("the deadline passed before the listeners ran");
        }
        let out = self
            .inner
            .alter(key, input, value, codec.clone_value.unwrap());
        if got.elapsed() > ask.deadline {
            return Answer::Refused {
                listener: None,
                owner: None,
                reason: format!(
                    "the listeners ended past the deadline of {:?}",
                    ask.deadline
                ),
                failed: true,
            };
        }
        if let Some(r) = out.refused {
            return Answer::Refused {
                listener: Some(r.listener),
                owner: r.owner,
                reason: r.reason,
                failed: r.failed,
            };
        }
        Answer::Done {
            value: (codec.encode_value.unwrap())(&*out.value).expect("the point's value"),
        }
    }
}

// --- The points of the tests ---

#[derive(Clone, Debug, PartialEq)]
struct Imported {
    path: String,
    count: u32,
}

/// A library's event: photos were imported. Relayable.
struct LibraryImported;
impl Point for LibraryImported {
    const ID: PointId = PointId::new("library.imported");
    const KIND: PointKind = PointKind::Event;
    type Input = Imported;
    type Value = ();
}

/// `path|count`; extra fields after them are ignored (a newer sender).
impl Relayed for LibraryImported {
    fn encode(input: &Imported) -> String {
        format!("{}|{}", input.path, input.count)
    }
    fn decode(text: &str) -> Result<Imported, String> {
        let mut parts = text.split('|');
        let path = parts.next().ok_or("no path")?.to_owned();
        let count = parts
            .next()
            .ok_or("no count")?
            .parse()
            .map_err(|e| format!("count: {e}"))?;
        Ok(Imported { path, count })
    }
}

/// A journaled event kept in its process.
struct StepComputed;
impl Point for StepComputed {
    const ID: PointId = PointId::new("engine.step");
    const KIND: PointKind = PointKind::Event;
    type Input = u32;
    type Value = ();
}

/// An event emitted directly, not journaled.
struct VolumeMounted;
impl Point for VolumeMounted {
    const ID: PointId = PointId::new("volume.mounted");
    const KIND: PointKind = PointKind::Event;
    type Input = String;
    type Value = ();
}

/// A request: not journaled.
struct DocClose;
impl Point for DocClose {
    const ID: PointId = PointId::new("doc.close");
    const KIND: PointKind = PointKind::Request;
    type Input = String;
    type Value = Vec<String>;
}

const IMPORTED: Codec = Codec::of::<LibraryImported>();

const POINTS: &[PointDecl] = &[
    PointDecl::of::<LibraryImported>("Photos were imported.").facets(&[relayable(&IMPORTED)]),
    PointDecl::of::<StepComputed>("A render step was computed.").facets(&[JOURNALED]),
    PointDecl::of::<VolumeMounted>("A volume was mounted."),
];

fn imported(path: &str, count: u32) -> Imported {
    Imported {
        path: path.into(),
        count,
    }
}

const WAIT: Duration = Duration::from_secs(5);

// --- Declaring ---

#[test]
fn the_table_says_what_each_point_needs() {
    let says = |d: &PointDecl| (is_journaled(d.facets), codec(d.facets).is_some());
    assert_eq!(says(&POINTS[2]), (false, false));
    assert_eq!(says(&POINTS[1]), (true, false));
    // A relayable event is journaled by the bus that declares it.
    assert_eq!(says(&POINTS[0]), (false, true));
    assert_eq!(POINTS[0].required(), [&bus_contract_crossing::CONTRACT]);
    assert_eq!(POINTS[1].required(), [&bus_contract_journal::CONTRACT]);
    assert!(POINTS[2].required().is_empty());
    let spec = POINTS[0].spec();
    assert_eq!(codec(&spec.facets), Some(&IMPORTED));
}

#[test]
fn a_bus_without_the_journal_refuses_journaled_and_relayable_points() {
    let bus: Arc<dyn Bus> = Arc::new(Memory::new(false, false, 16));
    assert!(!offers(&*bus, &bus_contract_journal::CONTRACT));
    assert!(!offers(&*bus, &bus_contract_crossing::CONTRACT));
    assert_eq!(
        declare_all(&*bus, POINTS),
        [
            Refused::Unsupported {
                point: "library.imported",
                contract: "crossing"
            },
            Refused::Unsupported {
                point: "engine.step",
                contract: "journal"
            },
        ]
    );
    // Left undeclared; the plain event point is there.
    let ids: Vec<_> = bus.points().iter().map(|p| p.id).collect();
    assert_eq!(ids, ["volume.mounted"]);
}

#[test]
fn a_bus_with_the_journal_only_refuses_relayable_points() {
    let bus: Arc<dyn Bus> = Arc::new(Memory::new(true, false, 16));
    assert!(offers(&*bus, &bus_contract_journal::CONTRACT));
    assert!(!offers(&*bus, &bus_contract_crossing::CONTRACT));
    assert_eq!(
        declare_all(&*bus, POINTS),
        [Refused::Unsupported {
            point: "library.imported",
            contract: "crossing"
        }]
    );
}

#[test]
fn only_an_event_point_is_journaled_and_a_codec_fits_its_point() {
    let bus = Memory::full();
    const TABLE: &[PointDecl] = &[
        PointDecl::of::<DocClose>("A document closes.").facets(&[JOURNALED]),
        PointDecl::of::<VolumeMounted>("A volume.").facets(&[relayable(&IMPORTED)]),
    ];
    let refused = declare_all(&*bus, TABLE);
    assert!(
        matches!(
            refused.as_slice(),
            [
                Refused::Contract {
                    point: "doc.close",
                    contract: "journal",
                    ..
                },
                Refused::Contract {
                    point: "volume.mounted",
                    contract: "crossing",
                    ..
                },
            ]
        ),
        "{refused:?}"
    );
    assert!(bus.points().is_empty());
    let err = refused[0].to_string();
    assert!(err.contains("only an event point"), "{err}");
}

#[test]
fn introspection_shows_journaled_and_relayable_points() {
    let bus = Memory::full();
    assert!(declare_all(&*bus, POINTS).is_empty());
    let shown: Vec<_> = bus
        .points()
        .iter()
        .map(|p| (p.id, is_journaled(&p.facets), codec(&p.facets).is_some()))
        .collect();
    assert_eq!(
        shown,
        [
            ("engine.step", true, false),
            ("library.imported", true, true),
            ("volume.mounted", false, false),
        ]
    );
    assert_eq!(bus_contract_journal::CONTRACT.name, "journal");
    assert_eq!(bus_contract_crossing::CONTRACT.name, "crossing");
}

// --- Journal ---

#[test]
fn publish_numbers_entries_and_never_runs_a_listener_on_the_publisher() {
    let bus = Memory::full();
    assert!(declare_all(&*bus, POINTS).is_empty());
    let heard: Arc<Mutex<Vec<(u32, ThreadId)>>> = Arc::default();
    let h = heard.clone();
    bus.listen::<StepComputed>("ui.steps", ListenOpts::new(), move |n, _| {
        h.lock().unwrap().push((*n, thread::current().id()));
        Ok(())
    })
    .unwrap();

    let journal = bus.journal().unwrap();
    assert_eq!(journal.last_seq(), 0);
    let seqs: Vec<u64> = (0..5)
        .map(|n| journal.publish::<StepComputed>(n).unwrap())
        .collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5]);
    assert_eq!(journal.last_seq(), 5);

    assert!(journal.flush(WAIT));
    let heard = heard.lock().unwrap();
    let order: Vec<u32> = heard.iter().map(|h| h.0).collect();
    assert_eq!(
        order,
        [0, 1, 2, 3, 4],
        "delivered once each, in journal order"
    );
    assert!(heard.iter().all(|h| h.1 != thread::current().id()));
}

#[test]
fn a_slow_listener_delays_the_next_entries_not_the_publisher() {
    let bus = Memory::full();
    assert!(declare_all(&*bus, POINTS).is_empty());
    bus.listen::<StepComputed>("slow", ListenOpts::new(), |_, _| {
        thread::sleep(Duration::from_millis(200));
        Ok(())
    })
    .unwrap();
    let journal = bus.journal().unwrap();
    let start = Instant::now();
    for n in 0..3 {
        journal.publish::<StepComputed>(n).unwrap();
    }
    assert!(start.elapsed() < Duration::from_millis(150));
    assert!(journal.flush(WAIT));
    assert!(start.elapsed() >= Duration::from_millis(600));
}

#[test]
fn publishing_on_a_point_not_journaled_records_nothing() {
    let bus = Memory::full();
    assert!(declare_all(&*bus, POINTS).is_empty());
    let journal = bus.journal().unwrap();
    assert_eq!(
        journal.publish::<VolumeMounted>("/media/card".into()),
        Err(bus_contract_journal::not_journaled("volume.mounted"))
    );
    assert_eq!(
        journal.publish::<DocClose>("a.txt".into()),
        Err(bus_contract_journal::not_journaled("doc.close"))
    );
    assert_eq!(journal.last_seq(), 0);
}

#[test]
fn since_reads_from_a_number_and_reports_a_gap() {
    let bus: Arc<dyn Bus> = Arc::new(Memory::new(true, true, 3));
    assert!(declare_all(&*bus, POINTS).is_empty());
    let journal = bus.journal().unwrap();
    let first = journal.publish::<StepComputed>(10).unwrap();
    journal.publish_caused::<StepComputed>(11, first).unwrap();

    let read = journal.since(0, 10);
    assert!(!read.gap);
    assert_eq!(read.last_seq, 2);
    assert_eq!(read.entries[1].cause, Some(1));
    assert_eq!(read.entries[1].read::<StepComputed>(), Some(&11));
    assert_eq!(read.entries[1].read::<VolumeMounted>(), None);
    assert!(read.entries[1].origin.is_none());

    for n in 12..16 {
        journal.publish::<StepComputed>(n).unwrap();
    }
    // The window keeps 3: entries 2 and 3 are gone.
    let read = journal.since(1, 10);
    assert!(read.gap);
    let seqs: Vec<u64> = read.entries.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [4, 5, 6]);
    let read = journal.since(3, 1);
    assert!(!read.gap);
    assert_eq!(read.entries.len(), 1);
    assert!(journal.since(6, 10).entries.is_empty());
}

#[test]
fn wait_returns_the_next_matching_entry() {
    let bus = Memory::full();
    assert!(declare_all(&*bus, POINTS).is_empty());
    let journal = bus.journal().unwrap();
    journal.publish::<StepComputed>(1).unwrap();
    journal
        .publish::<LibraryImported>(imported("/a", 2))
        .unwrap();

    // Already there: at once.
    let e = journal.wait(&["library.*"], 0, Duration::ZERO).unwrap();
    assert_eq!(e.seq, 2);
    assert_eq!(journal.wait(&[], 0, Duration::ZERO).unwrap().seq, 1);
    // Nothing after: times out.
    assert!(
        journal
            .wait(&["library.*"], 2, Duration::from_millis(20))
            .is_none()
    );

    // Published later, from another thread.
    let b = bus.clone();
    let publisher = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        b.journal()
            .unwrap()
            .publish::<LibraryImported>(imported("/b", 3))
            .unwrap()
    });
    let (seq, input) = journal.wait_for::<LibraryImported>(2, WAIT).unwrap();
    assert_eq!(seq, publisher.join().unwrap());
    assert_eq!(input, imported("/b", 3));
}

#[test]
fn patterns_match_an_id_a_prefix_or_everything() {
    assert!(kind_matches("vault.changed", "vault.changed"));
    assert!(kind_matches("vault.*", "vault.changed"));
    assert!(kind_matches("vault.*", "vault.stack.added"));
    assert!(kind_matches("*", "anything"));
    assert!(!kind_matches("vault.*", "vault"));
    assert!(!kind_matches("vault.*", "vaults.changed"));
    assert!(!kind_matches("vault", "vault.changed"));
}

// --- Relay ---

fn node_origin() -> Origin {
    Origin::process("node")
}

/// A node and a desktop, each with its bus, and a listener on the
/// desktop that records what it hears.
/// What a listener heard.
type Heard = Arc<Mutex<Vec<Imported>>>;

fn node_and_desktop() -> (Arc<dyn Bus>, Arc<dyn Bus>, Heard) {
    let node = Memory::full();
    let desktop = Memory::full();
    assert!(declare_all(&*node, POINTS).is_empty());
    assert!(declare_all(&*desktop, POINTS).is_empty());
    let heard: Heard = Arc::default();
    let h = heard.clone();
    desktop
        .listen::<LibraryImported>("desktop.grid", ListenOpts::new(), move |i, _| {
            h.lock().unwrap().push(i.clone());
            Ok(())
        })
        .unwrap();
    (node, desktop, heard)
}

#[test]
fn relayable_events_cross_and_are_delivered_as_local_ones() {
    let (node, desktop, heard) = node_and_desktop();
    let nj = node.journal().unwrap();
    let a = nj.publish::<LibraryImported>(imported("/a", 1)).unwrap();
    nj.publish::<StepComputed>(7).unwrap(); // journaled, not relayable
    nj.publish_caused::<LibraryImported>(imported("/b", 2), a)
        .unwrap();

    let read = node.relay().unwrap().resume(Cursor::START, 100);
    assert_eq!(read.lost, None);
    let kinds: Vec<(u64, &str, &str)> = read
        .entries
        .iter()
        .map(|e| (e.seq, e.kind.as_str(), e.text.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (1, "library.imported", "/a|1"),
            (3, "library.imported", "/b|2")
        ]
    );
    assert_eq!(read.entries[1].cause, Some(1));
    assert_eq!(read.next, node.relay().unwrap().cursor());

    for e in read.entries {
        desktop.relay().unwrap().ingest(&node_origin(), e).unwrap();
    }
    let dj = desktop.journal().unwrap();
    assert!(dj.flush(WAIT));
    assert_eq!(
        *heard.lock().unwrap(),
        [imported("/a", 1), imported("/b", 2)]
    );
    let kept = dj.since(0, 10);
    assert_eq!(kept.entries[1].origin, Some(node_origin()));
    assert_eq!(kept.entries[1].cause, Some(1), "the sender's number");
    assert_eq!(kept.entries[1].text.as_deref(), Some("/b|2"));
    assert_eq!(
        kept.entries[1].read::<LibraryImported>(),
        Some(&imported("/b", 2))
    );

    // Taken in, not served again.
    assert!(
        desktop
            .relay()
            .unwrap()
            .resume(Cursor::START, 100)
            .entries
            .is_empty()
    );
}

#[test]
fn a_reader_resumes_from_its_cursor_and_in_pages() {
    let (node, _, _) = node_and_desktop();
    let nj = node.journal().unwrap();
    let relay = node.relay().unwrap();
    for n in 0..5 {
        nj.publish::<LibraryImported>(imported("/p", n)).unwrap();
    }
    let page = relay.resume(Cursor::START, 2);
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.next.seq, 2);
    let rest = relay.resume(page.next, 100);
    let seqs: Vec<u64> = rest.entries.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [3, 4, 5]);
    assert_eq!(rest.lost, None);
    assert!(relay.resume(rest.next, 100).entries.is_empty());
}

#[test]
fn a_reader_learns_it_lost_entries_to_the_window_or_a_restart() {
    let node: Arc<dyn Bus> = Arc::new(Memory::new(true, true, 2));
    assert!(declare_all(&*node, POINTS).is_empty());
    let nj = node.journal().unwrap();
    nj.publish::<LibraryImported>(imported("/a", 1)).unwrap();
    let cursor = node.relay().unwrap().cursor();
    for n in 2..6 {
        nj.publish::<LibraryImported>(imported("/a", n)).unwrap();
    }
    let read = node.relay().unwrap().resume(cursor, 100);
    assert_eq!(read.lost, Some(Lost::Gap));
    let seqs: Vec<u64> = read.entries.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [4, 5]);

    // The node restarted: a new bus, a new run, numbers from 1 again.
    let restarted: Arc<dyn Bus> = Arc::new(Memory::new(true, true, 16));
    assert!(declare_all(&*restarted, POINTS).is_empty());
    restarted
        .journal()
        .unwrap()
        .publish::<LibraryImported>(imported("/new", 1))
        .unwrap();
    let read = restarted.relay().unwrap().resume(read.next, 100);
    assert_eq!(read.lost, Some(Lost::Restarted));
    assert_eq!(read.entries.len(), 1);
    assert_eq!(read.entries[0].seq, 1);
    assert_ne!(read.next.run, cursor.run);
}

#[test]
fn ingest_refuses_what_it_cannot_read_and_keeps_unknown_kinds_as_text() {
    let (_, desktop, heard) = node_and_desktop();
    let relay = desktop.relay().unwrap();
    let journal = desktop.journal().unwrap();

    // A newer sender's extra field is read; a broken payload is not.
    relay
        .ingest(
            &node_origin(),
            RelayedEntry::new(1, "library.imported", "/a|4|new-field"),
        )
        .unwrap();
    let err = relay
        .ingest(
            &node_origin(),
            RelayedEntry::new(2, "library.imported", "/a"),
        )
        .unwrap_err();
    assert_eq!(
        err,
        IngestError::Decode {
            kind: "library.imported".into(),
            reason: "no count".into()
        }
    );
    // Declared here but not relayable: refused.
    let err = relay
        .ingest(&node_origin(), RelayedEntry::new(3, "engine.step", "7"))
        .unwrap_err();
    assert_eq!(
        err,
        IngestError::NotRelayable {
            kind: "engine.step".into()
        }
    );
    assert!(err.to_string().contains("not relayable"), "{err}");
    // Unknown here: kept as text, heard by no one.
    let seq = relay
        .ingest(
            &node_origin(),
            RelayedEntry::new(4, "extract.item", "{\"n\":1}"),
        )
        .unwrap();
    assert_eq!(seq, 2);

    assert!(journal.flush(WAIT));
    assert_eq!(*heard.lock().unwrap(), [imported("/a", 4)]);
    let kept = journal.since(1, 10).entries;
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].kind, "extract.item");
    assert!(kept[0].input.is_none());
    assert_eq!(kept[0].text.as_deref(), Some("{\"n\":1}"));
}

// --- Crossing: requests answered on the other side ---

/// A request the node answers for the desktop: delete files. The value
/// is what each side did.
struct FileDelete;
impl Point for FileDelete {
    const ID: PointId = PointId::new("files.delete");
    const KIND: PointKind = PointKind::Request;
    type Input = String;
    type Value = Vec<String>;
}

impl Relayed for FileDelete {
    fn encode(input: &String) -> String {
        input.clone()
    }
    fn decode(text: &str) -> Result<String, String> {
        Ok(text.to_owned())
    }
}

/// `a,b,c`; empty for no step.
impl RelayedRequest for FileDelete {
    fn encode_value(value: &Vec<String>) -> String {
        value.join(",")
    }
    fn decode_value(text: &str) -> Result<Vec<String>, String> {
        if text.contains('|') {
            return Err("a `|` in the value".into());
        }
        Ok(text
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect())
    }
}

/// A plain alter point: it never crosses.
struct Title;
impl Point for Title {
    const ID: PointId = PointId::new("doc.title");
    type Input = ();
    type Value = String;
}

impl Relayed for Title {
    fn encode(_: &()) -> String {
        String::new()
    }
    fn decode(_: &str) -> Result<(), String> {
        Ok(())
    }
}

const ASK_DEADLINE: Duration = Duration::from_millis(300);

const IMPORTED_CROSSES: &[Facet] = &[relayable_event::<LibraryImported>()];
const DELETE_CROSSES: &[Facet] = &[relayable_request::<FileDelete>()];

fn crossing_table(version: u32) -> [PointDecl; 2] {
    [
        PointDecl::of::<LibraryImported>("Photos were imported.")
            .facets(IMPORTED_CROSSES)
            .version(version),
        PointDecl::of::<FileDelete>("Files are deleted.")
            .facets(DELETE_CROSSES)
            .version(version),
    ]
}

fn desktop_origin() -> Origin {
    Origin::process("desktop")
}

/// A desktop asking `files.delete` of a node through a direct call: the
/// transport is a function, the meaning is the contract's. The desktop
/// has a listener of its own before the far side's turn.
fn asker_and_node(node_version: u32) -> (Arc<dyn Bus>, Arc<dyn Bus>) {
    let node = Memory::full();
    let desktop = Memory::full();
    assert!(declare_all(&*node, &crossing_table(node_version)).is_empty());
    assert!(declare_all(&*desktop, &crossing_table(1)).is_empty());
    desktop
        .listen::<FileDelete>("desktop.confirm", ListenOpts::new(), |_, did| {
            did.push("desktop.confirm".into());
            Ok(())
        })
        .unwrap();
    let far = node.clone();
    desktop
        .listen_any(
            bus_contract::PointKey::of::<FileDelete>(),
            "desktop.node".into(),
            ListenOpts::new().deadline(ASK_DEADLINE).owner("bridge"),
            far_listener(
                Codec::request::<FileDelete>(),
                ASK_DEADLINE,
                1,
                move |ask| Some(far.relay().unwrap().answer(&desktop_origin(), ask)),
            ),
        )
        .unwrap();
    (desktop, node)
}

fn node_deletes(node: &Arc<dyn Bus>) -> Arc<Mutex<Vec<String>>> {
    let ran: Arc<Mutex<Vec<String>>> = Arc::default();
    let r = ran.clone();
    node.listen::<FileDelete>("node.vault", ListenOpts::new(), move |path, did| {
        r.lock().unwrap().push(path.clone());
        did.push("node.vault".into());
        Ok(())
    })
    .unwrap();
    ran
}

#[test]
fn a_relayable_request_is_fail_closed_and_only_events_and_requests_cross() {
    let [_, delete] = crossing_table(3);
    assert_eq!(delete.kind, PointKind::Request);
    // Fail-closed once a bus applies it (`bus_contract_crossing::applied`).
    assert!(!is_journaled(delete.facets) && codec(delete.facets).is_some());
    assert_eq!(delete.version, 3);
    assert_eq!(delete.required(), [&bus_contract_crossing::CONTRACT]);
    assert!(Codec::request::<FileDelete>().answers());
    assert!(!Codec::of::<LibraryImported>().answers());

    let bus = Memory::full();
    assert!(declare_all(&*bus, &crossing_table(3)).is_empty());
    let info = bus
        .points()
        .into_iter()
        .find(|p| p.id == "files.delete")
        .unwrap();
    assert!(codec(&info.facets).is_some() && info.fail_closed && !is_journaled(&info.facets));
    assert_eq!(info.version, 3);

    let bus = Memory::full();
    const TABLE: &[PointDecl] = &[
        PointDecl::of::<Title>("A title.").facets(&[relayable_event::<Title>()]),
        PointDecl::of::<FileDelete>("No value codec.").facets(&[relayable_event::<FileDelete>()]),
    ];
    let refused = declare_all(&*bus, TABLE);
    let points: Vec<_> = refused
        .iter()
        .map(|r| match r {
            Refused::Contract {
                point,
                contract: "crossing",
                ..
            } => *point,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(points, ["doc.title", "files.delete"]);
    let err = refused[0].to_string();
    assert!(err.contains("cannot cross"), "{err}");
}

#[test]
fn a_request_answered_on_the_other_side_means_what_it_means_here() {
    let (desktop, node) = asker_and_node(1);
    let ran = node_deletes(&node);
    let out = desktop.alter::<FileDelete>("/a.jpg".into(), Vec::new());
    assert!(out.refused.is_none(), "{:?}", out.refused);
    assert_eq!(out.value, ["desktop.confirm", "node.vault"]);
    assert_eq!(*ran.lock().unwrap(), ["/a.jpg"]);

    // The same request emitted on the node itself: the same listeners,
    // the same value.
    let here = node.alter::<FileDelete>("/a.jpg".into(), vec!["desktop.confirm".into()]);
    assert_eq!(here.value, out.value);
}

#[test]
fn a_veto_on_the_other_side_refuses_with_its_reason() {
    let (desktop, node) = asker_and_node(1);
    node.listen::<FileDelete>("node.guard", ListenOpts::new(), |_, _| {
        Err(HookError::refuse("the volume is read-only"))
    })
    .unwrap();
    let ran = node_deletes(&node);
    let out = desktop.alter::<FileDelete>("/a.jpg".into(), Vec::new());
    let refusal = out.refused.expect("refused");
    assert_eq!(refusal.listener, "desktop.node");
    assert!(!refusal.failed);
    assert!(
        refusal.reason.contains("node.guard") && refusal.reason.contains("read-only"),
        "{}",
        refusal.reason
    );
    assert!(ran.lock().unwrap().is_empty(), "the veto stopped the node");
    assert_eq!(out.value, ["desktop.confirm"], "the far edit is dropped");
}

#[test]
fn no_answer_in_time_is_a_refusal() {
    let desktop = Memory::full();
    assert!(declare_all(&*desktop, &crossing_table(1)).is_empty());
    let key = bus_contract::PointKey::of::<FileDelete>();
    desktop
        .listen_any(
            key,
            "desktop.gone".into(),
            ListenOpts::new(),
            far_listener(Codec::request::<FileDelete>(), ASK_DEADLINE, 1, |_| None),
        )
        .unwrap();
    let out = desktop.alter::<FileDelete>("/a.jpg".into(), Vec::new());
    let refusal = out.refused.expect("no answer refuses");
    assert!(refusal.reason.contains("no answer"), "{}", refusal.reason);

    // An answer that comes after the deadline is dropped: still refused.
    let desktop = Memory::full();
    assert!(declare_all(&*desktop, &crossing_table(1)).is_empty());
    desktop
        .listen_any(
            key,
            "desktop.slow".into(),
            ListenOpts::new(),
            far_listener(
                Codec::request::<FileDelete>(),
                Duration::from_millis(20),
                1,
                |ask| {
                    thread::sleep(Duration::from_millis(60));
                    Some(RelayedAnswer::to(&ask, Answer::Done { value: "x".into() }))
                },
            ),
        )
        .unwrap();
    let out = desktop.alter::<FileDelete>("/a.jpg".into(), Vec::new());
    let refusal = out.refused.expect("a late answer refuses");
    assert!(refusal.reason.contains("after"), "{}", refusal.reason);
    assert!(out.value.is_empty());

    // An answer to another ask, or one that cannot be read: refused.
    for answer in [
        |ask: &RelayedAsk| {
            let mut other = ask.clone();
            other.id += 1;
            RelayedAnswer::to(&other, Answer::Done { value: "x".into() })
        },
        |ask: &RelayedAsk| {
            RelayedAnswer::to(
                ask,
                Answer::Done {
                    value: "a|b".into(),
                },
            )
        },
    ] {
        let desktop = Memory::full();
        assert!(declare_all(&*desktop, &crossing_table(1)).is_empty());
        desktop
            .listen_any(
                key,
                "desktop.odd".into(),
                ListenOpts::new(),
                far_listener(
                    Codec::request::<FileDelete>(),
                    ASK_DEADLINE,
                    1,
                    move |ask| Some(answer(&ask)),
                ),
            )
            .unwrap();
        let out = desktop.alter::<FileDelete>("/a.jpg".into(), Vec::new());
        assert!(out.refused.is_some());
        assert!(out.value.is_empty());
    }
}

#[test]
fn the_far_side_refuses_an_ask_past_its_deadline() {
    let node = Memory::full();
    assert!(declare_all(&*node, &crossing_table(1)).is_empty());
    let ran = node_deletes(&node);
    let relay = node.relay().unwrap();
    let ask = RelayedAsk::new(1, 1, "files.delete", Duration::ZERO, "/a", "");
    thread::sleep(Duration::from_millis(1));
    match relay.answer(&desktop_origin(), ask).answer {
        Answer::Refused { listener: None, .. } => {}
        other => panic!("{other:?}"),
    }
    assert!(ran.lock().unwrap().is_empty(), "no listener ran");

    // Listeners that end past the deadline: refused (failed), whatever
    // they did, as the asker holds it.
    node.listen::<FileDelete>("node.slow", ListenOpts::new(), |_, _| {
        thread::sleep(Duration::from_millis(40));
        Ok(())
    })
    .unwrap();
    let ask = RelayedAsk::new(1, 2, "files.delete", Duration::from_millis(10), "/b", "");
    match relay.answer(&desktop_origin(), ask).answer {
        Answer::Refused { failed: true, .. } => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn each_ask_is_answered_once() {
    let node = Memory::full();
    assert!(declare_all(&*node, &crossing_table(1)).is_empty());
    let ran = node_deletes(&node);
    let relay = node.relay().unwrap();
    let ask = RelayedAsk::new(7, 1, "files.delete", ASK_DEADLINE, "/a", "");
    let first = relay.answer(&desktop_origin(), ask.clone());
    let again = relay.answer(&desktop_origin(), ask.clone());
    assert_eq!(first, again);
    assert_eq!(
        first.answer,
        Answer::Done {
            value: "node.vault".into()
        }
    );
    assert_eq!(ran.lock().unwrap().len(), 1, "run once");
    // Another run (the asker restarted) is another ask.
    let mut restarted = ask;
    restarted.run = 8;
    relay.answer(&desktop_origin(), restarted);
    assert_eq!(ran.lock().unwrap().len(), 2);
}

#[test]
fn what_is_not_a_relayable_request_here_is_refused() {
    let node = Memory::full();
    assert!(declare_all(&*node, &crossing_table(1)).is_empty());
    assert!(declare_all(&*node, &[PointDecl::of::<DocClose>("Local only.")]).is_empty());
    let relay = node.relay().unwrap();
    for (n, kind) in ["unknown.point", "doc.close", "library.imported"]
        .into_iter()
        .enumerate()
    {
        let ask = RelayedAsk::new(1, n as u64, kind, ASK_DEADLINE, "/a|1", "");
        match relay.answer(&desktop_origin(), ask).answer {
            Answer::Refused { listener: None, .. } => {}
            other => panic!("{kind}: {other:?}"),
        }
    }
    // A request is asked, not ingested.
    assert_eq!(
        relay.ingest(&node_origin(), RelayedEntry::new(1, "files.delete", "/a")),
        Err(IngestError::NotRelayable {
            kind: "files.delete".into()
        })
    );
}

#[test]
fn every_entry_taken_in_carries_its_origin() {
    let desktop = Memory::full();
    assert!(declare_all(&*desktop, &crossing_table(1)).is_empty());
    let from = Origin::process("agent-socket").agent("claude");
    desktop
        .relay()
        .unwrap()
        .ingest(&from, RelayedEntry::new(1, "library.imported", "/a|1"))
        .unwrap();
    let kept = desktop.journal().unwrap().since(0, 10);
    assert_eq!(kept.entries[0].origin, Some(from.clone()));
    assert_eq!(from.to_string(), "agent-socket (agent claude)");
    assert_eq!(node_origin().to_string(), "node");
}

#[test]
fn an_authority_refuses_what_comes_in_before_anything_is_recorded_or_run() {
    let node = Memory::full();
    assert!(declare_all(&*node, &crossing_table(1)).is_empty());
    let ran = node_deletes(&node);
    let relay = node.relay().unwrap();
    let seen: Arc<Mutex<Vec<(String, String, bool)>>> = Arc::default();
    let s = seen.clone();
    relay.authorize(move |c| {
        s.lock()
            .unwrap()
            .push((c.origin.to_string(), c.kind.to_owned(), c.ask));
        Ok(())
    });
    relay.authorize(|c| match &c.origin.agent {
        Some(agent) if c.kind == "files.delete" => Err(format!("{agent} may not delete")),
        _ => Ok(()),
    });

    let agent = Origin::process("desktop").agent("claude");
    let ask = RelayedAsk::new(1, 1, "files.delete", ASK_DEADLINE, "/a", "");
    assert_eq!(
        relay.answer(&agent, ask).answer,
        Answer::refused("claude may not delete")
    );
    assert!(ran.lock().unwrap().is_empty());
    // The same ask from the process itself is let through.
    let ask = RelayedAsk::new(1, 2, "files.delete", ASK_DEADLINE, "/a", "");
    assert!(matches!(
        relay.answer(&desktop_origin(), ask).answer,
        Answer::Done { .. }
    ));
    assert_eq!(seen.lock().unwrap().len(), 2, "each authority, in order");

    // Events too; a panicking authority refuses.
    relay.authorize(|c| {
        if c.origin.process == "rogue" {
            panic!("boom");
        }
        Ok(())
    });
    let err = relay
        .ingest(
            &Origin::process("rogue"),
            RelayedEntry::new(1, "library.imported", "/a|1"),
        )
        .unwrap_err();
    assert!(matches!(err, IngestError::Refused { .. }), "{err}");
    assert!(err.to_string().contains("refused"), "{err}");
    assert_eq!(node.journal().unwrap().last_seq(), 0, "nothing recorded");
}

#[test]
fn a_point_crosses_only_at_the_same_version() {
    let (desktop, node) = asker_and_node(2);
    let ran = node_deletes(&node);
    let out = desktop.alter::<FileDelete>("/a.jpg".into(), Vec::new());
    let refusal = out.refused.expect("another version refuses");
    assert!(refusal.reason.contains("version"), "{}", refusal.reason);
    assert!(ran.lock().unwrap().is_empty());

    let err = node
        .relay()
        .unwrap()
        .ingest(
            &desktop_origin(),
            RelayedEntry::new(1, "library.imported", "/a|1"),
        )
        .unwrap_err();
    assert_eq!(
        err,
        IngestError::Version {
            kind: "library.imported".into(),
            sent: 1,
            here: 2
        }
    );

    // Served with its version.
    node.journal()
        .unwrap()
        .publish::<LibraryImported>(imported("/b", 1))
        .unwrap();
    let read = node.relay().unwrap().resume(Cursor::START, 10);
    assert_eq!(read.entries[0].version, 2);
}
