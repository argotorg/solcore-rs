pragma solcore;

trait Eq<a> {}
trait Ord<a> where a: Eq {}
impl Eq<word> {}
impl Ord<word> {}
