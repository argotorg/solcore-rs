import * from std;
import * from std.dispatch;
import { Wrapper } from types;

// Selecting Wrapper's derived instance must validate its structural body in
// the definition module even though this consumer does not import the marker.
contract C {
    value: Wrapper;

    constructor() {}
}
