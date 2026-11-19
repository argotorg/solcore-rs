pragma solcore;

// This default's head matches `Box<t>` only once `t` is a `Box`, and then it
// takes precedence over the std fallback.
pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
enum Box<a> {
    Box(a)
}
default impl<a> Choice<Box<Box<a>>> {}

//~ goal: Choice Box param
//~ solution: no-solution
