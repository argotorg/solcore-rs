pragma solcore;

import * from std;

contract StorageIndexBinaryCompound {
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
        m[1] = 12;
        m[next()] /= next();
        return m[1];
    }
}
