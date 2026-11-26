pragma solcore;

// No impl head unifies with `Box<t>: Helper`, and every impl that could is
// visible here, so the default never applies to a `Box` and the std fallback
// decides the goal, as it does for `Box<word>`.
pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
enum Box<a> {
    Box(a)
}
default impl<a> Choice<Box<a>> where Box<a>: Helper {}

//~ goal: Choice Box param
//~ solution: unique std
