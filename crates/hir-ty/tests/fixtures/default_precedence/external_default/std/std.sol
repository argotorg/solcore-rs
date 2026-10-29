pragma solcore;

export { Choice, Helper, Outer };
trait Choice<a> {}
trait Helper<a> {}
trait Outer<a> {}
default impl<a> Choice<a> {}
