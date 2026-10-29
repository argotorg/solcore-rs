pragma solcore;

enum Food {
    Curry,
    Beans,
    Other
}
enum CFood {
    Red(Food),
    Green(Food),
    Nocolor
}

function fromEnum(x: CFood) returns (word) {
    assembly {
        mstore(0, 0)
    }
    match (x) {
        case CFood.Red(Food.Curry) {
            return 1;
        }
        case CFood.Green(Food.Beans) {
            return 42;
        }
        default {
            return 3;
        }
    }
}

contract FoodContract {
    function main() returns (word) {
        return fromEnum(CFood.Green(Food.Beans));
    }
}
