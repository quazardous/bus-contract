//! The typed calls on `dyn Bus`, the contract table and the tools, over
//! a recording in-memory implementation written against the contract
//! alone: the listeners in registration order, no deadlines; local
//! listeners kept in thread-local storage, the bus holding only where
//! they are; refusals, fail-closed points and the `Defer` capability;
//! the `Timings` capability's types and its absence.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use std::any::TypeId;

use bus_contract::{
    Altered, AnyInput, AnyListener, AnyLocalListener, AnyValue, Bus, Cast, CloneFn, Collected,
    Failure, FailureKind, FailureSink, FreshFn, HookError, ListenOpts, ListenerInfo, PendingRun,
    PendingWaker, Point, PointDecl, PointId, PointInfo, PointKey, PointKind, PointSpec, Provided,
    Refusal, Refused, declare_all, missing_targets, offers, order_dump,
};
use bus_contract_defer::{BusDefer, Defer};
use bus_contract_timings::{
    BusTimings, ListenerTimings, OverBudget, PointTimings, TimingStats, Timings,
};

thread_local! {
    /// The local listeners registered on this thread, by key.
    static LOCAL: RefCell<HashMap<u64, AnyLocalListener>> = RefCell::new(HashMap::new());
}

/// Keys of local listeners, unique in the process.
static NEXT_LOCAL: AtomicU64 = AtomicU64::new(0);

/// How a listener runs: shared, or local to a thread under a key.
#[derive(Clone)]
enum Run {
    Shared(AnyListener),
    Local(ThreadId, u64),
}

impl Run {
    /// Runs it on this thread; `None` when it is bound to another one.
    fn call(
        &self,
        input: &(dyn Any + Send + Sync),
        value: &mut (dyn Any + Send),
    ) -> Option<Result<(), HookError>> {
        match self {
            Run::Shared(run) => Some(run(input, value)),
            Run::Local(thread, key) if *thread == thread::current().id() => {
                // Taken out first: the listener may emit or listen.
                let run = LOCAL.with(|l| l.borrow().get(key).cloned())?;
                Some(run(input, value))
            }
            Run::Local(..) => None,
        }
    }

    fn thread(&self) -> Option<ThreadId> {
        match self {
            Run::Local(thread, _) => Some(*thread),
            Run::Shared(_) => None,
        }
    }
}

/// What a queued emit hands its deferred listeners.
enum Work {
    Alter(AnyValue, CloneFn),
    Collect(FreshFn),
}

/// An emit's local part, queued for its thread.
struct Queued {
    thread: ThreadId,
    point: PointKey,
    input: AnyInput,
    work: Work,
    ids: Vec<String>,
}

/// A request asked through `Defer`, waiting for its thread's drain.
struct Asked {
    thread: ThreadId,
    ask: u64,
    point: PointKey,
    input: AnyInput,
    value: AnyValue,
    clone: CloneFn,
}

/// What one listener's answer means for the emit.
enum Step {
    /// Its edit counts.
    Kept,
    /// Its edit is dropped; the others run.
    Dropped(Failure),
    /// The request stops here (with the failure that stopped it, on a
    /// fail-closed point).
    Stop(Refusal, Option<Failure>),
}

/// Listeners in registration order, no deadlines: enough to show the
/// contract works with any implementation.
#[derive(Default)]
struct Recording {
    /// Every call, in order: `declare doc.close`, `listen doc.close x`,
    /// `alter doc.close`.
    calls: Mutex<Vec<String>>,
    points: Mutex<Vec<(PointKey, Option<PointSpec>)>>,
    listeners: Mutex<Vec<(&'static str, String, ListenOpts, Run)>>,
    sinks: Mutex<Vec<FailureSink>>,
    wakers: Mutex<Vec<PendingWaker>>,
    queued: Mutex<Vec<Queued>>,
    /// Offers `Defer` only when set.
    defers: bool,
    asked: Mutex<Vec<Asked>>,
    next_ask: AtomicU64,
}

impl Recording {
    fn with_defer() -> Recording {
        Recording {
            defers: true,
            ..Recording::default()
        }
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    fn run(&self, point: &'static str) -> Vec<(String, Option<String>, Run)> {
        self.listeners
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.0 == point)
            .map(|l| (l.1.clone(), l.2.owner.clone(), l.3.clone()))
            .collect()
    }

    fn add(&self, point: PointKey, id: String, opts: ListenOpts, run: Run) -> Result<(), Refused> {
        self.record(format!("listen {} {id}", point.id));
        let mut listeners = self.listeners.lock().unwrap();
        if listeners.iter().any(|l| l.0 == point.id && l.1 == id) {
            return Err(Refused::Duplicate {
                point: point.id,
                listener: id,
            });
        }
        listeners.push((point.id, id, opts, run));
        Ok(())
    }

    fn wake(&self, thread: ThreadId) {
        for wake in self.wakers.lock().unwrap().iter() {
            wake(thread);
        }
    }

    /// Queues the deferred listeners, one emit per thread, and wakes.
    fn defer(
        &self,
        point: PointKey,
        input: &AnyInput,
        deferred: &[(String, ThreadId)],
        work: impl Fn() -> Work,
    ) {
        let mut threads: Vec<ThreadId> = deferred.iter().map(|d| d.1).collect();
        threads.dedup();
        for thread in threads {
            self.queued.lock().unwrap().push(Queued {
                thread,
                point,
                input: input.clone(),
                work: work(),
                ids: deferred
                    .iter()
                    .filter(|d| d.1 == thread)
                    .map(|d| d.0.clone())
                    .collect(),
            });
            self.wake(thread);
        }
    }

