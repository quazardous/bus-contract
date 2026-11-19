//! `BusTimings` reaches the capability on every kind of bus through the
//! lookup, and the contract is what `offers` and `declare_all` check.

use std::sync::Arc;

use bus_contract_testkit::{LookupBus, Offers, addr};
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
fn none_on_a_bus_without_it() {
    let everything_else = Offers {
        timings: false,
        ..Offers::ALL
    };
    let lookup: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(everything_else));
    assert!(lookup.timings().is_none());
}

#[test]
fn the_contract_is_offered_by_a_bus_that_answers_the_lookup() {
    let with: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(OFFERS));
    let without: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(Offers::NONE));
    assert!(bus_contract::offers(
        &*with,
        &bus_contract_timings::CONTRACT
    ));
    assert!(!bus_contract::offers(
        &*without,
        &bus_contract_timings::CONTRACT
    ));
    assert_eq!(
        bus_contract_timings::CONTRACT.type_id(),
        std::any::TypeId::of::<dyn bus_contract_timings::Timings>()
    );
    assert_eq!(bus_contract_timings::CONTRACT.version, "0.2");
}
