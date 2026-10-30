pragma solcore;

pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
default impl<a> Choice<a> {}
impl<a> Outer<a> where a: Choice {}

//~ goal: Outer
//~ solution: unique main main