    fn fail_closed(&self, point: &PointKey) -> bool {
        point.kind == PointKind::Request
            && self
                .points
                .lock()
                .unwrap()
                .iter()
                .any(|(k, s)| k.id == point.id && s.as_ref().is_some_and(|s| s.fail_closed))
    }

    /// What a listener's answer means, its failure sent to the sinks.
    fn judge(
        &self,
        point: &PointKey,
        id: &str,
        owner: &Option<String>,
        answer: Result<(), HookError>,
    ) -> Step {
        let request = point.kind == PointKind::Request;
        let message = match answer {
            Ok(()) => return Step::Kept,
            Err(HookError::Refuse(reason)) if request => {
                return Step::Stop(Refusal::new(point.id, id, owner.clone(), reason), None);
            }
            Err(e) => e.message().to_owned(),
        };
        let failure = Failure::new(point.id, id, owner.clone(), FailureKind::Error(message));
        for sink in self.sinks.lock().unwrap().iter() {
            sink(&failure);
        }
        if self.fail_closed(point) {
            Step::Stop(Refusal::from_failure(&failure), Some(failure))
        } else {
            Step::Dropped(failure)
        }
    }

    /// An alter emit; the listeners that ran on this thread with it.
    fn alter(
        &self,
        point: PointKey,
        input: AnyInput,
        mut value: AnyValue,
        clone: CloneFn,
    ) -> (Altered<AnyValue>, Vec<String>) {
        let mut ran = Vec::new();
        let mut failures = Vec::new();
        let mut deferred = Vec::new();
        let mut refused = None;
        for (id, owner, run) in self.run(point.id) {
            let mut v = clone(&*value);
            let Some(answer) = run.call(&*input, &mut *v) else {
                deferred.push((id, run.thread().unwrap()));
                continue;
            };
            ran.push(id.clone());
            match self.judge(&point, &id, &owner, answer) {
                Step::Kept => value = v,
                Step::Dropped(f) => failures.push(f),
                Step::Stop(r, f) => {
                    failures.extend(f);
                    refused = Some(r);
                    break;
                }
            }
        }
        if refused.is_some() {
            deferred.clear();
        }
        self.defer(point, &input, &deferred, || {
            Work::Alter(clone(&*value), clone)
        });
        let mut out = Altered::new(value);
        out.failures = failures;
        out.deferred = deferred.into_iter().map(|d| d.0).collect();
        out.refused = refused;
        (out, ran)
    }
}

impl Defer for Recording {
    fn ask_any(&self, point: PointKey, input: AnyInput, value: AnyValue, clone: CloneFn) -> u64 {
        self.record(format!("ask {}", point.id));
        let ask = self.next_ask.fetch_add(1, Ordering::Relaxed);
        let thread = thread::current().id();
        self.asked.lock().unwrap().push(Asked {
            thread,
            ask,
            point,
            input,
            value,
            clone,
        });
        self.wake(thread);
        ask
    }

    fn drain(&self) -> Vec<PendingRun> {
        let here = thread::current().id();
        // Taken at once: an ask made while draining waits for the next.
        let mine: Vec<Asked> = {
            let mut asked = self.asked.lock().unwrap();
            let (mine, rest) = std::mem::take(&mut *asked)
                .into_iter()
                .partition(|a| a.thread == here);
            *asked = rest;
            mine
        };
        mine.into_iter()
            .map(|a| {
                let (out, ran) = self.alter(a.point, a.input, a.value, a.clone);
                let mut run = PendingRun::new(a.point.id);
                run.ran = ran;
                run.failures = out.failures;
                run.refused = out.refused;
                run.ask = Some(a.ask);
                run
            })
            .collect()
    }

    fn waiting(&self) -> usize {
        let here = thread::current().id();
        self.asked
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a.thread == here)
            .count()
    }
}

impl Bus for Recording {
    fn declare_point(&self, point: PointKey, spec: PointSpec) -> Result<(), Refused> {
        self.record(format!("declare {} {}", point.id, point.kind.as_str()));
        let mut points = self.points.lock().unwrap();
        if points.iter().any(|(k, s)| k.id == point.id && s.is_some()) {
            return Err(Refused::AlreadyDeclared { point: point.id });
        }
        points.push((point, Some(spec)));
        Ok(())
    }

    fn listen_any(
        &self,
        point: PointKey,
        id: String,
        opts: ListenOpts,
        run: AnyListener,
    ) -> Result<(), Refused> {
        self.add(point, id, opts, Run::Shared(run))
    }

    fn listen_local_any(
        &self,
        point: PointKey,
        id: String,
        opts: ListenOpts,
        run: AnyLocalListener,
    ) -> Result<(), Refused> {
        if opts.threaded {
            return Err(Refused::Threaded {
                point: point.id,
                listener: id,
            });
        }
        let key = NEXT_LOCAL.fetch_add(1, Ordering::Relaxed);
        self.add(point, id, opts, Run::Local(thread::current().id(), key))?;
        LOCAL.with(|l| l.borrow_mut().insert(key, run));
        Ok(())
    }

