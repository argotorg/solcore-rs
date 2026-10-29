pragma solcore;

trait Leaf<a> {}
trait Left<a> {}
trait Right<a> {}
trait Top<a> {}

impl Leaf<word> {}

impl<a> Left<a> where a: Leaf {}
impl<a> Right<a> where a: Leaf {}
impl<a> Top<a> where a: Left, a: Right {}
