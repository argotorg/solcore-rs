pragma solcore;

trait Enum<a> {
    function fromEnum(x: a) returns (word);
}

enum Food {
    Curry,
    Beans,
    Other
}

function main() returns (word) {
    return Enum.fromEnum(Food.Beans);
}