    fn run_pending(&self) -> Vec<PendingRun> {
        let here = thread::current().id();
        let mine: Vec<Queued> = {
            let mut queued = self.queued.lock().unwrap();
            let (mine, rest) = std::mem::take(&mut *queued)
                .into_iter()
                .partition(|q| q.thread == here);
            *queued = rest;
            mine
        };
        mine.into_iter()
            .map(|q| {
                let mut out = PendingRun::new(q.point.id);
                let mut value = match &q.work {
                    Work::Alter(v, clone) => Some(clone(&**v)),
                    Work::Collect(_) => None,
                };
                // Still registered now: one removed since the emit is skipped.
                let now = self.run(q.point.id);
                for id in &q.ids {
                    let Some((_, owner, run)) = now.iter().find(|(i, ..)| i == id) else {
                        continue;
                    };
                    let mut v = match (&q.work, &value) {
                        (Work::Alter(_, clone), Some(v)) => clone(&**v),
                        (Work::Collect(fresh), _) => fresh(),
                        (Work::Alter(..), None) => unreachable!(),
                    };
                    let Some(answer) = run.call(&*q.input, &mut *v) else {
                        continue;
                    };
                    out.ran.push(id.clone());
                    match self.judge(&q.point, id, owner, answer) {
                        Step::Kept => {
                            if value.is_some() {
                                value = Some(v);
                            }
                        }
                        Step::Dropped(f) => out.failures.push(f),
                        Step::Stop(r, f) => {
                            out.failures.extend(f);
                            out.refused = Some(r);
                            break;
                        }
                    }
                }
                out
            })
            .collect()
    }

    fn add_pending_waker(&self, wake: PendingWaker) {
        self.wakers.lock().unwrap().push(wake);
    }

    fn unlisten(&self, point: &str, id: &str) -> bool {
        let mut listeners = self.listeners.lock().unwrap();
        let before = listeners.len();
        listeners.retain(|l| !(l.0 == point && l.1 == id));
        before != listeners.len()
    }

    fn forget(&self, owner: &str) -> usize {
        let mut listeners = self.listeners.lock().unwrap();
        let before = listeners.len();
        listeners.retain(|l| l.2.owner.as_deref() != Some(owner));
        before - listeners.len()
    }

    fn alter_any(
        &self,
        point: PointKey,
        input: AnyInput,
        value: AnyValue,
        clone: CloneFn,
    ) -> Altered<AnyValue> {
        self.record(format!("alter {}", point.id));
        self.alter(point, input, value, clone).0
    }

    fn collect_any(&self, point: PointKey, input: AnyInput, fresh: FreshFn) -> Collected<AnyValue> {
        self.record(format!("collect {}", point.id));
        let mut out = Collected::new();
        let mut deferred = Vec::new();
        for (id, owner, run) in self.run(point.id) {
            let mut v = fresh();
            let Some(answer) = run.call(&*input, &mut *v) else {
                deferred.push((id, run.thread().unwrap()));
                continue;
            };
            match self.judge(&point, &id, &owner, answer) {
                Step::Kept => out.parts.push((id, v)),
                Step::Dropped(f) => out.failures.push(f),
                Step::Stop(r, f) => {
                    out.failures.extend(f);
                    out.refused = Some(r);
                    deferred.clear();
                    break;
                }
            }
        }
        self.defer(point, &input, &deferred, || Work::Collect(fresh));
        out.deferred = deferred.into_iter().map(|d| d.0).collect();
        out
    }

    fn add_failure_sink(&self, sink: FailureSink) {
        self.sinks.lock().unwrap().push(sink);
    }

    fn points(&self) -> Vec<PointInfo> {
        self.points
            .lock()
            .unwrap()
            .iter()
            .map(|(k, s)| {
                let mut info = PointInfo::new(k.id, k.kind);
                if let Some(s) = s {
                    info.description = s.description.clone();
                    info.declared = true;
                    info.fail_closed = s.fail_closed;
                }
                info.listeners = self.listeners(k.id).unwrap_or_default();
                info
            })
            .collect()
    }

    fn listeners(&self, point: &str) -> Option<Vec<ListenerInfo>> {
        let listeners = self.listeners.lock().unwrap();
        let ids: Vec<&str> = listeners
            .iter()
            .filter(|l| l.0 == point)
            .map(|l| l.1.as_str())
            .collect();
        Some(
            listeners
                .iter()
                .filter(|l| l.0 == point)
                .map(|l| {
                    let mut info = ListenerInfo::new(l.1.clone());
                    info.owner = l.2.owner.clone();
                    info.after = l.2.after.clone();
                    info.before = l.2.before.clone();
                    info.priority = l.2.priority;
                    info.threaded = l.2.threaded;
                    info.local = matches!(l.3, Run::Local(..));
                    info.missing =
                        l.2.after
                            .iter()
                            .chain(&l.2.before)
                            .filter(|t| *t != bus_contract::LAST && !ids.contains(&t.as_str()))
                            .cloned()
                            .collect();
                    info
                })
                .collect(),
        )
    }

    fn capability(&self, contract: TypeId) -> Option<Provided<'_>> {
        (self.defers && contract == TypeId::of::<dyn Defer>())
            .then(|| Provided::new::<dyn Defer>(self, &AS_DEFER))
    }
}

fn as_defer(bus: &dyn Any) -> Option<&(dyn Defer + 'static)> {
    bus.downcast_ref::<Recording>().map(|b| b as _)
}
static AS_DEFER: Cast<dyn Defer> = as_defer;

