pragma solcore;

enum Pair<a, b> {
    Pair(a, b)
}

trait Leaf<a> {}
trait Left<a> {}
trait Right<a> {}
trait Top<a> {}

impl<a> Leaf<a> {}

impl<a, b, c> Left<a> where Pair<b, c>: Leaf {}
impl<a, c, b> Right<a> where Pair<b, c>: Leaf {}
impl<a> Top<a> where a: Left, a: Right {}
