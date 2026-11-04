pragma solcore;

// A dependency's default for its own trait and an application default for
// the application's own type constructor share a priority tier, so neither
// wins when both apply to `Box<word>`.
pragma no-patterson-condition Pick;
import * from std;
import * from @custom.overrides;
enum Box<a> {
    Box(a)
}
impl<a> Ready<Box<a>> {}
default impl<a> Pick<Box<a>> where a: Ready {}

//~ goal: Pick Box
//~ solution: ambiguous 2