struct DocClose;
impl Point for DocClose {
    const ID: PointId = PointId::new("doc.close");
    const KIND: PointKind = PointKind::Request;
    type Input = String;
    type Value = Vec<String>;
}

struct Findings;
impl Point for Findings {
    const ID: PointId = PointId::new("doc.findings");
    type Input = ();
    type Value = Vec<String>;
}

const POINTS: &[PointDecl] = &[
    PointDecl::of::<DocClose>("A document closes."),
    PointDecl::of::<Findings>("What each check finds.").deadline(std::time::Duration::from_secs(5)),
];

fn bus() -> Arc<dyn Bus> {
    Arc::new(Recording::default())
}

#[test]
fn the_table_declares_each_point_once_with_its_kind() {
    let bus = bus();
    assert!(declare_all(&*bus, POINTS).is_empty());
    assert_eq!(POINTS[0].kind, PointKind::Request);
    assert_eq!(POINTS[1].kind, PointKind::Alter);
    assert_eq!(
        POINTS[1].spec().deadline,
        Some(std::time::Duration::from_secs(5))
    );
    let points = bus.points();
    assert_eq!(points[0].id, "doc.close");
    assert_eq!(points[0].kind, PointKind::Request);
    // A second library (or application) declaring the same table is told.
    let again = declare_all(&*bus, POINTS);
    assert_eq!(again.len(), 2);
    assert_eq!(again[0], Refused::AlreadyDeclared { point: "doc.close" });
}

#[test]
fn typed_calls_go_through_the_erased_methods() {
    let bus = bus();
    bus.listen::<DocClose>(
        "doc.autosave",
        ListenOpts::new().owner("docs"),
        |doc, did| {
            did.push(format!("saved {doc}"));
            Ok(())
        },
    )
    .unwrap();
    bus.listen::<DocClose>("doc.broken", ListenOpts::new().owner("plugin"), |_, did| {
        did.clear();
        Err("no".into())
    })
    .unwrap();
    let failed = Arc::new(Mutex::new(Vec::new()));
    let sink = failed.clone();
    bus.on_failure(move |f| sink.lock().unwrap().push(f.listener.clone()));

    let out = bus.alter::<DocClose>("a.txt".into(), vec![]);
    assert_eq!(out.value, ["saved a.txt"], "the failing edit was dropped");
    assert_eq!(out.failures.len(), 1);
    assert_eq!(*failed.lock().unwrap(), ["doc.broken"]);

    bus.listen::<Findings>("disk", ListenOpts::new(), |_, out| {
        out.push("full".into());
        Ok(())
    })
    .unwrap();
    let found = bus.collect::<Findings>(());
    assert_eq!(
        found.parts,
        [("disk".to_string(), vec!["full".to_string()])]
    );

    assert_eq!(bus.forget("plugin"), 1);
    assert!(bus.unlisten("doc.close", "doc.autosave"));
    assert!(bus.alter::<DocClose>("b".into(), vec![]).value.is_empty());
}

#[test]
fn the_order_dump_and_missing_targets_read_any_implementation() {
    let bus = bus();
    bus.listen::<DocClose>(
        "doc.children",
        ListenOpts::new().owner("docs").priority(10),
        |_, _| Ok(()),
    )
    .unwrap();
    bus.listen::<DocClose>(
        "doc.place",
        ListenOpts::new().owner("docs").after("doc.registry"),
        |_, _| Ok(()),
    )
    .unwrap();
    bus.listen::<DocClose>("doc.invariants", ListenOpts::new().last(), |_, _| Ok(()))
        .unwrap();
    assert_eq!(
        order_dump(&*bus, &["doc.close"]),
        "doc.close\n  1. doc.children (owner docs): priority 10\n  2. doc.place (owner docs): after doc.registry; MISSING doc.registry\n  3. doc.invariants (owner -): last\n"
    );
    assert_eq!(
        missing_targets(&*bus, &["doc.close"]),
        ["doc.close: doc.place -> doc.registry"]
    );
}

/// What a library does on the bus it is given, as the application's
/// implementation sees it: each call, in order, with the point's kind.
#[test]
fn the_application_sees_every_call_a_library_makes() {
    let recording = Arc::new(Recording::default());
    let bus: Arc<dyn Bus> = recording.clone();
    declare_all(&*bus, POINTS);
    bus.listen::<DocClose>("doc.autosave", ListenOpts::new(), |_, _| Ok(()))
        .unwrap();
    bus.alter::<DocClose>("a".into(), vec![]);
    bus.collect::<Findings>(());
    assert_eq!(
        *recording.calls.lock().unwrap(),
        [
            "declare doc.close request",
            "declare doc.findings alter",
            "listen doc.close doc.autosave",
            "alter doc.close",
            "collect doc.findings",
        ]
    );
    // `&Arc<dyn Bus>` goes where `&dyn Bus` is asked.
    assert!(missing_targets(&bus, &["doc.close"]).is_empty());
}

