pragma solcore;

import * from abi;
import { Leaf } from reexport;

export { keepCompetitorReachable };

// This orphan is reachable from the entry module but is not visible in the
// module that defines Box. A derived wrapper must replay definition-side
// evidence rather than scanning every reachable environment.
impl ABIAttribs<Leaf> {
    function headSize(ty: Proxy<Leaf>) returns (word) {
        return 64;
    }
    function isStatic(ty: Proxy<Leaf>) returns (bool) {
        return true;
    }
}

function keepCompetitorReachable() returns (word) {
    return 0;
}
