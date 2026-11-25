pragma solcore;
import * from std;
import * from std.dispatch;

// Parameters, a loop that updates an accumulator, and a variable whose old
// value stays on the stack after a reassignment, for the tests that run the
// compiled code and read the stack locations of its variables.
contract StackValues {
    result: uint256;

    function main() public returns (uint256) {
        // `keep` reads its argument from storage, which is 0 here, so that the
        // compiler cannot fold the call into a constant.
        let start: uint256 = result + uint256(5);
        result = mix(uint256(3), uint256(4)) + keep(start);
        return result;
    }
}

function mix(a: uint256, b: uint256) returns (uint256) {
    let acc: uint256 = uint256(0);
    for (let i: uint256 = uint256(0); i < b; i = i + uint256(1)) {
        acc = acc + a;
    }
    return acc;
}

function keep(p: uint256) returns (uint256) {
    let x: uint256 = p;
    let y: uint256 = x;
    x = x + uint256(100);
    return x * uint256(1000) + y;
}