/// A local listener runs in its place among the others on its thread;
/// from another thread the others run and it is queued, then runs in its
/// order when its thread calls `run_pending`.
#[test]
fn local_listeners_keep_their_place_and_run_on_their_thread() {
    let bus = bus();
    // State that is not `Send`: what a local listener is for.
    let seen: Rc<RefCell<Vec<Vec<String>>>> = Rc::default();
    bus.listen::<DocClose>("doc.first", ListenOpts::new(), |_, did| {
        did.push("first".into());
        Ok(())
    })
    .unwrap();
    let mine = seen.clone();
    bus.listen_local::<DocClose>(
        "ui.panel",
        ListenOpts::new().owner("ui").after("doc.first"),
        move |_, did| {
            mine.borrow_mut().push(did.clone());
            did.push("panel".into());
            Ok(())
        },
    )
    .unwrap();
    bus.listen::<DocClose>("doc.last", ListenOpts::new().after("ui.panel"), |_, did| {
        did.push("last".into());
        Ok(())
    })
    .unwrap();
    let woken = Arc::new(Mutex::new(Vec::new()));
    let w = woken.clone();
    bus.on_pending(move |thread| w.lock().unwrap().push(thread));

    // On its thread: in order, inline.
    let out = bus.alter::<DocClose>("a".into(), vec![]);
    assert_eq!(out.value, ["first", "panel", "last"]);
    assert!(out.deferred.is_empty());
    assert!(bus.run_pending().is_empty());

    // From another thread: the others run, the local one is queued.
    let other = bus.clone();
    let out = thread::spawn(move || other.alter::<DocClose>("b".into(), vec![]))
        .join()
        .unwrap();
    assert_eq!(out.value, ["first", "last"]);
    assert_eq!(out.deferred, ["ui.panel"]);
    assert_eq!(*woken.lock().unwrap(), [thread::current().id()]);
    assert_eq!(seen.borrow().len(), 1, "not run before run_pending");

    let ran = bus.run_pending();
    assert_eq!(
        ran,
        [{
            let mut run = PendingRun::new("doc.close");
            run.ran = vec!["ui.panel".into()];
            run
        }]
    );
    assert_eq!(
        seen.borrow().last().unwrap(),
        &["first", "last"],
        "it edits a copy of the value the emit returned"
    );
    assert!(bus.run_pending().is_empty(), "run once");

    // Introspection shows it, marked local.
    let infos = bus.listeners("doc.close").unwrap();
    assert!(infos[1].local && !infos[0].local);
    assert_eq!(
        order_dump(&*bus, &["doc.close"]),
        "doc.close\n  1. doc.first (owner -): nothing\n  2. ui.panel (owner ui): local; after doc.first\n  3. doc.last (owner -): after ui.panel\n"
    );
}

/// A collect from another thread queues the local part: each local
/// listener fills a fresh value when its thread runs it.
#[test]
fn a_deferred_collect_fills_fresh_values() {
    let bus = bus();
    let filled: Rc<RefCell<Vec<String>>> = Rc::default();
    let mine = filled.clone();
    bus.listen_local::<Findings>("ui.badge", ListenOpts::new(), move |_, out| {
        assert!(out.is_empty());
        out.push("badge".into());
        mine.borrow_mut().push("badge".into());
        Ok(())
    })
    .unwrap();
    bus.listen::<Findings>("disk", ListenOpts::new(), |_, out| {
        out.push("full".into());
        Ok(())
    })
    .unwrap();
    let other = bus.clone();
    let out = thread::spawn(move || other.collect::<Findings>(()))
        .join()
        .unwrap();
    assert_eq!(out.parts.len(), 1);
    assert_eq!(out.deferred, ["ui.badge"]);
    assert_eq!(bus.run_pending()[0].ran, ["ui.badge"]);
    assert_eq!(*filled.borrow(), ["badge"]);
}

/// `forget` removes local listeners like the others, queued ones
/// included; a local listener cannot ask for a thread of its own.
#[test]
fn forget_removes_local_listeners_queued_or_not() {
    let bus = bus();
    let runs = Rc::new(RefCell::new(0));
    let mine = runs.clone();
    bus.listen_local::<DocClose>("ui.panel", ListenOpts::new().owner("ui"), move |_, _| {
        *mine.borrow_mut() += 1;
        Ok(())
    })
    .unwrap();
    let other = bus.clone();
    let out = thread::spawn(move || other.alter::<DocClose>("a".into(), vec![]))
        .join()
        .unwrap();
    assert_eq!(out.deferred, ["ui.panel"]);
    assert_eq!(bus.forget("ui"), 1);
    assert_eq!(bus.run_pending()[0].ran, Vec::<String>::new());
    bus.alter::<DocClose>("b".into(), vec![]);
    assert_eq!(*runs.borrow(), 0);
    assert!(bus.listeners("doc.close").unwrap().is_empty());

    assert_eq!(
        bus.listen_local::<DocClose>("ui.slow", ListenOpts::new().threaded(), |_, _| Ok(())),
        Err(Refused::Threaded {
            point: "doc.close",
            listener: "ui.slow".into()
        })
    );
}

/// A command run, as a request whose listeners may refuse it.
struct CommandRun;
impl Point for CommandRun {
    const ID: PointId = PointId::new("command.run");
    const KIND: PointKind = PointKind::Request;
    type Input = String;
    type Value = Vec<String>;
}

/// The same request, declared fail-closed.
struct GuardedRun;
impl Point for GuardedRun {
    const ID: PointId = PointId::new("command.guarded");
    const KIND: PointKind = PointKind::Request;
    type Input = String;
    type Value = Vec<String>;
}

/// A request heard after the current pass.
struct LibraryWrite;
impl Point for LibraryWrite {
    const ID: PointId = PointId::new("library.write");
    const KIND: PointKind = PointKind::Request;
    type Input = String;
    type Value = Vec<String>;
}

