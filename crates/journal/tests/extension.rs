//! `BusJournal` reaches the capability on every kind of bus, and the items
//! re-exported here are the core's own.

use std::any::TypeId;
use std::sync::Arc;

use bus_contract_journal::BusJournal;
use bus_contract_testkit::{GetterBus, LookupBus, Offers, addr};

const OFFERS: Offers = Offers {
    journal: true,
    ..Offers::NONE
};

/// Calls the capability once, through `BusJournal`.
fn exercise<B: BusJournal + ?Sized>(bus: &B) -> u64 {
    bus.journal().unwrap().last_seq()
}

#[test]
fn the_reexported_trait_is_the_cores() {
    assert_eq!(
        TypeId::of::<dyn bus_contract_journal::Journal>(),
        TypeId::of::<dyn bus_contract::Journal>()
    );
}

#[test]
fn reached_on_a_bus_offering_it_through_the_lookup() {
    let shared: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(OFFERS));
    let by_ext = shared.journal().expect("the extension finds it");
    let by_lookup = shared
        .get::<dyn bus_contract_journal::Journal>()
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
    assert!(bus.journal().is_some());
    exercise(&bus);
}

#[test]
fn none_on_a_bus_without_it() {
    let everything_else = Offers {
        journal: false,
        ..Offers::ALL
    };
    let lookup: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(everything_else));
    assert!(lookup.journal().is_none());
    let getter: Arc<dyn bus_contract::Bus> = Arc::new(GetterBus::new(Offers::NONE));
    assert!(getter.journal().is_none());
}
