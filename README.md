# bus-contract

A contract for an in-process bus of **hook points**: requests, events,
alter and collect points, with listeners in an order they declare.
Libraries declare their points and listen on them; the application
supplies the bus.

The bus is **passed in** at construction (`Documents::new(bus)`), never
a process-global, so one process or one test can hold several
applications, each with its own bus, or one bus shared by every
library. One bus or several is the application's choice, never a
library's.

## What it holds

- **Points are types** (`Point`): an id (`doc.close`), the input every
  listener reads, the value a listener edits, and a declared kind
  (`request`, `event`, `alter`, `collect`).
- **`Bus`**, an object-safe trait (`Arc<dyn Bus>`): declare a point;
  listen under an id and an owner, ordered by `after` / `before` /
  `last`, then priority; unlisten; forget an owner; `alter` and
  `collect` emits that return every listener's failure (error, panic,
  missed deadline); failure sinks; introspection.
- **Typed calls on `dyn Bus`** (`declare::<P>`, `listen::<P>`,
  `alter::<P>`, `collect::<P>`, `on_failure`), written once over the
  erased methods: an implementation writes only those.
- **A contract table** (`PointDecl`): a library's points as a `const`,
  readable before anything is built, declared with `declare_all`.
- **Tools on the contract**: `order_dump` ("who runs when", for reviewed
  files and debug views) and `missing_targets` (an ordering constraint
  naming a listener that is not there).
- No dependencies, no `unsafe`, no async runtime.

It runs nothing itself. An implementation keeps the listeners and runs
the emits: [`egui-pop-hooks`](https://github.com/quazardous/egui-pop)'s
`Kernel` is one (ordering, a per-listener error boundary with caught
panics, inline and threaded deadlines, counters), or write your own and
test against the promises documented on `Bus`.

## A library declares, the application supplies

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

The application builds one bus and hands it to every library, so their
points meet on it (another library can listen to `doc.close` at a
declared place, `after("docs.autosave")`):

```rust,ignore
let bus: Arc<dyn Bus> = Arc::new(egui_pop_hooks::Kernel::new()); // or its own
let docs = Documents::new(bus.clone());
let search = Search::new(bus.clone());
print!("{}", bus_contract::order_dump(&*bus, &["doc.close"]));
```

## Not in it (yet)

- A journal (events with a revision, catch-up from a revision): planned
  as a second trait once two implementations exist to cut it against.
- Transport across processes: a listener that answers from another
  process is the caller's transport, given a longer deadline
  (`PLUGIN_DEADLINE`).

## License

MIT OR Apache-2.0, at your option.
