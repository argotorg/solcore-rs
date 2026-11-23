pragma solcore;

import * from std;
import * from traits;

export { Unit(*), viaNewDictionary };

enum Unit {
    Unit
}

impl Base<Unit> {
    function base(x: Unit) returns (word) {
        return 4;
    }
}

// The method uses the superclass of its own dictionary.
impl Refined<Unit> {
    function refined(x: Unit) returns (word) {
        return primAddWord(Base.base(x), 30);
    }
}

// Builds `Unit: Refined` while it holds a dictionary for the same predicate.
// The caller built that one without seeing `Base`, so it has no superclass;
// the dictionary built here has.
function viaNewDictionary<t>(x: t) returns (word) where t: Refined {
    return Refined.refined(Unit.Unit);
}
