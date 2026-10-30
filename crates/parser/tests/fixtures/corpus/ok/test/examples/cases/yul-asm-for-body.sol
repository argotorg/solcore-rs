pragma solcore;

import * from std;

function yul_asm_for_body() {
    let result: word = 0;
    assembly {
        for {
            let i := 0
        } lt(i, 3) {
            i := add(i, 1)
        } {
            result := callvalue()
        }
    }
}

contract Foo {
    function main() public {
        yul_asm_for_body()
    }
}
