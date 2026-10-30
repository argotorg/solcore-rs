pragma solcore;

trait Known<a> {}
impl Known<word> {}
trait Missing<a> {}
trait Cycle<a> {}
trait Other<a> {}
impl<a> Cycle<a> where a: Other {}
impl<a> Other<a> where a: Cycle {}
