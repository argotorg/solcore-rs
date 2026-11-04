pragma solcore;

pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
enum Box<a> {
    Box(a)
}
impl Choice<Box<word>> where Box<word>: Helper {}
default impl<a> Choice<Box<a>> {}

//~ goal: Choice Box
//~ solution: no-solution
