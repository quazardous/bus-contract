//! `BusDefer` reaches the capability on every kind of bus, and the items
//! re-exported here are the core's own.

use std::any::TypeId;
use std::sync::Arc;

use bus_contract_defer::BusDefer;
use bus_contract_testkit::{GetterBus, LookupBus, Offers, addr};

const OFFERS: Offers = Offers {
    defer: true,
    ..Offers::NONE
};

struct Asked;
impl bus_contract::Point for Asked {
    const ID: &'static str = "test.write.asked";
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
fn the_reexported_trait_is_the_cores() {
    assert_eq!(
        TypeId::of::<dyn bus_contract_defer::Defer>(),
        TypeId::of::<dyn bus_contract::Defer>()
    );
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
fn reached_on_a_bus_overriding_the_getter() {
    let bus: Arc<dyn bus_contract::Bus> = Arc::new(GetterBus::new(OFFERS));
    assert!(bus.defer().is_some());
    exercise(&bus);
}

#[test]
fn none_on_a_bus_without_it() {
    let everything_else = Offers {
        defer: false,
        ..Offers::ALL
    };
    let lookup: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(everything_else));
    assert!(lookup.defer().is_none());
    let getter: Arc<dyn bus_contract::Bus> = Arc::new(GetterBus::new(Offers::NONE));
    assert!(getter.defer().is_none());
}