const COMMAND_POINTS: &[PointDecl] = &[
    PointDecl::of::<CommandRun>("A command runs."),
    PointDecl::of::<GuardedRun>("A command runs, its policy fail-closed.").fail_closed(),
];

const LIBRARY_POINTS: &[PointDecl] =
    &[
        PointDecl::of::<LibraryWrite>("A library write, heard after drawing.")
            .needs(&[&bus_contract_defer::CONTRACT]),
    ];

/// Three listeners in order: `policy`, `audit`, `handler`; `policy`
/// answers with `policy`.
fn command_listeners<P: Point<Input = String, Value = Vec<String>>>(
    bus: &dyn Bus,
    policy: impl Fn(&str) -> Result<(), HookError> + Send + Sync + 'static,
) {
    bus.listen::<P>(
        "policy",
        ListenOpts::new().owner("policy"),
        move |cmd, did| {
            did.push("policy".into());
            policy(cmd)
        },
    )
    .unwrap();
    bus.listen::<P>("audit", ListenOpts::new(), |_, did| {
        did.push("audit".into());
        Ok(())
    })
    .unwrap();
    bus.listen::<P>("handler", ListenOpts::new(), |cmd, did| {
        did.push(format!("ran {cmd}"));
        Ok(())
    })
    .unwrap();
}

/// A listener refusing a request stops it: no later listener runs, its
/// own edit is dropped, the result names it, and it is no failure.
#[test]
fn a_refusal_stops_the_request() {
    let bus = bus();
    declare_all(&*bus, COMMAND_POINTS);
    command_listeners::<CommandRun>(&*bus, |cmd| {
        if cmd == "delete" {
            Err(HookError::refuse("agents may not delete"))
        } else {
            Ok(())
        }
    });
    let sunk = Arc::new(Mutex::new(0));
    let s = sunk.clone();
    bus.on_failure(move |_| *s.lock().unwrap() += 1);

    let out = bus.alter::<CommandRun>("open".into(), vec![]);
    assert_eq!(out.value, ["policy", "audit", "ran open"]);
    assert!(out.refused.is_none());

    let out = bus.alter::<CommandRun>("delete".into(), vec![]);
    assert_eq!(
        out.value,
        Vec::<String>::new(),
        "nothing ran past the refusal"
    );
    let refused = out.refused.unwrap();
    assert_eq!(refused.listener, "policy");
    assert_eq!(refused.owner.as_deref(), Some("policy"));
    assert_eq!(refused.reason, "agents may not delete");
    assert!(!refused.failed);
    assert_eq!(
        refused.to_string(),
        "hook `command.run`: refused by `policy`: agents may not delete"
    );
    assert!(out.failures.is_empty());
    assert_eq!(*sunk.lock().unwrap(), 0, "a refusal is not a failure");
}

/// Off a request, a refusal counts as a failure: dropped, the others run.
#[test]
fn a_refusal_off_a_request_is_a_failure() {
    let bus = bus();
    bus.listen::<Findings>("picky", ListenOpts::new(), |_, out| {
        out.push("picky".into());
        Err(HookError::refuse("no"))
    })
    .unwrap();
    bus.listen::<Findings>("disk", ListenOpts::new(), |_, out| {
        out.push("full".into());
        Ok(())
    })
    .unwrap();
    let out = bus.alter::<Findings>((), vec![]);
    assert_eq!(out.value, ["full"]);
    assert!(out.refused.is_none());
    assert_eq!(out.failures[0].kind, FailureKind::Error("no".into()));
}

/// A failing listener is dropped on an ordinary request and the handler
/// runs (fail-open); on a fail-closed one the failure refuses it.
#[test]
fn a_fail_closed_request_turns_a_failure_into_a_refusal() {
    let bus = bus();
    assert!(declare_all(&*bus, COMMAND_POINTS).is_empty());
    command_listeners::<CommandRun>(&*bus, |_| Err("policy table unreadable".into()));
    command_listeners::<GuardedRun>(&*bus, |_| Err("policy table unreadable".into()));

    let open = bus.alter::<CommandRun>("delete".into(), vec![]);
    assert_eq!(open.value, ["audit", "ran delete"], "fail-open");
    assert!(open.refused.is_none());
    assert_eq!(open.failures.len(), 1);

    let closed = bus.alter::<GuardedRun>("delete".into(), vec![]);
    assert_eq!(
        closed.value,
        Vec::<String>::new(),
        "the handler did not run"
    );
    assert_eq!(closed.failures.len(), 1, "the failure is still reported");
    let refused = closed.refused.unwrap();
    assert!(refused.failed);
    assert_eq!(refused.listener, "policy");
    assert_eq!(refused.reason, "policy table unreadable");
    assert_eq!(
        refused.to_string(),
        "hook `command.guarded`: refused, `policy` failed: policy table unreadable"
    );
    let late = Failure::new(
        "command.guarded",
        "policy",
        None,
        FailureKind::Late {
            deadline: std::time::Duration::from_millis(20),
            took: None,
        },
    );
    assert_eq!(
        Refusal::from_failure(&late).reason,
        "no answer within its deadline of 20ms"
    );

    let points = bus.points();
    let guarded = points.iter().find(|p| p.id == "command.guarded").unwrap();
    assert!(guarded.fail_closed);
    assert!(COMMAND_POINTS[1].spec().fail_closed);
}

