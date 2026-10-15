import * from std;
import * from std.dispatch;

pragma no-generic-instance-for Point;
enum Point {
    Point(word, word)
}

contract Shapes {
    function roundtrip(p: Point) public returns (Point) {
        return p;
    }
}
