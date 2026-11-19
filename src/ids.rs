//! Validated ids: [`PointId`] and [`ListenerId`], checked when they are
//! made, in `const` as well as at run time.
//!
//! The idea of ids as newtypes checked at construction comes from
//! eventbus-contract's `Topic::new` and `ConsumerGroup::new`
//! (<https://github.com/mapseekai/eventbus-contract>), and the `const`
//! form that fails at compile time from qubit-event-bus's
//! `Topic::new_static` (<https://github.com/qubit-ltd/rs-event-bus>).

use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;

/// Why an id was refused ([`check_id`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum IdError {
    /// The id is empty.
    Empty,
    /// The id has no `<crate>.` prefix: it has one segment only.
    NoPrefix,
    /// A segment between two dots, or at either end, is empty
    /// (`a..b`, `a.`, `.a`); `at` is the byte where it is.
    EmptySegment {
        /// The byte offset of the empty segment.
        at: usize,
    },
    /// The byte at `at` is not allowed there.
    BadChar {
        /// The byte offset of the character.
        at: usize,
    },
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdError::Empty => f.write_str("the id is empty"),
            IdError::NoPrefix => {
                f.write_str("the id has no `<crate>.` prefix (`<crate>.<subject>.<moment>`)")
            }
            IdError::EmptySegment { at } => write!(f, "the id has an empty segment at byte {at}"),
            IdError::BadChar { at } => write!(
                f,
                "the id has a character not allowed at byte {at}: the prefix is `[a-z][a-z0-9_-]*`, the other segments `[A-Za-z0-9_-]+`"
            ),
        }
    }
}

impl std::error::Error for IdError {}

/// Checks `id` against the naming rule of point and listener ids,
/// `<crate>.<subject>.<moment>` (the README's "Naming points"):
///
/// - at least two segments separated by dots, none empty;
/// - the first segment, the crate's prefix, is a lowercase ASCII letter
///   followed by lowercase ASCII letters, digits, `_` or `-`;
/// - the other segments are ASCII letters (either case), digits, `_` or
///   `-`.
///
/// A `const fn`, so a table checks its ids at compile time.
pub const fn check_id(id: &str) -> Result<(), IdError> {
    let b = id.as_bytes();
    if b.is_empty() {
        return Err(IdError::Empty);
    }
    let mut i = 0;
    let mut start = 0;
    let mut segments = 1;
    while i < b.len() {
        let c = b[i];
        if c == b'.' {
            if i == start {
                return Err(IdError::EmptySegment { at: i });
            }
            segments += 1;
            start = i + 1;
        } else if segments == 1 {
            let ok =
                c.is_ascii_lowercase() || (i > 0 && (c.is_ascii_digit() || c == b'_' || c == b'-'));
            if !ok {
                return Err(IdError::BadChar { at: i });
            }
        } else if !(c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
            return Err(IdError::BadChar { at: i });
        }
        i += 1;
    }
    if segments < 2 {
        return Err(IdError::NoPrefix);
    }
    if start == b.len() {
        return Err(IdError::EmptySegment { at: b.len() });
    }
    Ok(())
}

/// Panics with the error's message: a `const` caller turns it into a
/// compile error.
const fn refuse(error: IdError) -> ! {
    match error {
        IdError::Empty => panic!("invalid bus id: the id is empty"),
        IdError::NoPrefix => {
            panic!("invalid bus id: no `<crate>.` prefix (`<crate>.<subject>.<moment>`)")
        }
        IdError::EmptySegment { .. } => panic!("invalid bus id: an empty segment"),
        IdError::BadChar { .. } => panic!(
            "invalid bus id: a character not allowed (prefix `[a-z][a-z0-9_-]*`, then `[A-Za-z0-9_-]+`)"
        ),
    }
}

/// A point's id ([`Point::ID`](crate::Point::ID)), checked when made
/// against the naming rule ([`check_id`]): `<crate>.<subject>.<moment>`.
///
/// Made in a `const`, an invalid id is a compile error:
///
/// ```
/// use bus_contract::PointId;
/// const ID: PointId = PointId::new("doc.close");
/// assert_eq!(ID, "doc.close");
/// ```
///
/// ```compile_fail
/// use bus_contract::PointId;
/// const ID: PointId = PointId::new("close"); // no `<crate>.` prefix
/// ```
///
/// It reads as a `&str` ([`Deref`], [`PointId::as_str`], comparisons
/// with `str`). Newtype ids checked at construction follow
/// eventbus-contract (<https://github.com/mapseekai/eventbus-contract>);
/// the `const` constructor follows qubit-event-bus's `Topic::new_static`
/// (<https://github.com/qubit-ltd/rs-event-bus>).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PointId(&'static str);

impl PointId {
    /// The id `id`, checked; panics when it breaks the naming rule, so a
    /// `const` with a bad id does not compile. [`PointId::try_new`]
    /// returns the error instead.
    pub const fn new(id: &'static str) -> PointId {
        match check_id(id) {
            Ok(()) => PointId(id),
            Err(e) => refuse(e),
        }
    }

    /// The id `id`, or why it breaks the naming rule.
    pub const fn try_new(id: &'static str) -> Result<PointId, IdError> {
        match check_id(id) {
            Ok(()) => Ok(PointId(id)),
            Err(e) => Err(e),
        }
    }

    /// The id as a string.
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl Deref for PointId {
    type Target = str;
    fn deref(&self) -> &str {
        self.0
    }
}

impl AsRef<str> for PointId {
    fn as_ref(&self) -> &str {
        self.0
    }
}

impl fmt::Display for PointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl fmt::Debug for PointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.0, f)
    }
}

impl TryFrom<&'static str> for PointId {
    type Error = IdError;
    fn try_from(id: &'static str) -> Result<PointId, IdError> {
        PointId::try_new(id)
    }
}

