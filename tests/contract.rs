//! The typed calls on `dyn Bus`, the contract table and the tools, over
//! a recording in-memory implementation written against the contract
//! alone: the listeners in registration order, no deadlines.

use std::sync::{Arc, Mutex};

use bus_contract::{
    Altered, AnyInput, AnyListener, AnyValue, Bus, CloneFn, Collected, Failure, FailureKind,
    FailureSink, FreshFn, ListenOpts, ListenerInfo, Point, PointDecl, PointInfo, PointKey,
    PointKind, PointSpec, Refused, declare_all, missing_targets, order_dump,
};

/// Listeners in registration order, no deadlines: enough to show the
/// contract works with any implementation.
#[derive(Default)]
struct Recording {
    /// Every call, in order: `declare doc.close`, `listen doc.close x`,
    /// `alter doc.close`.
    calls: Mutex<Vec<String>>,
    points: Mutex<Vec<(PointKey, Option<PointSpec>)>>,
    listeners: Mutex<Vec<(&'static str, String, ListenOpts, AnyListener)>>,
    sinks: Mutex<Vec<FailureSink>>,
}

impl Recording {
    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    fn run(&self, point: &'static str) -> Vec<(String, AnyListener)> {
        self.listeners
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.0 == point)
            .map(|l| (l.1.clone(), l.3.clone()))
            .collect()
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
        for (id, run) in self.run(point.id) {
            let mut v = clone(&*value);
            match run(&*input, &mut *v) {
                Ok(()) => value = v,
                Err(e) => failures.push(self.fail(point.id, &id, e.0)),
            }
        }
        Altered { value, failures }
    }

    fn collect_any(&self, point: PointKey, input: AnyInput, fresh: FreshFn) -> Collected<AnyValue> {
        self.record(format!("collect {}", point.id));
        let mut parts = Vec::new();
        let mut failures = Vec::new();
        for (id, run) in self.run(point.id) {
            let mut v = fresh();
            match run(&*input, &mut *v) {
                Ok(()) => parts.push((id, v)),
                Err(e) => failures.push(self.fail(point.id, &id, e.0)),
            }
        }
        Collected { parts, failures }
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
