pragma solcore;

import * from std;
import { Pair } from facade;

export { sizeViaFacade };

function sizeViaFacade() returns (word) {
    let p: Proxy<Pair>;
    return ABIAttribs.headSize(p);
}
