pragma solcore;

pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
enum Box<a> {
    Box(a)
}
default impl<a> Choice<Box<a>> where a: Helper {}
default impl<a> Choice<Box<a>> where a: Helper {}

//~ goal: Choice Box
//~ solution: ambiguous 2
