# Local listeners

A listener registered with `listen_local::<P>` may hold state that is
not `Send`, such as an interface's `Rc<RefCell<…>>`. It is bound to the
thread that registered it.

- It is ordered with the other listeners of its point (`after`,
  `before`, `last`, priority), keeps its owner and deadline, and shows
  in introspection as local.
- An emit on its thread runs it in that order with the others.
- An emit on another thread runs the other listeners, queues the local
  ones and names them in the result (`deferred`). The wakers added with
  `on_pending` are called with the thread's id. The thread runs the
  queued listeners, in order, when it calls `run_pending()`, typically
  once per pass of its loop. If your thread never calls
  `run_pending()`, its local listeners never hear emits made from other
  threads.
- `unlisten` and `forget` remove local listeners too, queued runs
  included.

```rust,ignore
let panel = Rc::new(RefCell::new(Panel::default()));
let mine = panel.clone();
bus.listen_local::<DocClose>("ui.panel", ListenOpts::new().after("docs.autosave"), move |doc, _| {
    mine.borrow_mut().closed(doc);
    Ok(())
})?;
bus.on_pending(move |_| ctx.request_repaint());
// each pass of the interface's loop:
bus.run_pending();
```
