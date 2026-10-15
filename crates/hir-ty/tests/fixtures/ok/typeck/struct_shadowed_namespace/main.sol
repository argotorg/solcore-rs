import lib;

struct Point {
    x: word;
}
struct Line {
    end: Point;
}

trait Picker<self> {
    function pick(value: self) returns (word);
}
impl Picker<Point> {
    function pick(value: Point) returns (word) {
        return value.x;
    }
}

function typeParameter(Point: Point) returns (word) {
    return Point.x;
}

function moduleParameter(lib: Point) returns (word) {
    return lib.x;
}

function localModule(value: word) returns (word) {
    let lib = Point(value);
    return lib.x;
}

function nestedModule(lib: Line) returns (word) {
    return lib.end.x;
}

function localUfcs(Picker: Point) returns (word) {
    return Picker.pick();
}

function qualified() returns (word) {
    return lib.x();
}
