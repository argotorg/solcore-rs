pragma solcore;

// In generic code `t: Helper` is not known, so the std fallback cannot decide
// `Box<t>: Choice`: once `t` is instantiated, this default may apply and take
// precedence.
pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
enum Box<a> {
    Box(a)
}
default impl<a> Choice<Box<a>> where a: Helper {}

//~ goal: Choice Box param
//~ solution: no-solution
