# Changelog

## 0.2.0

- Local listeners: `Bus::listen_local_any` and the typed
  `listen_local::<P>` register a listener that is not `Send`, bound to
  the thread that registered it and ordered with the other listeners.
  An emit from another thread queues it for that thread's
  `Bus::run_pending`; `Bus::add_pending_waker` (typed: `on_pending`)
  is called when work is queued.
- `Altered` and `Collected` gain `deferred`: the local listeners an
  emit queued for another thread.
- `PendingRun`: what one emit's queued listeners did.
- `ListenerInfo` gains `local`; `order_dump` marks local listeners.
- `Refused::Threaded`: a local listener asked for a thread of its own.
- `erase_local`, `AnyLocalListener`, `PendingWaker`.
- Breaking: `Bus` has three new required methods; `Altered`,
  `Collected`, `ListenerInfo` and `Refused` have new fields or variants.

## 0.1.0

- First release: typed hook points, ordered listeners with owners and
  deadlines, alter and collect emits, failure sinks, introspection, the
  contract table and the tools on it.
