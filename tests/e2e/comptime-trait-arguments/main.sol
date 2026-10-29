pragma solcore;

import * from std;
import * from std.dispatch;

enum Box<a> {
    Box(a)
}
trait Qualified<t> {
    function value(x: t) returns (word);
}
impl<a> Qualified<Box<a>> {
    function value(x: Box<a>) returns (word) {
        return 7;
    }
}
function indirect<t>(x: t) returns (word) where t: Qualified {
    return Qualified.value(x);
}

contract ComptimeTraitArguments {
    // #[() -> 7]
    function direct() public returns (uint256) {
        let x: comptime<Box<comptime<word>>> = Box(3);
        return uint256(Qualified.value(x));
    }

    // #[() -> 7]
    function nested() public returns (uint256) {
        let x: comptime<Box<comptime<word>>> = Box(3);
        return uint256(indirect(x));
    }
}
