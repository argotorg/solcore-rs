import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

trait Toggle<a> {
    function toggle(value: a) returns (a);
}

enum Switch {
    Off,
    On
}

impl Toggle<Switch> {
    function toggle(value: Switch) returns (Switch) {
        match (value) {
            case Switch.Off { return Switch.On; }
            case Switch.On { return Switch.Off; }
        }
    }
}

contract LightSwitch {
    state : Switch;

    constructor() {
        state = Switch.Off;
    }

    function flip() public {
        state = Toggle.toggle(state);
    }

    function isOn() public returns (bool) {
        match (state) {
            case Switch.On { return true; }
            default { return false; }
        }
    }
}
