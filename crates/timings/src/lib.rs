#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

#[allow(unused_imports)] // named by the docs
use bus_contract::PointSpec;
use bus_contract::{Bus, Contract, capability};

/// The timings contract: a bus offers it by answering for
/// `dyn Timings` in `Bus::capability`. A point's budget
/// (`PointSpec::budget`) needs no contract: on a bus without this one it
/// is only shown.
pub const CONTRACT: Contract = Contract::new::<dyn Timings>("timings", "0.2", &[]);

/// The name a host gives the fact it publishes for each [`OverBudget`]
/// its budget sinks receive
/// ([`Timings::add_budget_sink`]).
pub const OVER_BUDGET_FACT: &str = "bus.overBudget";

/// Receives each pass over a point's budget ([`OverBudget`]).
pub type BudgetSink = Arc<dyn Fn(&OverBudget) + Send + Sync>;

/// A capability of a bus ([`BusTimings::timings`]): how long listeners take,
/// per point and per listener, and the passes over a point's budget
/// ([`PointSpec::budget`]) reported as they happen.
///
/// A pass is what the listeners of one emit ran on one thread: the
/// inline part of an alter or collect emit, one emit's local listeners
/// run by [`Bus::run_pending`], or one ask run by the defer contract's drain. Its
/// time is its listeners' times added up.
///
/// What an implementation promises:
///
/// - Every listener run it makes is timed, whatever its outcome (a
///   threaded listener that misses its deadline counts the time the
///   emit waited), and counted in the listener's and the pass's
///   [`TimingStats`] until [`Timings::reset`].
/// - A pass of a point with a budget that takes longer is reported to
///   the budget sinks, on the thread that ran it, after the pass, with
///   each listener's time ([`OverBudget`]). The pass's results are kept:
///   a budget is not a deadline, and a pass over it is not a failure.
/// - Quantiles may be approximate; counts, totals and maxima are exact.
/// - Timing costs little next to a listener: clock reads and counters,
///   no allocation per run.
pub trait Timings: Send + Sync {
    /// Every point that ran a pass since the last reset, in id order.
    fn points(&self) -> Vec<PointTimings>;

    /// The timings of the point `point`; `None` when it ran no pass
    /// since the last reset.
    fn point(&self, point: &str) -> Option<PointTimings>;

    /// Starts a new window: every count back to zero (after a start-up
    /// phase, before a measured run).
    fn reset(&self);

    /// Adds a sink called with each pass over its point's budget, on the
    /// thread that ran it: where the host publishes its
    /// [`OVER_BUDGET_FACT`] fact or logs. A sink that panics is ignored.
    fn add_budget_sink(&self, sink: BudgetSink);
}

impl<'a> dyn Timings + 'a {
    /// Adds a sink called with each pass over its point's budget
    /// ([`Timings::add_budget_sink`]).
    pub fn on_over_budget(&self, sink: impl Fn(&OverBudget) + Send + Sync + 'static) {
        self.add_budget_sink(Arc::new(sink));
    }
}

/// How long something took over a window of runs
/// ([`Timings`]): a listener's runs, or a point's passes.
/// The quantiles may be approximate (an implementation may read them
/// from a histogram); the count, total and max are exact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TimingStats {
    /// How many runs.
    pub count: u64,
    /// Their time added up.
    pub total: Duration,
    /// The longest.
    pub max: Duration,
    /// The median.
    pub p50: Duration,
    /// The 95th percentile.
    pub p95: Duration,
}

impl TimingStats {
    /// No run.
    pub fn new() -> TimingStats {
        TimingStats::default()
    }

    /// The mean time of a run; zero with no run.
    pub fn mean(&self) -> Duration {
        match u32::try_from(self.count) {
            Ok(0) => Duration::ZERO,
            Ok(n) => self.total / n,
            Err(_) => Duration::from_secs_f64(self.total.as_secs_f64() / self.count as f64),
        }
    }
}

/// One listener's time on a point ([`PointTimings::listeners`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListenerTimings {
    /// Its id.
    pub id: String,
    /// Its owner.
    pub owner: Option<String>,
    /// Its runs.
    pub stats: TimingStats,
}

impl ListenerTimings {
    /// The listener `id`, no run.
    pub fn new(id: impl Into<String>) -> ListenerTimings {
        ListenerTimings {
            id: id.into(),
            owner: None,
            stats: TimingStats::new(),
        }
    }
}

/// A point's time ([`Timings::points`]): its
/// passes, its listeners, its budget and how often a pass went over.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PointTimings {
    /// The point.
    pub point: &'static str,
    /// Its budget ([`PointSpec::budget`]).
    pub budget: Option<Duration>,
    /// Its passes: each the time of the listeners one emit, queued run
    /// or drained ask ran on one thread.
    pub passes: TimingStats,
    /// How many passes went over the budget.
    pub over_budget: u64,
    /// Its listeners that ran, in the order they run.
    pub listeners: Vec<ListenerTimings>,
}

impl PointTimings {
    /// The point `point`, no budget, no run.
    pub fn new(point: &'static str) -> PointTimings {
        PointTimings {
            point,
            budget: None,
            passes: TimingStats::new(),
            over_budget: 0,
            listeners: Vec::new(),
        }
    }

    /// Its listeners, the slowest first by their 95th percentile, then
    /// by their max.
    pub fn slowest(&self) -> Vec<&ListenerTimings> {
        let mut out: Vec<&ListenerTimings> = self.listeners.iter().collect();
        out.sort_by(|a, b| {
            (b.stats.p95, b.stats.max)
                .cmp(&(a.stats.p95, a.stats.max))
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }
}

/// A pass of a point's listeners that went over its budget
/// ([`PointSpec::budget`]): reported to the budget sinks
/// ([`Timings::add_budget_sink`]). The
/// pass's results were kept: a budget is not a deadline.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OverBudget {
    /// The point.
    pub point: &'static str,
    /// The pass's time: its listeners' times added up.
    pub took: Duration,
    /// The point's budget.
    pub budget: Duration,
    /// Each listener that ran in the pass, in order, with its time.
    pub listeners: Vec<(String, Duration)>,
}

impl OverBudget {
    /// A pass of `point` that took `took` against `budget`.
    pub fn new(point: &'static str, took: Duration, budget: Duration) -> OverBudget {
        OverBudget {
            point,
            took,
            budget,
            listeners: Vec::new(),
        }
    }

    /// The listener that took longest in the pass, with its time.
    pub fn slowest(&self) -> Option<(&str, Duration)> {
        self.listeners
            .iter()
            .max_by_key(|(_, t)| *t)
            .map(|(id, t)| (id.as_str(), *t))
    }
}

impl fmt::Display for OverBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "hook `{}`: a pass took {:?}, over its budget of {:?}",
            self.point, self.took, self.budget
        )?;
        if let Some((id, t)) = self.slowest() {
            write!(f, " (slowest `{id}`: {t:?})")?;
        }
        Ok(())
    }
}

/// Reaches the [`Timings`] capability of any bus: `bus.timings()`.
///
/// Implemented for every [`Bus`], sized or not (`dyn Bus`, `Arc<dyn
/// Bus>`, a concrete bus): what the bus answers for `dyn Timings`
/// through the typed lookup `Bus::capability`.
pub trait BusTimings {
    /// The [`Timings`] capability of this bus; `None` when the bus does
    /// not offer it.
    fn timings(&self) -> Option<&dyn Timings>;
}

impl<B: Bus + ?Sized> BusTimings for B {
    fn timings(&self) -> Option<&dyn Timings> {
        capability::<dyn Timings, B>(self)
    }
}
