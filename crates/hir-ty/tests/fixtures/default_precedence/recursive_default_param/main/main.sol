pragma solcore;

// The default applies to `Box<t>` itself, its condition proved by the std
// fallback, exactly as for `Box<word>`.
pragma no-patterson-condition Choice, Helper, Outer;
import * from std;
enum Box<a> {
    Box(a)
}
default impl<a> Choice<Box<a>> where Box<a>: Choice {}

//~ goal: Choice Box param
//~ solution: unique main std