/// A refused request queues nothing for another thread's local
/// listeners; a refusal among queued local listeners stops the rest.
#[test]
fn a_refusal_stops_local_listeners_too() {
    let bus = bus();
    declare_all(&*bus, COMMAND_POINTS);
    bus.listen_local::<CommandRun>("ui.confirm", ListenOpts::new(), |cmd, did| {
        if cmd == "delete" {
            return Err(HookError::refuse("the user said no"));
        }
        did.push("confirmed".into());
        Ok(())
    })
    .unwrap();
    bus.listen_local::<CommandRun>("ui.after", ListenOpts::new(), |_, did| {
        did.push("after".into());
        Ok(())
    })
    .unwrap();
    bus.listen::<CommandRun>("gate", ListenOpts::new(), |cmd, _| {
        if cmd == "format" {
            Err(HookError::refuse("never"))
        } else {
            Ok(())
        }
    })
    .unwrap();

    let other = bus.clone();
    let out = thread::spawn(move || other.alter::<CommandRun>("format".into(), vec![]))
        .join()
        .unwrap();
    assert_eq!(out.refused.unwrap().listener, "gate");
    assert!(out.deferred.is_empty(), "a refused request queues nothing");
    assert!(bus.run_pending().is_empty());

    let other = bus.clone();
    let out = thread::spawn(move || other.alter::<CommandRun>("delete".into(), vec![]))
        .join()
        .unwrap();
    assert_eq!(out.deferred, ["ui.confirm", "ui.after"]);
    let ran = bus.run_pending();
    assert_eq!(ran[0].ran, ["ui.confirm"]);
    assert_eq!(ran[0].refused.as_ref().unwrap().reason, "the user said no");
    assert_eq!(ran[0].ask, None);
}

/// A bus without `Defer` says so; a table needing it is refused clearly
/// and its point left undeclared.
#[test]
fn a_point_needing_a_missing_capability_is_refused() {
    let bus = bus();
    assert!(bus.defer().is_none());
    assert!(!offers(&*bus, &bus_contract_defer::CONTRACT));
    let refused = declare_all(&*bus, LIBRARY_POINTS);
    assert_eq!(
        refused,
        [Refused::Unsupported {
            point: "library.write",
            contract: "defer"
        }]
    );
    assert_eq!(
        refused[0].to_string(),
        "hook `library.write` needs the `defer` contract, which this bus does not offer"
    );
    assert!(bus.points().is_empty(), "not declared");

    let bus: Arc<dyn Bus> = Arc::new(Recording::with_defer());
    assert!(offers(&*bus, &bus_contract_defer::CONTRACT));
    assert!(declare_all(&*bus, LIBRARY_POINTS).is_empty());
    // Through `Arc<B>` too.
    let shared = Arc::new(Recording::with_defer());
    assert!(BusDefer::defer(&shared).is_some());
}

/// Asks are queued, run at the drain in ask order with every listener,
/// refusals and failures reported; an ask made while draining waits for
/// the next drain; an immediate emit after an ask runs first.
#[test]
fn deferred_requests_run_at_the_drain_in_ask_order() {
    let recording = Arc::new(Recording::with_defer());
    let bus: Arc<dyn Bus> = recording.clone();
    declare_all(&*bus, LIBRARY_POINTS);
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let h = heard.clone();
    let again = bus.clone();
    bus.listen::<LibraryWrite>("persist", ListenOpts::new(), move |what, did| {
        if what == "locked" {
            return Err(HookError::refuse("the library is read-only"));
        }
        if what == "broken" {
            return Err("disk full".into());
        }
        h.lock().unwrap().push(what.clone());
        did.push(format!("wrote {what}"));
        if what == "rename" {
            // Heard at the next drain, not this one.
            again
                .defer()
                .unwrap()
                .ask::<LibraryWrite>("reindex".into(), vec![]);
        }
        Ok(())
    })
    .unwrap();
    let woken = Arc::new(Mutex::new(0));
    let w = woken.clone();
    bus.on_pending(move |_| *w.lock().unwrap() += 1);

    let defer = bus.defer().unwrap();
    let first = defer.ask::<LibraryWrite>("rename".into(), vec![]);
    let second = defer.ask::<LibraryWrite>("locked".into(), vec![]);
    defer.ask::<LibraryWrite>("broken".into(), vec![]);
    assert!(second > first);
    assert_eq!(*woken.lock().unwrap(), 3);
    assert!(
        heard.lock().unwrap().is_empty(),
        "nothing heard before the drain"
    );
    assert_eq!(defer.waiting(), 3);

    // An immediate emit made after the asks runs first.
    bus.alter::<LibraryWrite>("now".into(), vec![]);
    assert_eq!(*heard.lock().unwrap(), ["now"]);

    let ran = defer.drain();
    assert_eq!(ran.len(), 3);
    assert_eq!(ran[0].ask, Some(first));
    assert_eq!(ran[0].ran, ["persist"]);
    assert!(ran[0].refused.is_none() && ran[0].failures.is_empty());
    assert_eq!(ran[1].ask, Some(second));
    assert_eq!(
        ran[1].refused.as_ref().unwrap().reason,
        "the library is read-only"
    );
    assert_eq!(
        ran[2].failures[0].kind,
        FailureKind::Error("disk full".into())
    );
    assert_eq!(*heard.lock().unwrap(), ["now", "rename"]);

    // The ask made while draining waited for this drain.
    assert_eq!(defer.waiting(), 1);
    let ran = defer.drain();
    assert_eq!(ran.len(), 1);
    assert_eq!(*heard.lock().unwrap(), ["now", "rename", "reindex"]);
    assert!(defer.drain().is_empty());
    assert_eq!(defer.waiting(), 0);
}

