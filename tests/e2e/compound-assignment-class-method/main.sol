import * from std;
import * from std.dispatch;

enum Choice {
    Choice(uint256)
}

impl Add<Choice> {
    function add(l: Choice, r: Choice) returns (Choice) {
        return r;
    }
}

contract CompoundAssignmentClassMethod {
    constructor() {}

    // #[(3, 7) -> 7]
    // #[(11, 5) -> 5]
    function choose_right(x: uint256, y: uint256) public returns (uint256) {
        let result: Choice = Choice(x);
        result += Choice(y);
        match (result) {
            case Choice(value) {
                return value;
            }
        }
    }
}
