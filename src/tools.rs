//! Tools that work on any implementation of the contract: declaring a
//! library's table, the order dump and the missing targets a test ends
//! without.

use crate::{Bus, LAST, PointDecl, PointKind, Refused, offers};

/// Declares every point of a library's contract table on `bus`; the
/// refusals, in table order, each point refused left undeclared:
///
/// - a point already declared (another library, or a second application
///   on the same bus);
/// - a point that needs a capability the bus does not offer
///   ([`Refused::Unsupported`]): those it names, the journal for a
///   journaled point, the relay for a relayable one
///   ([`PointDecl::required`]);
/// - a journaled point that is not an event ([`Refused::NotEvent`]);
/// - a relayable point that cannot cross: an alter or collect point, a
///   request whose codec writes no value ([`Refused::NotCrossable`]);
/// - a relayable point with another point's codec
///   ([`Refused::TypeMismatch`]).
pub fn declare_all(bus: &dyn Bus, table: &[PointDecl]) -> Vec<Refused> {
    table
        .iter()
        .filter_map(|d| {
            if d.journaled && d.kind != PointKind::Event {
                return Some(Refused::NotEvent { point: d.id });
            }
            if let Some(codec) = d.relay {
                if codec.point != d.id {
                    return Some(Refused::TypeMismatch { point: d.id });
                }
                let crosses = match d.kind {
                    PointKind::Event => true,
                    PointKind::Request => codec.answers(),
                    _ => false,
                };
                if !crosses {
                    return Some(Refused::NotCrossable { point: d.id });
                }
            }
            if let Some(capability) = d.required().into_iter().find(|c| !offers(bus, *c)) {
                return Some(Refused::Unsupported {
                    point: d.id,
                    capability,
                });
            }
            bus.declare_point(d.key(), d.spec()).err()
        })
        .collect()
}

/// The order the listeners of `points` run in, one point after the other,
/// each listener with its owner and what it declares: "who runs when",
/// read rather than remembered.
///
/// ```text
/// doc.close
///   1. doc.children (owner docs): before doc.registry; priority 1000
///   2. doc.registry (owner docs): nothing
///   3. doc.invariants (owner -): last
/// ```
///
/// A local listener ([`Bus::listen_local_any`]) is marked `local`.
pub fn order_dump(bus: &dyn Bus, points: &[&str]) -> String {
    let mut out = String::new();
    for point in points {
        out.push_str(point);
        out.push('\n');
        for (n, l) in bus.listeners(point).unwrap_or_default().iter().enumerate() {
            let mut declares: Vec<String> = Vec::new();
            if l.local {
                declares.push("local".into());
            }
            if l.after.iter().any(|a| a == LAST) {
                declares.push("last".into());
            }
            let after: Vec<&str> = l
                .after
                .iter()
                .filter(|a| *a != LAST)
                .map(String::as_str)
                .collect();
            if !after.is_empty() {
                declares.push(format!("after {}", after.join(", ")));
            }
            if !l.before.is_empty() {
                declares.push(format!("before {}", l.before.join(", ")));
            }
            if l.priority != 0 {
                declares.push(format!("priority {}", l.priority));
            }
            if !l.missing.is_empty() {
                declares.push(format!("MISSING {}", l.missing.join(", ")));
            }
            let declares = if declares.is_empty() {
                "nothing".to_string()
            } else {
                declares.join("; ")
            };
            out.push_str(&format!(
                "  {}. {} (owner {}): {declares}\n",
                n + 1,
                l.id,
                l.owner.as_deref().unwrap_or("-"),
            ));
        }
    }
    out
}

/// The ordering constraints of `points` that name a listener not there:
/// `point: listener -> target`. A test ends with none.
pub fn missing_targets(bus: &dyn Bus, points: &[&str]) -> Vec<String> {
    points
        .iter()
        .flat_map(|p| {
            bus.listeners(p)
                .unwrap_or_default()
                .into_iter()
                .flat_map(move |l| {
                    l.missing
                        .iter()
                        .map(|t| format!("{p}: {} -> {t}", l.id))
                        .collect::<Vec<_>>()
                })
        })
        .collect()
}
