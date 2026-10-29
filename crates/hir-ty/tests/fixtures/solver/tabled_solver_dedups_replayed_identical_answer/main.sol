pragma solcore;

trait Seed<a> {}
trait Derived<a> {}

impl Seed<word> {}

impl<a> Derived<a> where a: Seed, a: Seed {}
