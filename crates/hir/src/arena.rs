use std::{
    marker::PhantomData,
    ops::{Index, IndexMut},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct Id<T> {
    raw: u32,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Id<T> {
    pub fn as_usize(self) -> usize {
        self.raw as usize
    }
}

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
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn alloc(&mut self, value: T) -> Id<T> {
        let id = Id {
            raw: self.items.len() as u32,
            _marker: PhantomData,
        };
        self.items.push(value);
        id
    }

    pub fn get(&self, id: Id<T>) -> &T {
        &self.items[id.as_usize()]
    }

    pub fn get_mut(&mut self, id: Id<T>) -> &mut T {
        &mut self.items[id.as_usize()]
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

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
