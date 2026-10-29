pragma solcore;

pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
default impl<a> Choice<a> where a: Helper {}
impl<a> Helper<a> where a: Choice {}

//~ goal: Choice
//~ solution: unique main main std
