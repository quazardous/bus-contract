# Design note: the Journal and Relay capabilities

The core contract delivers an emit to its listeners inline, on the
emitting thread, in a declared order. Applications that already run a
fact bus (past-tense events with a sequence number, a window read again
from a number, events relayed between processes) had to keep it beside
the contract: two vocabularies, anonymous subscribers draining channels.
`Journal` and `Relay` let such a bus implement the contract's missing
parts instead, as optional capabilities shaped like `Defer`.

## Decisions

**Optional, found at run time.** `Bus::journal()` and `Bus::relay()`
return `None` by default. An implementation of 0.3 keeps compiling and
offers neither. A point that needs one says so in its declaration
(`journaled()`, `relayable(codec)`) and `declare_all` refuses it on a
bus that cannot serve it (`Refused::Unsupported`), as `needs` does for
`Defer`. `PointDecl::required()` lists the capabilities a point needs,
named and implied.

**Journaled is a property of an event point.** Only `PointKind::Event`
points are journaled (`Refused::NotEvent` otherwise): requests and
alters return values to their caller, which a deferred delivery cannot.
A journaled point's events are published through the journal, not
emitted; the same point type is used to listen, so a listener does not
know whether its events were published here or relayed in.

**Publishing never runs a listener.** Fact publishers (renderers, file
watchers, database writers) must not block on a listener. The journal
records the entry and returns its number; a pump of the
implementation's delivers entries in number order as collect emits,
with every rule of the core (order, deadlines, failures, local
listeners queued for their thread). The cost is head-of-line blocking
on the pump: a slow listener delays the next entries. Deadlines bound
it; an implementation may run one pump per bus.

**Numbers per journal, a run per process.** Numbers start at 1 and are
never reused while the process runs. A restart starts numbers again, so
a relay cursor carries the journal's run: a cursor of another run is
answered with `Lost::Restarted`, an evicted range with `Lost::Gap`. A
reader that lost entries resyncs from the current state, then applies
what it was given. `Since::gap` says the same for in-process readers.

**Codecs as function pointers.** The crate has no dependencies, so it
does not pick a format. A relayable point implements `Relayed` (text in,
text out) and its table line carries `Codec::of::<P>()`: three function
pointers built in a `const`. Two processes of different versions must
read each other, so decoders are asked to accept unknown fields and
fail only on a payload they cannot use (`IngestError::Decode`).

**What crosses, and what does not.** Only relayable points are served
by `resume`, so internal, high-rate events stay in their process. An
entry relayed in keeps its source and the sender's cause number; it is
not served again, so two processes relaying to each other do not echo.
A kind the receiver does not declare is kept as text, delivered to no
one, readable with `since`. A point declared on the receiver but not
relayable is refused: a remote process may not publish it.

**No transport.** A relay reads (`resume`) and takes in (`ingest`); the
application moves entries over its own socket or RPC.

## Not decided here

- Whether `Journal` entries of kinds with no point (an implementation's
  own untyped facts) get a contract type: the journal keeps them as
  text, which is enough to read and relay them by hand.
- Back-pressure on the pump (a bounded queue, dropping, or blocking the
  publisher past a size): left to implementations for now.
