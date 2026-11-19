//! Capability contracts as data: [`Contract`], what a point needs from a
//! bus, and [`Facet`], what a point's declaration says for one contract.
//! The core names no capability: each capability contract crate exports
//! its `CONTRACT` and its facets.

use std::any::{Any, TypeId};
use std::fmt;

use crate::Bus;

/// A capability contract a bus may offer: its name, its version line and
/// the trait object type a bus answers for in [`Bus::capability`]. Each
/// contract crate exports one as `CONTRACT`
/// (`bus_contract_journal::CONTRACT`); a point names those it needs in
/// its table ([`PointDecl::needs`](crate::PointDecl::needs)) and
/// [`declare_all`](crate::declare_all) refuses it on a bus without them.
///
/// Two contracts are the same when their trait object types are: two
/// semver-incompatible versions of a contract crate are two contracts,
/// so a library on one is refused, with a reason, by a bus that
/// implements the other.
#[derive(Clone, Copy)]
pub struct Contract {
    /// The contract's name (`journal`), for messages and introspection.
    pub name: &'static str,
    /// The version line of the crate that defines it (`0.2`).
    pub version: &'static str,
    /// The contracts a bus that offers this one offers too (crossing
    /// requires journal); [`offers`] checks them.
    pub requires: &'static [&'static Contract],
    type_id: fn() -> TypeId,
}

impl Contract {
    /// The contract `name`, version line `version`, offered by a bus that
    /// answers for the trait object type `C` (`dyn Journal`).
    pub const fn new<C: ?Sized + 'static>(
        name: &'static str,
        version: &'static str,
        requires: &'static [&'static Contract],
    ) -> Contract {
        Contract {
            name,
            version,
            requires,
            type_id: TypeId::of::<C>,
        }
    }

    /// The `TypeId` a bus answers for in [`Bus::capability`].
    pub fn type_id(&self) -> TypeId {
        (self.type_id)()
    }
}

impl PartialEq for Contract {
    fn eq(&self, other: &Contract) -> bool {
        self.type_id() == other.type_id()
    }
}

impl Eq for Contract {}

impl fmt::Debug for Contract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.name, self.version)
    }
}

impl fmt::Display for Contract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

/// Whether `bus` offers `contract`: it answers the lookup for the
/// contract's type ([`Bus::capability`]), and offers every contract it
/// requires.
pub fn offers(bus: &dyn Bus, contract: &Contract) -> bool {
    bus.capability(contract.type_id()).is_some() && contract.requires.iter().all(|r| offers(bus, r))
}

/// What a point's declaration says for one capability contract: the
/// contract, and data only that contract's crate reads (a journaled
/// mark, a codec). Made by the contract crate, in a `const`
/// (`bus_contract_journal::JOURNALED`,
/// `bus_contract_crossing::relayable(&CODEC)`), and carried by
/// [`PointDecl::facets`](crate::PointDecl::facets) and
/// [`PointSpec::facets`](crate::PointSpec::facets). A point with a facet
/// needs its contract.
#[derive(Clone, Copy)]
pub struct Facet {
    /// The contract the facet belongs to.
    pub contract: &'static Contract,
    /// What it says, read back by the contract's crate ([`Facet::get`]).
    pub data: &'static (dyn Any + Send + Sync),
}

impl Facet {
    /// A facet of `contract` saying `data`.
    pub const fn new(contract: &'static Contract, data: &'static (dyn Any + Send + Sync)) -> Facet {
        Facet { contract, data }
    }

    /// The facet's data as a `T`; `None` when it is another type.
    pub fn get<T: Any>(&self) -> Option<&'static T> {
        self.data.downcast_ref::<T>()
    }

    /// The first facet among `facets` of `contract` whose data is a `T`.
    pub fn find<T: Any>(facets: &[Facet], contract: &Contract) -> Option<&'static T> {
        facets
            .iter()
            .filter(|f| f.contract == contract)
            .find_map(Facet::get::<T>)
    }
}

/// Two facets are the same when they belong to the same contract and
/// carry data of the same type.
impl PartialEq for Facet {
    fn eq(&self, other: &Facet) -> bool {
        self.contract == other.contract && (*self.data).type_id() == (*other.data).type_id()
    }
}

impl Eq for Facet {}

impl fmt::Debug for Facet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Facet({})", self.contract.name)
    }
}
