# bus-contract

[<img alt="github" src="https://img.shields.io/badge/github-quazardous/bus--contract-8da0cb?logo=github" height="20">](https://github.com/quazardous/bus-contract)
[![Latest version](https://img.shields.io/crates/v/bus-contract.svg)](https://crates.io/crates/bus-contract)
[![Documentation](https://docs.rs/bus-contract/badge.svg)](https://docs.rs/bus-contract)
[![Build Status](https://github.com/quazardous/bus-contract/actions/workflows/ci.yml/badge.svg)](https://github.com/quazardous/bus-contract/actions/workflows/ci.yml)
[![MSRV 1.95](https://img.shields.io/badge/MSRV-1.95-orange.svg)](https://github.com/quazardous/bus-contract/blob/main/Cargo.toml)
[![no unsafe](https://img.shields.io/badge/unsafe-forbidden-success.svg)](https://github.com/rust-secure-code/safety-dance/)
[![no dependencies](https://img.shields.io/badge/dependencies-none-success.svg)](https://github.com/quazardous/bus-contract/blob/main/Cargo.toml)
[![MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/quazardous/bus-contract/blob/main/LICENSE-MIT)
[![Apache](https://img.shields.io/badge/license-Apache-blue.svg)](https://github.com/quazardous/bus-contract/blob/main/LICENSE-APACHE)

Traits and types for an in-process bus of hook points (requests,
events, alter, collect) with ordered listeners.

A library declares its points and listeners against the `Bus` trait.
The application creates the bus and passes it to each library at
construction. There is no global.

## Contents

- **Points are types** (`Point`): an id (`doc.close`), the input every
  listener reads, the value a listener edits, and a declared kind
  (`request`, `event`, `alter`, `collect`).
- **`Bus`**, an object-safe trait (`Arc<dyn Bus>`): declare a point;
  listen under an id and an owner, ordered by `after` / `before` /
  `last`, then priority; unlisten; forget an owner; `alter` and
  `collect` emits that return every listener's failure (error, panic,
  missed deadline); failure sinks; introspection.
- **Local listeners** (`listen_local::<P>`) for state that is not
  `Send` (`Rc<RefCell<…>>`): bound to the thread that registered them,
  in the same order as the other listeners. An emit from another
  thread queues them for their thread's `run_pending()`.
- **Typed calls on `dyn Bus`** (`declare::<P>`, `listen::<P>`,
  `listen_local::<P>`, `alter::<P>`, `collect::<P>`, `on_failure`,
  `on_pending`), written once over the erased methods: an
  implementation writes only those.
- **A contract table** (`PointDecl`): a library's points as a `const`,
  readable before anything is built, declared with `declare_all`.
- **Tools on the contract**: `order_dump` ("who runs when", for reviewed
  files and debug views) and `missing_targets` (an ordering constraint
  naming a listener that is not there).
- No dependencies, no `unsafe`, no async runtime.

This crate has no implementation. One is `Kernel` in
[`egui-pop-hooks`](https://github.com/quazardous/egui-pop). The
guarantees an implementation must keep are documented on `Bus`.

## Example

```rust
use std::sync::Arc;
use bus_contract::{Bus, ListenOpts, Point, PointDecl, PointKind, declare_all};

/// The library's request: a document closes.
pub struct DocClose;
impl Point for DocClose {
    const ID: &'static str = "doc.close";
    const KIND: PointKind = PointKind::Request;
    type Input = String;
    type Value = Vec<String>;
}

/// The library's points, readable before anything is built.
pub const POINTS: &[PointDecl] = &[PointDecl::of::<DocClose>("A document closes.")];

/// The library takes the bus; it never constructs one.
pub struct Documents(Arc<dyn Bus>);

impl Documents {
    pub fn new(bus: Arc<dyn Bus>) -> Documents {
        declare_all(&*bus, POINTS);
        let opts = ListenOpts::new().owner("docs");
        let _ = bus.listen::<DocClose>("docs.autosave", opts, |doc, did| {
            did.push(format!("saved {doc}"));
            Ok(())
        });
        Documents(bus)
    }

    pub fn close(&self, doc: &str) -> Vec<String> {
        self.0.alter::<DocClose>(doc.to_owned(), Vec::new()).value
    }
}
```

The application creates the bus and passes it to each library:

```rust,ignore
let bus: Arc<dyn Bus> = Arc::new(egui_pop_hooks::Kernel::new()); // or its own
let docs = Documents::new(bus.clone());
let search = Search::new(bus.clone());
print!("{}", bus_contract::order_dump(&*bus, &["doc.close"]));
```

## Local listeners

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
  once per pass of its loop.
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

## Not included

- A journal (events with a revision, catch-up from a revision).
- Transport across processes.

## License

MIT OR Apache-2.0, at your option.
