#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use bus_contract::Bus;

/// The items of the timings contract. In this release they are the
/// `bus-contract` core's own items, re-exported: the same types, so code
/// importing them from here and code importing them from the core work
/// together.
pub use bus_contract::{
    BudgetSink, ListenerTimings, OVER_BUDGET_FACT, OverBudget, PointTimings, TimingStats, Timings,
};

/// Reaches the [`Timings`] capability of any bus: `bus.timings()`.
///
/// Implemented for every [`Bus`], sized or not (`dyn Bus`, `Arc<dyn
/// Bus>`, a concrete bus). It answers what the bus answers for
/// `dyn Timings`: in this release, through the core's `Bus::timings` getter,
/// which defaults to the typed lookup `Bus::capability`. From
/// `bus-contract` 0.5 the getter leaves the core and this trait is the
/// way to reach the capability, through the lookup only.
pub trait BusTimings {
    /// The [`Timings`] capability of this bus; `None` when the bus does not
    /// offer it.
    fn timings(&self) -> Option<&dyn Timings>;
}

impl<B: Bus + ?Sized> BusTimings for B {
    fn timings(&self) -> Option<&dyn Timings> {
        Bus::timings(self)
    }
}
