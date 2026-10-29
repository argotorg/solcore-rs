pragma solcore;

import * from std;
import * from std.dispatch;
import lib;

struct Point {
    x: uint256;
}
struct Line {
    end: Point;
}

trait Picker<self> {
    function pick(value: self) returns (uint256);
}
impl Picker<Point> {
    function pick(value: Point) returns (uint256) {
        return value.x;
    }
}

function typeParameter(Point: Point) returns (uint256) {
    return Point.x;
}

function nestedModule(lib: Line) returns (uint256) {
    return lib.end.x;
}

function localUfcs(Picker: Point) returns (uint256) {
    return Picker.pick();
}

contract StructShadowedNamespace {
    // #[() -> 42]
    function localModule() public returns (uint256) {
        let lib = Point(42);
        return lib.x;
    }

    // #[() -> 42]
    function typeShadow() public returns (uint256) {
        return typeParameter(Point(42));
    }

    // #[() -> 42]
    function nestedShadow() public returns (uint256) {
        return nestedModule(Line(Point(42)));
    }

    // #[() -> 42]
    function ufcsShadow() public returns (uint256) {
        return localUfcs(Point(42));
    }

    // #[() -> 99]
    function qualified() public returns (uint256) {
        return lib.x();
    }
}
