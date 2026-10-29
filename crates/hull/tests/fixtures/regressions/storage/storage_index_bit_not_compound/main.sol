pragma solcore;

import * from std;

contract StorageIndexBitNotCompound {
    counter: word;
    m: mapping(word => word);

    function next() returns (word) {
        let cur: word = counter;
        let res: word;
        assembly {
            res := add(cur, 1)
        }
        counter = res;
        return res;
    }

    function main() public returns (word) {
        counter = 0;
        m[1] = 10;
        m[next()] ~=;
        return m[1];
    }
}
