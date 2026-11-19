pragma solcore;

// The first default applies through the assumption, but where `t: Helper`
// also holds the second applies too, at the same priority, and the goal is
// ambiguous: generic code may not choose the first.
pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
enum Box<a> {
    Box(a)
}
default impl<a> Choice<Box<a>> where a: Outer {}
default impl<a> Choice<Box<a>> where a: Helper {}

//~ goal: Choice Box param
//~ given: Outer
//~ solution: no-solution
