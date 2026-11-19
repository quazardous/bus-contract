//! `BusCrossing` reaches the capability on every kind of bus through the
//! lookup, and the contract is what `offers` and `declare_all` check.

use std::sync::Arc;

use bus_contract_crossing::BusCrossing;
use bus_contract_crossing::BusJournal;
use bus_contract_testkit::{LookupBus, Offers, addr};

const OFFERS: Offers = Offers {
    relay: true,
    journal: true,
    ..Offers::NONE
};

/// Calls the capability once, through `BusCrossing`.
fn exercise<B: BusCrossing + ?Sized>(bus: &B) -> u64 {
    bus.relay().unwrap().cursor().seq
}

#[test]
fn reached_on_a_bus_offering_it_through_the_lookup() {
    let shared: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(OFFERS));
    let by_ext = shared.relay().expect("the extension finds it");
    let by_lookup = shared
        .get::<dyn bus_contract_crossing::Relay>()
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
        relay: false,
        ..Offers::ALL
    };
    let lookup: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(everything_else));
    assert!(lookup.relay().is_none());
}

#[test]
fn the_journal_under_the_relay_is_reached_from_here_too() {
    let bus: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(OFFERS));
    let relay = bus.relay().unwrap();
    let journal = bus.journal().unwrap();
    assert_eq!(addr(relay), addr(journal));
}

#[test]
fn the_contract_is_offered_by_a_bus_that_answers_the_lookup() {
    let with: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(OFFERS));
    let without: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(Offers::NONE));
    assert!(bus_contract::offers(
        &*with,
        &bus_contract_crossing::CONTRACT
    ));
    assert!(!bus_contract::offers(
        &*without,
        &bus_contract_crossing::CONTRACT
    ));
    assert_eq!(
        bus_contract_crossing::CONTRACT.type_id(),
        std::any::TypeId::of::<dyn bus_contract_crossing::Relay>()
    );
    assert_eq!(bus_contract_crossing::CONTRACT.version, "0.2");
}
