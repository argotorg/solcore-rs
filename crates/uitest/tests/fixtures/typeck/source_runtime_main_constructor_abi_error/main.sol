pragma solcore;

import * from std;

enum Choice {
    Left(word),
    Right(word)
}

contract C {
    constructor(value: Choice) {}
    function main() {
        return ();
    }
}
