pragma solcore;

pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
enum Box<a> {
    Box(a)
}
default impl<a> Choice<Box<a>> {}

//~ goal: Choice Box
//~ solution: unique main
