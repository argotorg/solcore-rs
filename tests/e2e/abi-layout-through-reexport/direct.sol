pragma solcore;

import * from std;
import { Pair } from types;

export { sizeDirect };

function sizeDirect() returns (word) {
    let p: Proxy<Pair>;
    return ABIAttribs.headSize(p);
}
