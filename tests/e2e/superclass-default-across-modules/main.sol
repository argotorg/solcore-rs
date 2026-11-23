pragma solcore;

import * from std;
import * from std.dispatch;
import * from lib;

enum Foo {
    Foo
}

impl Q<Foo> {}

contract SuperclassDefaultAcrossModules {
    // #[() -> 2]
    function throughLibrary() public returns (uint256) {
        return uint256(boxed(Foo.Foo));
    }

    // #[() -> 2]
    function direct() public returns (uint256) {
        return uint256(sizeViaB(Box.Box(Foo.Foo)));
    }
}
