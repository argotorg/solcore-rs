//! Typed index arena for HIR bodies.
//!
//! Function bodies store statements, expressions, and patterns in compact
//! arenas so recursive references can be represented by copyable IDs rather
//! than by nested boxes. An `Id<T>` is meaningful only for the `Arena<T>` that
//! allocated it.

use std::{
    marker::PhantomData,
    ops::{Index, IndexMut},
};

/// Typed index into an [`Arena`].
///
/// The `T` marker prevents accidentally indexing an expression arena with a
/// statement ID. IDs are stable for the lifetime of the arena because the arena
/// never removes or reorders items.
#[derive(Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct Id<T> {
    raw: u32,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Id<T> {}

impl<T> Id<T> {
    /// Returns the zero-based arena index for this ID.
    ///
    /// This is mainly for diagnostics, iteration, and implementing indexing.
    /// It does not identify an item outside the arena that allocated it.
    pub fn as_usize(self) -> usize {
        self.raw as usize
    }
}

/// Append-only typed arena.
///
/// The arena gives HIR bodies stable intra-body IDs without interning every
/// expression or statement in Salsa. Items can be mutated before the body is
/// frozen into a tracked value; after that, callers normally use shared access.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update, Default)]
pub struct Arena<T>
where
    T: salsa::Update,
{
    items: Vec<T>,
}

impl<T> Arena<T>
where
    T: salsa::Update,
{
    /// Creates an empty arena.
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Appends `value` and returns its typed ID.
    ///
    /// # Panics
    ///
    /// The raw index is stored as `u32`; this will wrap if more than
    /// `u32::MAX` items are allocated, which is outside the expected body size.
    pub fn alloc(&mut self, value: T) -> Id<T> {
        let id = Id {
            raw: self.items.len() as u32,
            _marker: PhantomData,
        };
        self.items.push(value);
        id
    }

    /// Returns the item for `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` was not allocated by this arena.
    pub fn get(&self, id: Id<T>) -> &T {
        &self.items[id.as_usize()]
    }

    /// Returns a mutable item for `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` was not allocated by this arena.
    pub fn get_mut(&mut self, id: Id<T>) -> &mut T {
        &mut self.items[id.as_usize()]
    }

    /// Returns the number of allocated items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns whether the arena contains no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Iterates over allocated IDs and their items in allocation order.
    pub fn iter(&self) -> impl Iterator<Item = (Id<T>, &T)> {
        self.items.iter().enumerate().map(|(i, v)| {
            (
                Id {
                    raw: i as u32,
                    _marker: PhantomData,
                },
                v,
            )
        })
    }
}

impl<T> Index<Id<T>> for Arena<T>
where
    T: salsa::Update,
{
    type Output = T;

    fn index(&self, index: Id<T>) -> &Self::Output {
        self.get(index)
    }
}

impl<T> IndexMut<Id<T>> for Arena<T>
where
    T: salsa::Update,
{
    fn index_mut(&mut self, index: Id<T>) -> &mut Self::Output {
        self.get_mut(index)
    }
}
