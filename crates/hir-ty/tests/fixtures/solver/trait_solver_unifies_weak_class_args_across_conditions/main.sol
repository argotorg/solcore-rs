pragma solcore;

enum Uint {
    Uint(word)
}

trait Typedef<abs, rep> {}
impl Typedef<Uint, word> {}

trait StorageSize<a> {}
impl StorageSize<word> {}

impl<a, b> StorageSize<a> where a: Typedef<b>, b: StorageSize {}
