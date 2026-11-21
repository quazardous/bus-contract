# Defer

`bus.defer()` (`use bus_contract_defer::BusDefer;`) returns
`Option<&dyn Defer>`. A request asked through it returns at once; its
listeners run at the next `drain()` on the same thread. It is for code
that cannot be re-entered while it runs, such as an interface drawing a
panel with its state borrowed.

- Asks are per thread and run in ask order, each as an alter emit with
  the listeners registered at the drain. Refusals and fail-closed
  points apply.
- `drain()` returns what each ask did (`PendingRun` with `ask` set: the
  listeners that ran, failures, refusal). The asker gets nothing back:
  if you need the outcome, read it from the drain.
- An ask made during a drain waits for the next drain. An immediate
  emit made after an ask runs before it.
- `waiting()` counts the asks queued for the calling thread, so a host
  can drain before it exits.

```rust,ignore
pub const POINTS: &[PointDecl] = &[PointDecl::of::<LibraryWrite>("A library write.")
    .needs(&[&bus_contract_defer::CONTRACT])];

// while drawing, with the library borrowed:
bus.defer().expect("declared with needs").ask::<LibraryWrite>(write, Vec::new());
// after drawing, where nothing is borrowed:
for run in bus.defer().map(|d| d.drain()).unwrap_or_default() {
    if let Some(refusal) = run.refused { log::warn!("{refusal}"); }
}
```
