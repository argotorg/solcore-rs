pragma solcore;

pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
enum Box<a> {
    Box(a)
}
default impl<a> Choice<Box<a>> {}
impl<a> Outer<Box<a>> where Box<a>: Choice {}

//~ goal: Outer Box
//~ solution: unique main main
