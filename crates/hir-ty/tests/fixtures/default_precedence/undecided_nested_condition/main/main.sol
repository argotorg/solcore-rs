pragma solcore;

// The condition `Box<t>: Helper` reduces to `t: Outer`, which is not known in
// generic code.
pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
enum Box<a> {
    Box(a)
}
impl<a> Helper<Box<a>> where a: Outer {}
default impl<a> Choice<Box<a>> where Box<a>: Helper {}

//~ goal: Choice Box param
//~ solution: no-solution
