pragma solcore;

// Both defaults apply through the assumption, so the goal is ambiguous as it
// is for `Box<word>`.
pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
enum Box<a> {
    Box(a)
}
default impl<a> Choice<Box<a>> where a: Helper {}
default impl<a> Choice<Box<a>> where a: Helper {}

//~ goal: Choice Box param
//~ given: Helper
//~ solution: ambiguous 2
