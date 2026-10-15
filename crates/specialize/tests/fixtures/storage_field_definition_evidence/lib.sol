import * from std;

export { keepLibReachable };

function keepLibReachable() returns (word) {
    return 0;
}

contract C {
    value: uint256;

    function main() returns (uint256) {
        return value;
    }
}
