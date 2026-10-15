import * from std;
import * from std.dispatch;

enum Point {
    Point(word, bool)
}

contract C {
    function roundtrip(value: memory<Point>) public returns (word) {
        return 0;
    }
}
