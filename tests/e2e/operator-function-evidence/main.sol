pragma solcore;

import * from std;
import * from std.dispatch;
import { directA, genericA, explicitA, runtimeDirectA, runtimeGenericA } from a;
import { directB, genericB, explicitB, runtimeDirectB, runtimeGenericB } from b;

contract OperatorFunctionEvidence {
    // #[() -> 7]
    function direct() public returns (uint256) {
        return uint256(primAddWord(directA(), directB()));
    }
    // #[() -> 7]
    function generic() public returns (uint256) {
        return uint256(primAddWord(genericA(), genericB()));
    }
    // #[() -> 7]
    function explicit() public returns (uint256) {
        return uint256(primAddWord(explicitA(), explicitB()));
    }
    // #[(0) -> 3]
    // #[(1) -> 100]
    function directInA(x: uint256) public returns (uint256) {
        return uint256(runtimeDirectA(Typedef.rep(x)));
    }
    // #[(0) -> 4]
    // #[(1) -> 100]
    function directInB(x: uint256) public returns (uint256) {
        return uint256(runtimeDirectB(Typedef.rep(x)));
    }
    // #[(0) -> 3]
    // #[(1) -> 4]
    function genericInA(x: uint256) public returns (uint256) {
        return uint256(runtimeGenericA(Typedef.rep(x)));
    }
    // #[(0) -> 4]
    // #[(1) -> 3]
    function genericInB(x: uint256) public returns (uint256) {
        return uint256(runtimeGenericB(Typedef.rep(x)));
    }
}
