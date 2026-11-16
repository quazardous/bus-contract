//! The typed calls on `dyn Bus`, the contract table and the tools, over
//! a recording in-memory implementation written against the contract
//! alone: the listeners in registration order, no deadlines; local
//! listeners kept in thread-local storage, the bus holding only where
//! they are.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use bus_contract::{
    Altered, AnyInput, AnyListener, AnyLocalListener, AnyValue, Bus, CloneFn, Collected, Failure,
    FailureKind, FailureSink, FreshFn, HookError, ListenOpts, ListenerInfo, PendingRun,
    PendingWaker, Point, PointDecl, PointInfo, PointKey, PointKind, PointSpec, Refused,
    declare_all, missing_targets, order_dump,
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
    point: &'static str,
    input: AnyInput,
    work: Work,
    ids: Vec<String>,
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
}

impl Recording {
    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    fn run(&self, point: &'static str) -> Vec<(String, Run)> {
        self.listeners
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.0 == point)
            .map(|l| (l.1.clone(), l.3.clone()))
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

    /// Queues the deferred listeners, one emit per thread, and wakes.
    fn defer(
        &self,
        point: &'static str,
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
            for wake in self.wakers.lock().unwrap().iter() {
                wake(thread);
            }
        }
    }

    fn fail(&self, point: &'static str, listener: &str, message: String) -> Failure {
        let failure = Failure {
            point,
            listener: listener.to_owned(),
            owner: None,
            kind: FailureKind::Error(message),
        };
        for sink in self.sinks.lock().unwrap().iter() {
            sink(&failure);
        }
        failure
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
                let mut ran = Vec::new();
                let mut failures = Vec::new();
                let mut value = match &q.work {
                    Work::Alter(v, clone) => Some(clone(&**v)),
                    Work::Collect(_) => None,
                };
                // Still registered now: one removed since the emit is skipped.
                let now = self.run(q.point);
                for id in &q.ids {
                    let Some((_, run)) = now.iter().find(|(i, _)| i == id) else {
                        continue;
                    };
                    let mut v = match (&q.work, &value) {
                        (Work::Alter(_, clone), Some(v)) => clone(&**v),
                        (Work::Collect(fresh), _) => fresh(),
                        (Work::Alter(..), None) => unreachable!(),
                    };
                    match run.call(&*q.input, &mut *v) {
                        Some(Ok(())) => {
                            ran.push(id.clone());
                            if value.is_some() {
                                value = Some(v);
                            }
                        }
                        Some(Err(e)) => {
                            ran.push(id.clone());
                            failures.push(self.fail(q.point, id, e.0));
                        }
                        None => {}
                    }
                }
                PendingRun {
                    point: q.point,
                    ran,
                    failures,
                }
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
        mut value: AnyValue,
        clone: CloneFn,
    ) -> Altered<AnyValue> {
        self.record(format!("alter {}", point.id));
        let mut failures = Vec::new();
        let mut deferred = Vec::new();
        for (id, run) in self.run(point.id) {
            let mut v = clone(&*value);
            match run.call(&*input, &mut *v) {
                Some(Ok(())) => value = v,
                Some(Err(e)) => failures.push(self.fail(point.id, &id, e.0)),
                None => deferred.push((id, run.thread().unwrap())),
            }
        }
        self.defer(point.id, &input, &deferred, || {
            Work::Alter(clone(&*value), clone)
        });
        Altered {
            value,
            failures,
            deferred: deferred.into_iter().map(|d| d.0).collect(),
        }
    }

    fn collect_any(&self, point: PointKey, input: AnyInput, fresh: FreshFn) -> Collected<AnyValue> {
        self.record(format!("collect {}", point.id));
        let mut parts = Vec::new();
        let mut failures = Vec::new();
        let mut deferred = Vec::new();
        for (id, run) in self.run(point.id) {
            let mut v = fresh();
            match run.call(&*input, &mut *v) {
                Some(Ok(())) => parts.push((id, v)),
                Some(Err(e)) => failures.push(self.fail(point.id, &id, e.0)),
                None => deferred.push((id, run.thread().unwrap())),
            }
        }
        self.defer(point.id, &input, &deferred, || Work::Collect(fresh));
        Collected {
            parts,
            failures,
            deferred: deferred.into_iter().map(|d| d.0).collect(),
        }
    }

    fn add_failure_sink(&self, sink: FailureSink) {
        self.sinks.lock().unwrap().push(sink);
    }

    fn points(&self) -> Vec<PointInfo> {
        self.points
            .lock()
            .unwrap()
            .iter()
            .map(|(k, s)| PointInfo {
                id: k.id,
                kind: k.kind,
                description: s
                    .as_ref()
                    .map(|s| s.description.clone())
                    .unwrap_or_default(),
                declared: s.is_some(),
                deadline: bus_contract::DEFAULT_DEADLINE,
                listeners: self.listeners(k.id).unwrap_or_default(),
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
                .map(|l| ListenerInfo {
                    id: l.1.clone(),
                    owner: l.2.owner.clone(),
                    after: l.2.after.clone(),
                    before: l.2.before.clone(),
                    priority: l.2.priority,
                    deadline: bus_contract::DEFAULT_DEADLINE,
                    threaded: l.2.threaded,
                    local: matches!(l.3, Run::Local(..)),
                    missing: l
                        .2
                        .after
                        .iter()
                        .chain(&l.2.before)
                        .filter(|t| *t != bus_contract::LAST && !ids.contains(&t.as_str()))
                        .cloned()
                        .collect(),
                    runs: 0,
                    failures: 0,
                    last_failure: None,
                })
                .collect(),
        )
    }
}

struct DocClose;
impl Point for DocClose {
    const ID: &'static str = "doc.close";
    const KIND: PointKind = PointKind::Request;
    type Input = String;
    type Value = Vec<String>;
}

struct Findings;
impl Point for Findings {
    const ID: &'static str = "doc.findings";
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
        [PendingRun {
            point: "doc.close",
            ran: vec!["ui.panel".into()],
            failures: vec![],
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
