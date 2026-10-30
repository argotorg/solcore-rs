pragma solcore;

struct Point {
    x: word;
    y: word;
}

function update(p: Point) returns (()) {
    p.x = 1;
}
