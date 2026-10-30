pragma solcore;

import { memory, string } from std;

contract A {
    function main() returns (memory<string>) {
        return "shared";
    }
}
contract B {
    function main() returns (memory<string>) {
        return "shared";
    }
}
