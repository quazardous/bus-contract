//! `BusCrossing` reaches the capability on every kind of bus, and the items
//! re-exported here are the core's own.

use std::any::TypeId;
use std::sync::Arc;

use bus_contract_crossing::BusCrossing;
use bus_contract_crossing::BusJournal;
use bus_contract_testkit::{GetterBus, LookupBus, Offers, addr};

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
fn the_reexported_trait_is_the_cores() {
    assert_eq!(
        TypeId::of::<dyn bus_contract_crossing::Relay>(),
        TypeId::of::<dyn bus_contract::Relay>()
    );
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
fn reached_on_a_bus_overriding_the_getter() {
    let bus: Arc<dyn bus_contract::Bus> = Arc::new(GetterBus::new(OFFERS));
    assert!(bus.relay().is_some());
    exercise(&bus);
}

#[test]
fn none_on_a_bus_without_it() {
    let everything_else = Offers {
        relay: false,
        ..Offers::ALL
    };
    let lookup: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(everything_else));
    assert!(lookup.relay().is_none());
    let getter: Arc<dyn bus_contract::Bus> = Arc::new(GetterBus::new(Offers::NONE));
    assert!(getter.relay().is_none());
}

#[test]
fn the_journal_under_the_relay_is_reached_from_here_too() {
    let bus: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(OFFERS));
    let relay = bus.relay().unwrap();
    let journal = bus.journal().unwrap();
    assert_eq!(addr(relay), addr(journal));
}
