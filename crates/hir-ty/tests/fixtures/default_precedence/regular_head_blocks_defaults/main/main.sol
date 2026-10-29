pragma solcore;

pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
impl Choice<word> where word: Helper {}
default impl<a> Choice<a> {}

//~ goal: Choice
//~ solution: no-solution
