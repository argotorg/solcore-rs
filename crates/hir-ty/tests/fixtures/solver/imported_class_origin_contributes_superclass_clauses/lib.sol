pragma solcore;

export { Eq, Ord };

trait Eq<a> {}
trait Ord<a> where a: Eq {}
