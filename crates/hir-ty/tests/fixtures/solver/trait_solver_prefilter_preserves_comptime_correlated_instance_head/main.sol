pragma solcore;

trait Correlated<a> {}
impl<x> Correlated<(comptime<x>, x)> {}