impl From<PointId> for &'static str {
    fn from(id: PointId) -> &'static str {
        id.0
    }
}

impl From<PointId> for String {
    fn from(id: PointId) -> String {
        id.0.to_owned()
    }
}

impl PartialEq<str> for PointId {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for PointId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<PointId> for str {
    fn eq(&self, other: &PointId) -> bool {
        self == other.0
    }
}

impl PartialEq<PointId> for &str {
    fn eq(&self, other: &PointId) -> bool {
        *self == other.0
    }
}

impl PartialEq<PointId> for String {
    fn eq(&self, other: &PointId) -> bool {
        self == other.0
    }
}

/// A listener's id, checked when made against the naming rule
/// ([`check_id`]): `<crate>.<what>` (`docs.autosave`), so the order
/// dump of a point shows who each listener belongs to.
///
/// [`ListenerId::new`] is `const` and panics on a bad id, so a `const`
/// with one does not compile; [`ListenerId::try_new`] checks an id made
/// at run time. The typed `listen` calls still take any
/// `impl Into<String>`, a `ListenerId` or a `&str`: the bus checks the id
/// when the listener registers and refuses a bad one
/// ([`Refused::BadId`](crate::Refused::BadId)).
///
/// ```
/// use bus_contract::ListenerId;
/// const SAVE: ListenerId = ListenerId::new("docs.autosave");
/// assert!(ListenerId::try_new(format!("docs.{}", "draft")).is_ok());
/// assert!(ListenerId::try_new("autosave").is_err());
/// assert_eq!(SAVE, "docs.autosave");
/// ```
///
/// Newtype ids checked at construction follow eventbus-contract
/// (<https://github.com/mapseekai/eventbus-contract>); the `const`
/// constructor follows qubit-event-bus's `Topic::new_static`
/// (<https://github.com/qubit-ltd/rs-event-bus>).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ListenerId(Cow<'static, str>);

impl ListenerId {
    /// The id `id`, checked; panics when it breaks the naming rule, so a
    /// `const` with a bad id does not compile.
    pub const fn new(id: &'static str) -> ListenerId {
        match check_id(id) {
            Ok(()) => ListenerId(Cow::Borrowed(id)),
            Err(e) => refuse(e),
        }
    }

    /// The id `id`, made at run time, or why it breaks the naming rule.
    pub fn try_new(id: impl Into<String>) -> Result<ListenerId, IdError> {
        let id = id.into();
        check_id(&id)?;
        Ok(ListenerId(Cow::Owned(id)))
    }

    /// The id as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for ListenerId {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ListenerId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ListenerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for ListenerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

impl TryFrom<&str> for ListenerId {
    type Error = IdError;
    fn try_from(id: &str) -> Result<ListenerId, IdError> {
        ListenerId::try_new(id)
    }
}

impl TryFrom<String> for ListenerId {
    type Error = IdError;
    fn try_from(id: String) -> Result<ListenerId, IdError> {
        ListenerId::try_new(id)
    }
}

impl From<ListenerId> for String {
    fn from(id: ListenerId) -> String {
        id.0.into_owned()
    }
}

impl From<&ListenerId> for String {
    fn from(id: &ListenerId) -> String {
        id.0.clone().into_owned()
    }
}

impl PartialEq<str> for ListenerId {
    fn eq(&self, other: &str) -> bool {
        *self.0 == *other
    }
}

impl PartialEq<&str> for ListenerId {
    fn eq(&self, other: &&str) -> bool {
        *self.0 == **other
    }
}

impl PartialEq<ListenerId> for str {
    fn eq(&self, other: &ListenerId) -> bool {
        *self == *other.0
    }
}

impl PartialEq<ListenerId> for String {
    fn eq(&self, other: &ListenerId) -> bool {
        **self == *other.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_naming_rule() {
        for ok in [
            "doc.close",
            "stack.layout.changed",
            "bus.overBudget",
            "egui_pop.a-b.c_d9",
            "a1.x",
        ] {
            assert_eq!(check_id(ok), Ok(()), "{ok}");
        }
        assert_eq!(check_id(""), Err(IdError::Empty));
        assert_eq!(check_id("close"), Err(IdError::NoPrefix));
        assert_eq!(check_id("a..b"), Err(IdError::EmptySegment { at: 2 }));
        assert_eq!(check_id(".a"), Err(IdError::EmptySegment { at: 0 }));
        assert_eq!(check_id("a."), Err(IdError::EmptySegment { at: 2 }));
        assert_eq!(check_id("Doc.close"), Err(IdError::BadChar { at: 0 }));
        assert_eq!(check_id("1doc.close"), Err(IdError::BadChar { at: 0 }));
        assert_eq!(check_id("doc.cl ose"), Err(IdError::BadChar { at: 6 }));
        assert_eq!(check_id("doc.*"), Err(IdError::BadChar { at: 4 }));
        assert_eq!(check_id("doc.é"), Err(IdError::BadChar { at: 4 }));
    }

    #[test]
    fn newtypes_read_as_str() {
        const P: PointId = PointId::new("doc.close");
        assert_eq!(P, "doc.close");
        assert_eq!(&*P, "doc.close");
        assert_eq!(P.to_string(), "doc.close");
        assert!(PointId::try_new("close").is_err());
        let l = ListenerId::try_new("doc.save").unwrap();
        assert_eq!(String::from(l.clone()), "doc.save");
        assert_eq!(l, "doc.save");
    }

    #[test]
    #[should_panic(expected = "invalid bus id")]
    fn new_panics_at_run_time() {
        let id = String::from("x");
        let _ = ListenerId::new(Box::leak(id.into_boxed_str()));
    }
}
