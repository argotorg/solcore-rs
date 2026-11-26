pragma solcore;

import * from std;
import * from std.dispatch;
import { Unit, viaNewDictionary } from lib;

// This module imports no trait: the dictionary it builds for `Unit: Refined`
// does not hold the superclass `Unit: Base`.
contract SuperclassOfUnimportedTrait {
    // #[() -> 34]
    function newDictionary() public returns (uint256) {
        return uint256(viaNewDictionary(Unit.Unit));
    }
}
