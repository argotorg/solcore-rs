pragma solcore;

trait Seed<a> {}
trait Derived<a> {}
trait Needs<a> {}

impl Seed<word> {}

impl<a> Derived<a> where a: Seed {}
impl<a> Needs<a> where a: Seed, a: Derived {}
