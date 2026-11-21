# Naming points

Several libraries share one bus, so a point's id starts with a prefix
that names the crate declaring it, then the subject and the moment:
`<crate>.<subject>.<moment>`.

- A library uses one short prefix of its own, written in its docs:
  `stack.layout.changed`, `stack.window.closed` for a docking crate
  called `egui_stack`; `doc.close` for the README's example.
- An event names what happened, in the past (`…changed`, `…closed`).
  A request names what is asked (`…close`, `…drop`).
- A library never declares a point under another crate's prefix, and
  never asks another crate to rename its points to free a bare name.
- Listener ids follow the same rule (`docs.autosave`), so the
  `order_dump` of a point shows who each listener belongs to.

Two libraries that keep to this cannot collide, whatever the
application puts on its bus. A clash is refused at declaration time
(`Refused::TypeMismatch`): a point id is registered with one `Point`
type. If two libraries declare the same id with different `Point`
types, the second declaration is refused and its point stays
undeclared.

## The rule is checked

`check_id` checks it: at least two dot-separated segments, none empty;
the prefix is `[a-z][a-z0-9_-]*`, the other segments `[A-Za-z0-9_-]+`.

- A point's id is a `PointId`. `PointId::new` is a `const fn` that
  panics on a bad id. Since `Point::ID` is a `const`, that panic happens
  while the compiler evaluates the constant: a bad id is a **compile
  error**, not a runtime panic. `const ID: PointId = PointId::new("close");`
  does not build. A `PointId` reads as a `&str` (`Deref`, `as_str`,
  comparisons with `str`).
- A listener's id may be a `ListenerId` (`ListenerId::new` in a `const`,
  `ListenerId::try_new` at run time), or any string: the bus checks it
  when the listener registers and refuses a bad one
  (`Refused::BadId`). An implementation also refuses a point key whose
  id breaks the rule. If you build a listener id from run-time data,
  use `ListenerId::try_new` to see the error before you register.

Newtype ids checked at construction come from
[eventbus-contract](https://github.com/mapseekai/eventbus-contract)
(`Topic::new`, `ConsumerGroup::new`); the `const` form that fails at
compile time from [qubit-event-bus](https://github.com/qubit-ltd/rs-event-bus)
(`Topic::new_static`).
