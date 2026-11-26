pragma solcore;

import { one, twice } from helpers;

function zero() returns (word) {
    return 0;
}

contract Counter {
    function reset() returns (word) {
        return 0;
    }

    function main(x: word) public returns (word) {
        let a = x.zero();
        let b = x.one();
        let c = x.reset();
        return x.twice();
    }
}
