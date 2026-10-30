pragma solcore;

pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
default impl<a> Choice<a> where a: Helper {}

//~ goal: Choice
//~ solution: unique std
