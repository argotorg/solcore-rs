pragma solcore;

import * from std;

contract MappingWriter {
    m: mapping(word => word);

    function set(k: word, v: word) returns (word) {
        m[k] = v;
        return v;
    }

    function main() public returns (word) {
        let a: word = set(1, 42);
        return m[1];
    }
}
