function g(x: word) returns (word) {
    x
}

function h<a>(x: a) returns (a) {
    x
}

contract C {
    function main() public returns (word) {
        g(h(42))
    }
}
