pragma solcore;

// direct-impl: false
trait Eq<a> {}
trait Ord<a> where a: Eq {}
impl Ord<word> {}
