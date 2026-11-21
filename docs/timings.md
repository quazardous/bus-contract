# Timings

`bus.timings()` (`use bus_contract_timings::BusTimings;`) returns
`Option<&dyn Timings>`: how long listeners take, per point and per
listener (`TimingStats`: count, total, max, p50, p95), since the last
`reset()`.

- A pass is what one emit's listeners ran on one thread (an inline
  emit, a `run_pending` run, a drained ask); its time is its listeners'
  times added up.
- A point may declare a budget for a pass (`PointDecl::budget`,
  `PointSpec::budget`). A pass over it goes to the budget sinks
  (`on_over_budget`, `OverBudget` with each listener's time), on the
  thread that ran it. Nothing fails: a budget is not a deadline. If a
  slow listener must count as a failure, give it a deadline
  (`ListenOpts::deadline`), not a budget. A host publishes it as the `bus.overBudget` fact (`OVER_BUDGET_FACT`)
  or logs.
- A budget needs no capability: on a bus without `Timings` it is only
  shown in introspection (`PointInfo::budget`).

```rust,ignore
pub const POINTS: &[PointDecl] = &[PointDecl::of::<ImageOpened>("An image was opened.")
    .budget(Duration::from_millis(4))];

if let Some(timings) = bus.timings() {
    timings.on_over_budget(|over| log::warn!("{over}"));
    for point in timings.points() {
        for l in point.slowest().iter().take(3) {
            println!("{} {} p95 {:?}", point.point, l.id, l.stats.p95);
        }
    }
}
```
