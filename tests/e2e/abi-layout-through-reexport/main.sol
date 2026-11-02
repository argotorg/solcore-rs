pragma solcore;

import * from std;
import * from std.dispatch;
import { sizeViaFacade } from viafacade;
import { sizeDirect } from direct;

// `Pair` has a compiler-derived `ABIAttribs` instance next to its definition.
// A module that reaches `Pair` through a re-export must see the same instance
// (two words, 64 bytes), not fall back to a single-word default.
contract AbiLayoutThroughReexport {
    // #[() -> 64]
    function viaFacade() public returns (uint256) {
        return uint256(sizeViaFacade());
    }

    // #[() -> 64]
    function direct() public returns (uint256) {
        return uint256(sizeDirect());
    }
}
