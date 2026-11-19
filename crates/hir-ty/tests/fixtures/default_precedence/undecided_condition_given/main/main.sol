pragma solcore;

// A condition that the checked code assumes is decided: the default applies.
pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
enum Box<a> {
    Box(a)
}
default impl<a> Choice<Box<a>> where a: Helper {}

//~ goal: Choice Box param
//~ given: Helper
//~ solution: unique main