/// Asks are per thread: another thread's asks wait for its own drain.
#[test]
fn deferred_requests_belong_to_the_asking_thread() {
    let bus: Arc<dyn Bus> = Arc::new(Recording::with_defer());
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let h = heard.clone();
    bus.listen::<LibraryWrite>("persist", ListenOpts::new(), move |what, _| {
        h.lock().unwrap().push(what.clone());
        Ok(())
    })
    .unwrap();
    let other = bus.clone();
    let (drained_here, ran_there) = thread::spawn(move || {
        let defer = other.defer().unwrap();
        defer.ask::<LibraryWrite>("there".into(), vec![]);
        (defer.waiting(), defer.drain().len())
    })
    .join()
    .unwrap();
    assert_eq!((drained_here, ran_there), (1, 1));
    bus.defer()
        .unwrap()
        .ask::<LibraryWrite>("here".into(), vec![]);
    assert_eq!(*heard.lock().unwrap(), ["there"]);
    assert_eq!(bus.defer().unwrap().drain().len(), 1);
    assert_eq!(*heard.lock().unwrap(), ["there", "here"]);
}

/// A budget in a contract table reaches the spec the bus is given; it
/// needs no capability, so a bus without `Timings` declares the point.
#[test]
fn a_budget_reaches_the_spec_and_needs_no_capability() {
    const TABLE: &[PointDecl] =
        &[PointDecl::of::<DocClose>("A document closes.")
            .budget(std::time::Duration::from_millis(4))];
    assert_eq!(TABLE[0].budget, Some(std::time::Duration::from_millis(4)));
    assert_eq!(
        TABLE[0].spec().budget,
        Some(std::time::Duration::from_millis(4))
    );
    assert_eq!(
        PointSpec::new("x")
            .budget(std::time::Duration::from_millis(2))
            .budget,
        Some(std::time::Duration::from_millis(2))
    );

    let recording = Arc::new(Recording::default());
    let bus: Arc<dyn Bus> = recording.clone();
    assert!(bus.timings().is_none());
    assert!(!offers(&*bus, &bus_contract_timings::CONTRACT));
    assert!(
        BusTimings::timings(&recording).is_none(),
        "through `Arc<B>`"
    );
    assert!(declare_all(&*bus, TABLE).is_empty());
    let points = recording.points.lock().unwrap();
    let spec = points[0].1.as_ref().unwrap();
    assert_eq!(spec.budget, Some(std::time::Duration::from_millis(4)));
    assert_eq!(bus_contract_timings::CONTRACT.name, "timings");
}

/// The timing types: the mean, the slowest listeners first, and a pass
/// over budget naming its slowest listener.
#[test]
fn timing_types_read_the_slowest_first() {
    use std::time::Duration;
    let mut stats = TimingStats::new();
    assert_eq!(stats.mean(), Duration::ZERO);
    stats.count = 4;
    stats.total = Duration::from_millis(10);
    assert_eq!(stats.mean(), Duration::from_micros(2500));

    let mut point = PointTimings::new("image.opened");
    for (id, p95, max) in [("a", 1, 9), ("b", 3, 3), ("c", 1, 2)] {
        let mut l = ListenerTimings::new(id);
        l.stats.p95 = Duration::from_millis(p95);
        l.stats.max = Duration::from_millis(max);
        point.listeners.push(l);
    }
    let order: Vec<&str> = point.slowest().iter().map(|l| l.id.as_str()).collect();
    assert_eq!(order, ["b", "a", "c"]);

    let mut over = OverBudget::new(
        "image.opened",
        Duration::from_millis(7),
        Duration::from_millis(4),
    );
    assert_eq!(over.slowest(), None);
    over.listeners = vec![
        ("marks".into(), Duration::from_millis(1)),
        ("snapshots".into(), Duration::from_millis(6)),
    ];
    assert_eq!(
        over.slowest(),
        Some(("snapshots", Duration::from_millis(6)))
    );
    assert_eq!(
        over.to_string(),
        "hook `image.opened`: a pass took 7ms, over its budget of 4ms (slowest `snapshots`: 6ms)"
    );
}

/// `on_over_budget` on `dyn Timings` goes through `add_budget_sink`.
#[test]
fn on_over_budget_adds_a_sink() {
    #[derive(Default)]
    struct Sinks(Mutex<Vec<bus_contract_timings::BudgetSink>>);
    impl Timings for Sinks {
        fn points(&self) -> Vec<PointTimings> {
            Vec::new()
        }
        fn point(&self, _: &str) -> Option<PointTimings> {
            None
        }
        fn reset(&self) {}
        fn add_budget_sink(&self, sink: bus_contract_timings::BudgetSink) {
            self.0.lock().unwrap().push(sink);
        }
    }
    let sinks = Sinks::default();
    let heard = Arc::new(AtomicU64::new(0));
    let h = heard.clone();
    (&sinks as &dyn Timings).on_over_budget(move |o| {
        h.fetch_add(o.took.as_millis() as u64, Ordering::Relaxed);
    });
    let over = OverBudget::new(
        "p",
        std::time::Duration::from_millis(5),
        std::time::Duration::from_millis(1),
    );
    for sink in sinks.0.lock().unwrap().iter() {
        sink(&over);
    }
    assert_eq!(heard.load(Ordering::Relaxed), 5);
}
