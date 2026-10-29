pragma solcore;

function foo() returns (function (word) returns (bool)) {
    return lam (x: word) -> bool {
        return true;
    };
}
