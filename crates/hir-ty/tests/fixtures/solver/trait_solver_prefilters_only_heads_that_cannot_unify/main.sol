pragma solcore;

trait Target<a> {}
trait Noise<a> {}
trait DefaultTarget<a> {}
trait GenericTarget<a> {}
trait GivenTarget<a> {}
trait Parent<a> {}
trait Child<a> where a: Parent {}
trait AmbiguousTarget<a> {}

impl Target<word> {}
impl Noise<bool> {}
default impl<a> Noise<a> {}
default impl<a> DefaultTarget<a> {}
impl<a> GenericTarget<a> {}
impl AmbiguousTarget<word> {}
impl AmbiguousTarget<word> {}
