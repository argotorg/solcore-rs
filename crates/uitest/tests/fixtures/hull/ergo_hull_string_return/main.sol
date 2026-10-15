// A runtime function whose result type is not representable in Hull.
import { string } from std;

contract Answer {
    function main() returns (string) {
        return helper();
    }
}

function helper() returns (string) {
    return "42";
}
