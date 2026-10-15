import * from std;

contract C {
    value: bytes;

    constructor(x: memory<bytes>) {
        value = x;
    }

    function get() public returns (memory<bytes>) {
        return value;
    }

    function main() {
        return ();
    }
}
