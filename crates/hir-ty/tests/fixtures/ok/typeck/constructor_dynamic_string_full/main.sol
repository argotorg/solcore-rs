import * from std;

contract C {
    value: string;

    constructor(x: memory<string>) {
        value = x;
    }

    function get() public returns (memory<string>) {
        return value;
    }

    function main() {
        return ();
    }
}
