pragma solcore;

pragma no-generic-instance-for Wrap, Point;

import * from std;
import * from std.Generic;

// The pragma removes the synthesized Generic impl, and nothing replaces it.
#[derive(Eq)]
enum Wrap {
    Wrap(word)
}

// A hand-written Generic impl replaces the synthesized one.
#[derive(Eq)]
enum Point {
    Point(word, word)
}

impl Generic<Point, (word, word)> {
    function from(x: Point) returns ((word, word)) {
        match (x) {
            case Point(a, b) {
                return (a, b);
            }
        }
    }
    function to(x: (word, word)) returns (Point) {
        match (x) {
            case (a, b) {
                return Point(a, b);
            }
        }
    }
}
