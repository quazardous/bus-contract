//! `BusDefer` reaches the capability on every kind of bus through the
//! lookup, and the contract is what `offers` and `declare_all` check.

use std::sync::Arc;

use bus_contract_defer::BusDefer;
use bus_contract_testkit::{LookupBus, Offers, addr};

const OFFERS: Offers = Offers {
    defer: true,
    ..Offers::NONE
};

struct Asked;
impl bus_contract::Point for Asked {
    const ID: bus_contract::PointId = bus_contract::PointId::new("test.write.asked");
    type Input = ();
    type Value = u32;
}

fn key() -> bus_contract::PointKey {
    bus_contract::PointKey::of::<Asked>()
}

fn clone_u32(v: &(dyn std::any::Any + Send)) -> bus_contract::AnyValue {
    Box::new(*v.downcast_ref::<u32>().unwrap())
}

/// Calls the capability once, through `BusDefer`.
fn exercise<B: BusDefer + ?Sized>(bus: &B) -> u64 {
    bus.defer()
        .unwrap()
        .ask_any(key(), Arc::new(()), Box::new(0u32), clone_u32)
}

#[test]
fn reached_on_a_bus_offering_it_through_the_lookup() {
    let shared: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(OFFERS));
    let by_ext = shared.defer().expect("the extension finds it");
    let by_lookup = shared
        .get::<dyn bus_contract_defer::Defer>()
        .expect("the lookup finds it");
    assert_eq!(addr(by_ext), addr(by_lookup));
    exercise(&shared);
    exercise(&*shared);
    exercise(&LookupBus::new(OFFERS));
    exercise(&Arc::new(LookupBus::new(OFFERS)));
}

#[test]
fn none_on_a_bus_without_it() {
    let everything_else = Offers {
        defer: false,
        ..Offers::ALL
    };
    let lookup: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(everything_else));
    assert!(lookup.defer().is_none());
}

#[test]
fn the_contract_is_offered_by_a_bus_that_answers_the_lookup() {
    let with: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(OFFERS));
    let without: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(Offers::NONE));
    assert!(bus_contract::offers(&*with, &bus_contract_defer::CONTRACT));
    assert!(!bus_contract::offers(
        &*without,
        &bus_contract_defer::CONTRACT
    ));
    assert_eq!(
        bus_contract_defer::CONTRACT.type_id(),
        std::any::TypeId::of::<dyn bus_contract_defer::Defer>()
    );
    assert_eq!(bus_contract_defer::CONTRACT.version, "0.2");
}

const NEEDS: &[bus_contract::PointDecl] =
    &[bus_contract::PointDecl::of::<Asked>("Asked for later.")
        .needs(&[&bus_contract_defer::CONTRACT])];

#[test]
fn a_point_needing_defer_is_refused_on_a_bus_without_it() {
    let without: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(Offers::NONE));
    assert_eq!(
        bus_contract::declare_all(&*without, NEEDS),
        [bus_contract::Refused::Unsupported {
            point: "test.write.asked",
            contract: "defer"
        }]
    );
    let with: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(OFFERS));
    assert!(bus_contract::declare_all(&*with, NEEDS).is_empty());
}
