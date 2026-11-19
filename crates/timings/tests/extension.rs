//! `BusTimings` reaches the capability on every kind of bus, and the items
//! re-exported here are the core's own.

use std::any::TypeId;
use std::sync::Arc;

use bus_contract_testkit::{GetterBus, LookupBus, Offers, addr};
use bus_contract_timings::BusTimings;

const OFFERS: Offers = Offers {
    timings: true,
    ..Offers::NONE
};

/// Calls the capability once, through `BusTimings`.
fn exercise<B: BusTimings + ?Sized>(bus: &B) -> u64 {
    bus.timings().unwrap().points().len() as u64
}

#[test]
fn the_reexported_trait_is_the_cores() {
    assert_eq!(
        TypeId::of::<dyn bus_contract_timings::Timings>(),
        TypeId::of::<dyn bus_contract::Timings>()
    );
}

#[test]
fn reached_on_a_bus_offering_it_through_the_lookup() {
    let shared: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(OFFERS));
    let by_ext = shared.timings().expect("the extension finds it");
    let by_lookup = shared
        .get::<dyn bus_contract_timings::Timings>()
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
    assert!(bus.timings().is_some());
    exercise(&bus);
}

#[test]
fn none_on_a_bus_without_it() {
    let everything_else = Offers {
        timings: false,
        ..Offers::ALL
    };
    let lookup: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(everything_else));
    assert!(lookup.timings().is_none());
    let getter: Arc<dyn bus_contract::Bus> = Arc::new(GetterBus::new(Offers::NONE));
    assert!(getter.timings().is_none());
}
