//! The `Journal` and `Relay` capabilities over a reference in-memory
//! implementation written against the contract alone: shared listeners
//! in registration order, no deadlines, no local listeners; a journal
//! with a window and a pump thread that delivers its entries in order.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use bus_contract::{
    Altered, AnyInput, AnyListener, AnyLocalListener, AnyValue, Bus, Capability, CloneFn, Codec,
    Collected, Cursor, Failure, FailureKind, FailureSink, FreshFn, IngestError, Journal,
    JournalEntry, ListenOpts, ListenerInfo, Lost, PendingRun, PendingWaker, Point, PointDecl,
    PointInfo, PointKey, PointKind, PointSpec, Refused, Relay, Relayed, RelayedEntry, Resumed,
    Since, declare_all, kind_matches, offers,
};

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

struct Inner {
    points: Mutex<HashMap<&'static str, (PointKey, PointSpec)>>,
    listeners: Mutex<Vec<(&'static str, String, ListenOpts, AnyListener)>>,
    sinks: Mutex<Vec<FailureSink>>,
    log: Mutex<Log>,
    changed: Condvar,
    run: u64,
    window: usize,
}

impl Inner {
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
        if spec.journaled && point.kind != PointKind::Event {
            return Err(Refused::NotEvent { point: point.id });
        }
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
        _: PointKey,
        _: AnyInput,
        value: AnyValue,
        _: CloneFn,
    ) -> Altered<AnyValue> {
        Altered::new(value)
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
                info.journaled = s.journaled;
                info.relayable = s.relay.is_some();
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

    fn journal(&self) -> Option<&dyn Journal> {
        self.journal.then_some(self as &dyn Journal)
    }

    fn relay(&self) -> Option<&dyn Relay> {
        (self.journal && self.relay).then_some(self as &dyn Relay)
    }
}

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
            return Err(Refused::NotJournaled { point: point.id });
        };
        if key.type_id != point.type_id {
            return Err(Refused::TypeMismatch { point: point.id });
        }
        if !spec.journaled {
            return Err(Refused::NotJournaled { point: point.id });
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
            let (Some(codec), None, Some(input)) = (spec.relay, &e.source, &e.input) else {
                continue;
            };
            let Some(text) = (codec.encode)(&**input) else {
                continue;
            };
            let mut out = RelayedEntry::new(e.seq, e.kind.clone(), text);
            out.cause = e.cause;
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

    fn ingest(&self, source: &str, relayed: RelayedEntry) -> Result<u64, IngestError> {
        let mut entry = JournalEntry::new(0, relayed.kind.clone());
        entry.cause = relayed.cause;
        entry.source = Some(source.to_owned());
        let Some((key, spec)) = self.inner.spec(&relayed.kind) else {
            entry.text = Some(relayed.text);
            return Ok(self.inner.record(entry, None));
        };
        let Some(codec) = spec.relay else {
            return Err(IngestError::NotRelayable { kind: relayed.kind });
        };
        let input = (codec.decode)(&relayed.text).map_err(|reason| IngestError::Decode {
            kind: relayed.kind.clone(),
            reason,
        })?;
        entry.input = Some(input.clone());
        entry.text = Some(relayed.text);
        Ok(self.inner.record(entry, Some((key, input, codec.fresh))))
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
    const ID: &'static str = "library.imported";
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
    const ID: &'static str = "engine.step";
    const KIND: PointKind = PointKind::Event;
    type Input = u32;
    type Value = ();
}

/// An event emitted directly, not journaled.
struct VolumeMounted;
impl Point for VolumeMounted {
    const ID: &'static str = "volume.mounted";
    const KIND: PointKind = PointKind::Event;
    type Input = String;
    type Value = ();
}

/// A request: not journaled.
struct DocClose;
impl Point for DocClose {
    const ID: &'static str = "doc.close";
    const KIND: PointKind = PointKind::Request;
    type Input = String;
    type Value = Vec<String>;
}

const POINTS: &[PointDecl] = &[
    PointDecl::of::<LibraryImported>("Photos were imported.")
        .relayable(Codec::of::<LibraryImported>()),
    PointDecl::of::<StepComputed>("A render step was computed.").journaled(),
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
    assert!(!POINTS[2].journaled && POINTS[2].relay.is_none());
    assert!(POINTS[1].journaled && POINTS[1].relay.is_none());
    assert!(POINTS[0].journaled && POINTS[0].relay.is_some());
    assert_eq!(
        POINTS[0].required(),
        [Capability::Journal, Capability::Relay]
    );
    assert_eq!(POINTS[1].required(), [Capability::Journal]);
    assert!(POINTS[2].required().is_empty());
    let spec = POINTS[0].spec();
    assert!(spec.journaled);
    assert_eq!(spec.relay, Some(Codec::of::<LibraryImported>()));
}

#[test]
fn a_bus_without_the_journal_refuses_journaled_and_relayable_points() {
    let bus: Arc<dyn Bus> = Arc::new(Memory::new(false, false, 16));
    assert!(!offers(&*bus, Capability::Journal));
    assert!(!offers(&*bus, Capability::Relay));
    assert_eq!(
        declare_all(&*bus, POINTS),
        [
            Refused::Unsupported {
                point: "library.imported",
                capability: Capability::Journal
            },
            Refused::Unsupported {
                point: "engine.step",
                capability: Capability::Journal
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
    assert!(offers(&*bus, Capability::Journal));
    assert!(!offers(&*bus, Capability::Relay));
    assert_eq!(
        declare_all(&*bus, POINTS),
        [Refused::Unsupported {
            point: "library.imported",
            capability: Capability::Relay
        }]
    );
}

#[test]
fn only_an_event_point_is_journaled_and_a_codec_fits_its_point() {
    let bus = Memory::full();
    let table = [
        PointDecl::of::<DocClose>("A document closes.").journaled(),
        PointDecl::of::<VolumeMounted>("A volume.").relayable(Codec::of::<LibraryImported>()),
    ];
    assert_eq!(
        declare_all(&*bus, &table),
        [
            Refused::NotEvent { point: "doc.close" },
            Refused::TypeMismatch {
                point: "volume.mounted"
            },
        ]
    );
    assert!(bus.points().is_empty());
    let err = Refused::NotEvent { point: "doc.close" }.to_string();
    assert!(err.contains("not an event point"), "{err}");
}

#[test]
fn introspection_shows_journaled_and_relayable_points() {
    let bus = Memory::full();
    assert!(declare_all(&*bus, POINTS).is_empty());
    let shown: Vec<_> = bus
        .points()
        .iter()
        .map(|p| (p.id, p.journaled, p.relayable))
        .collect();
    assert_eq!(
        shown,
        [
            ("engine.step", true, false),
            ("library.imported", true, true),
            ("volume.mounted", false, false),
        ]
    );
    assert_eq!(Capability::Journal.as_str(), "journal");
    assert_eq!(Capability::Relay.as_str(), "relay");
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
        Err(Refused::NotJournaled {
            point: "volume.mounted"
        })
    );
    assert_eq!(
        journal.publish::<DocClose>("a.txt".into()),
        Err(Refused::NotJournaled { point: "doc.close" })
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
    assert!(read.entries[1].source.is_none());

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
        desktop.relay().unwrap().ingest("node", e).unwrap();
    }
    let dj = desktop.journal().unwrap();
    assert!(dj.flush(WAIT));
    assert_eq!(
        *heard.lock().unwrap(),
        [imported("/a", 1), imported("/b", 2)]
    );
    let kept = dj.since(0, 10);
    assert_eq!(kept.entries[1].source.as_deref(), Some("node"));
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
            "node",
            RelayedEntry::new(1, "library.imported", "/a|4|new-field"),
        )
        .unwrap();
    let err = relay
        .ingest("node", RelayedEntry::new(2, "library.imported", "/a"))
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
        .ingest("node", RelayedEntry::new(3, "engine.step", "7"))
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
        .ingest("node", RelayedEntry::new(4, "extract.item", "{\"n\":1}"))
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
